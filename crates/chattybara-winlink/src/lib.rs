//! Transport-neutral Winlink mailbox primitives.
//!
//! This crate intentionally separates mailbox state from transport mechanics.
//! The fake transport is complete enough for deterministic tests. Telnet, VARA,
//! and orca reports are guarded scaffolds that share the same store and safety
//! model while the full session protocols are built out.

use base64::Engine as _;
use flate2::read::GzDecoder;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const DEFAULT_CMS_HOST: &str = "cms-z.winlink.org";
pub const DEFAULT_CMS_PORT: u16 = 8772;
pub const DEFAULT_TELNET_TIMEOUT_MS: u64 = 3000;
pub const WINLINK_PASSWORD_ENV: &str = "CHATTYBARA_WINLINK_PASSWORD";
const CMS_TELNET_PASSWORD: &str = "CMSTELNET";
const TELNET_PROMPT_MAX_BYTES: usize = 4096;
const B2F_LINE_MAX_BYTES: usize = 8192;
const B2F_MAX_COMPRESSED_BYTES: usize = 10 * 1024 * 1024;
const B2F_MAX_UNCOMPRESSED_BYTES: usize = 25 * 1024 * 1024;
const B2F_SOH: u8 = 0x01;
const B2F_STX: u8 = 0x02;
const B2F_EOT: u8 = 0x04;
const WINLINK_SECURE_SALT: [u8; 64] = [
    77, 197, 101, 206, 190, 249, 93, 200, 51, 243, 93, 237, 71, 94, 239, 138, 68, 108, 70, 185,
    225, 137, 217, 16, 51, 122, 193, 48, 194, 195, 198, 175, 172, 169, 70, 84, 61, 62, 104, 186,
    114, 52, 61, 168, 66, 129, 192, 208, 187, 249, 232, 193, 41, 113, 41, 45, 240, 16, 29, 228,
    208, 228, 61, 20,
];
pub const DEFAULT_VARA_HOST: &str = "127.0.0.1";
pub const DEFAULT_VARA_COMMAND_PORT: u16 = 8300;
pub const DEFAULT_VARA_DATA_PORT: u16 = 8301;

#[derive(Debug, Error)]
pub enum WinlinkError {
    #[error("empty Winlink station call sign")]
    EmptyStation,
    #[error("empty Winlink recipient")]
    EmptyRecipient,
    #[error("empty Winlink subject")]
    EmptySubject,
    #[error("empty Winlink body")]
    EmptyBody,
    #[error("unknown Winlink transport: {0}")]
    UnknownTransport(String),
    #[error("Winlink message not found: {0}")]
    MessageNotFound(String),
    #[error(
        "live Winlink {transport} sync is not implemented in this alpha; use dry-run or fake transport"
    )]
    LiveSyncNotImplemented { transport: String },
    #[error("Winlink {transport} send requires --allow-send when queued outbox mail exists")]
    SendNotAllowed { transport: String },
    #[error("invalid B2F proposal: {0}")]
    InvalidB2fProposal(String),
    #[error("live Winlink Telnet/CMS inbox check requires {0} in the environment")]
    MissingPasswordEnv(&'static str),
    #[error("Winlink Telnet/CMS protocol error: {0}")]
    Protocol(String),
    #[error("Winlink store I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Winlink store JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

pub type WinlinkResult<T> = std::result::Result<T, WinlinkError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinlinkAccount {
    pub station: String,
    pub address: String,
    pub password_source: CredentialSource,
}

impl WinlinkAccount {
    pub fn new(station: impl AsRef<str>, password_source: CredentialSource) -> WinlinkResult<Self> {
        let station = normalize_call(station.as_ref())?;
        Ok(Self {
            address: format!("{station}@winlink.org"),
            station,
            password_source,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialSource {
    None,
    Env,
    Keychain,
}

impl CredentialSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Env => "env",
            Self::Keychain => "keychain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WinlinkTransportKind {
    Fake,
    #[serde(rename = "telnet-cms")]
    Telnet,
    Vara,
    Orca,
}

impl WinlinkTransportKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fake => "fake",
            Self::Telnet => "telnet-cms",
            Self::Vara => "vara",
            Self::Orca => "orca",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fake" | "fixture" => Some(Self::Fake),
            "telnet" | "telnet-cms" | "cms" => Some(Self::Telnet),
            "vara" | "vara-hf" | "vara-fm" => Some(Self::Vara),
            "orca" => Some(Self::Orca),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MailFolder {
    Inbox,
    Outbox,
    Sent,
    Drafts,
}

impl MailFolder {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Outbox => "outbox",
            Self::Sent => "sent",
            Self::Drafts => "drafts",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageState {
    Draft,
    Queued,
    Sent,
    Received,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinlinkAttachment {
    pub filename: String,
    pub byte_count: u64,
    pub sha256: String,
    pub source_path: Option<PathBuf>,
}

impl WinlinkAttachment {
    pub fn from_path(path: impl AsRef<Path>) -> WinlinkResult<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path)?;
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("attachment.bin")
            .to_owned();
        Ok(Self {
            filename,
            byte_count: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            source_path: Some(path.to_owned()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinlinkMessage {
    pub id: String,
    pub folder: MailFolder,
    pub state: MessageState,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<WinlinkAttachment>,
    pub transport: Option<WinlinkTransportKind>,
    pub last_error: Option<String>,
}

impl WinlinkMessage {
    pub fn payload_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        writeln!(&mut out, "From: {}", self.from).expect("write string");
        writeln!(&mut out, "To: {}", self.to.join(", ")).expect("write string");
        writeln!(&mut out, "Subject: {}", self.subject).expect("write string");
        writeln!(&mut out).expect("write string");
        out.push_str(&self.body);
        out.into_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinlinkStore {
    pub kind: String,
    pub station: String,
    pub account: Option<WinlinkAccount>,
    pub next_sequence: u64,
    pub messages: Vec<WinlinkMessage>,
}

impl WinlinkStore {
    pub fn new(station: impl AsRef<str>) -> WinlinkResult<Self> {
        Ok(Self {
            kind: "chattybara-winlink-store".to_owned(),
            station: normalize_call(station.as_ref())?,
            account: None,
            next_sequence: 1,
            messages: Vec::new(),
        })
    }

    pub fn load_or_new(path: impl AsRef<Path>, station: impl AsRef<str>) -> WinlinkResult<Self> {
        let path = path.as_ref();
        if path.exists() {
            let store: Self = serde_json::from_str(&fs::read_to_string(path)?)?;
            Ok(store)
        } else {
            Self::new(station)
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> WinlinkResult<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn set_account(&mut self, account: WinlinkAccount) {
        self.station = account.station.clone();
        self.account = Some(account);
    }

    pub fn queue_message(
        &mut self,
        to: Vec<String>,
        subject: impl AsRef<str>,
        body: impl AsRef<str>,
        attachments: Vec<WinlinkAttachment>,
    ) -> WinlinkResult<String> {
        if to.is_empty() {
            return Err(WinlinkError::EmptyRecipient);
        }
        let to = to
            .into_iter()
            .map(|value| normalize_recipient(&value))
            .collect::<WinlinkResult<Vec<_>>>()?;
        let subject = require_text(subject.as_ref(), TextField::Subject)?;
        let body = require_text(body.as_ref(), TextField::Body)?;
        let id = self.next_message_id();
        let message = WinlinkMessage {
            id: id.clone(),
            folder: MailFolder::Outbox,
            state: MessageState::Queued,
            from: format!("{}@winlink.org", self.station),
            to,
            subject,
            body,
            attachments,
            transport: None,
            last_error: None,
        };
        self.messages.push(message);
        Ok(id)
    }

    pub fn messages_in(&self, folder: MailFolder) -> Vec<&WinlinkMessage> {
        self.messages
            .iter()
            .filter(|message| message.folder == folder)
            .collect()
    }

    pub fn find_message(&self, id: &str) -> WinlinkResult<&WinlinkMessage> {
        self.messages
            .iter()
            .find(|message| message.id == id)
            .ok_or_else(|| WinlinkError::MessageNotFound(id.to_owned()))
    }

    pub fn find_message_mut(&mut self, id: &str) -> WinlinkResult<&mut WinlinkMessage> {
        self.messages
            .iter_mut()
            .find(|message| message.id == id)
            .ok_or_else(|| WinlinkError::MessageNotFound(id.to_owned()))
    }

    pub fn has_message(&self, id: &str) -> bool {
        self.messages.iter().any(|message| message.id == id)
    }

    fn has_complete_inbox_message(&self, id: &str) -> bool {
        self.messages.iter().any(|message| {
            message.id == id && message.folder == MailFolder::Inbox && message.last_error.is_none()
        })
    }

    fn next_message_id(&mut self) -> String {
        let id = format!("{}-{:04}", self.station, self.next_sequence);
        self.next_sequence += 1;
        id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct B2fProposal {
    pub message_id: String,
    pub byte_count: u64,
    pub checksum: String,
    pub subject: String,
}

impl B2fProposal {
    pub fn from_message(message: &WinlinkMessage) -> Self {
        let payload = message.payload_bytes();
        Self {
            message_id: message.id.clone(),
            byte_count: payload.len() as u64,
            checksum: sha256_hex(&payload),
            subject: message.subject.clone(),
        }
    }

    pub fn to_line(&self) -> String {
        format!(
            "FC EM {} {} {} {}",
            self.message_id, self.byte_count, self.checksum, self.subject
        )
    }

    pub fn parse(line: &str) -> WinlinkResult<Self> {
        let mut parts = line.trim().splitn(6, char::is_whitespace);
        let command = parts.next().unwrap_or_default();
        let message_type = parts.next().unwrap_or_default();
        let message_id = parts.next().unwrap_or_default();
        let byte_count = parts.next().unwrap_or_default();
        let checksum = parts.next().unwrap_or_default();
        let subject = parts.next().unwrap_or_default();
        if command != "FC"
            || message_type != "EM"
            || message_id.is_empty()
            || byte_count.is_empty()
            || checksum.is_empty()
            || subject.is_empty()
        {
            return Err(WinlinkError::InvalidB2fProposal(line.to_owned()));
        }
        let byte_count = byte_count
            .parse::<u64>()
            .map_err(|_| WinlinkError::InvalidB2fProposal(line.to_owned()))?;
        Ok(Self {
            message_id: message_id.to_owned(),
            byte_count,
            checksum: checksum.to_owned(),
            subject: subject.to_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinlinkSyncReport {
    pub kind: &'static str,
    pub ok: bool,
    pub station: String,
    pub transport: WinlinkTransportKind,
    pub dry_run: bool,
    pub live: bool,
    pub store_path: Option<PathBuf>,
    pub inbox_received: usize,
    pub outbox_sent: usize,
    pub queued_remaining: usize,
    pub notes: Vec<String>,
}

pub fn fake_sync(
    store: &mut WinlinkStore,
    store_path: Option<PathBuf>,
) -> WinlinkResult<WinlinkSyncReport> {
    let inbox_received = if store
        .messages
        .iter()
        .any(|message| message.id == fake_inbox_id(&store.station))
    {
        0
    } else {
        store.messages.push(fake_inbox_message(&store.station));
        1
    };
    let mut outbox_sent = 0;
    for message in store
        .messages
        .iter_mut()
        .filter(|message| message.folder == MailFolder::Outbox)
    {
        message.folder = MailFolder::Sent;
        message.state = MessageState::Sent;
        message.transport = Some(WinlinkTransportKind::Fake);
        message.last_error = None;
        outbox_sent += 1;
    }
    let queued_remaining = store.messages_in(MailFolder::Outbox).len();
    Ok(WinlinkSyncReport {
        kind: "winlink-sync-report",
        ok: true,
        station: store.station.clone(),
        transport: WinlinkTransportKind::Fake,
        dry_run: false,
        live: false,
        store_path,
        inbox_received,
        outbox_sent,
        queued_remaining,
        notes: vec!["fake CMS sync completed without network or radio".to_owned()],
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportStatusReport {
    pub kind: &'static str,
    pub ok: bool,
    pub station: String,
    pub transport: WinlinkTransportKind,
    pub dry_run: bool,
    pub live: bool,
    pub endpoint: Option<String>,
    pub connected: bool,
    pub greeting: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetCmsConfig {
    pub station: String,
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
    pub live: bool,
}

impl TelnetCmsConfig {
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

pub fn telnet_cms_check(config: TelnetCmsConfig) -> WinlinkResult<TransportStatusReport> {
    let station = normalize_call(&config.station)?;
    let endpoint = config.endpoint();
    if !config.live {
        return Ok(TransportStatusReport {
            kind: "winlink-transport-status",
            ok: true,
            station,
            transport: WinlinkTransportKind::Telnet,
            dry_run: true,
            live: false,
            endpoint: Some(endpoint),
            connected: false,
            greeting: None,
            notes: vec![
                "dry run only; pass --live to open a TCP connection".to_owned(),
                "live Telnet/CMS sync can receive messages; sending requires --allow-send"
                    .to_owned(),
            ],
        });
    }

    let address = endpoint
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("could not resolve {endpoint}")))?;
    let timeout = Duration::from_millis(config.timeout_ms);
    let mut stream = TcpStream::connect_timeout(&address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(b"\r\n")?;
    let mut buffer = [0_u8; 256];
    let greeting = match stream.read(&mut buffer) {
        Ok(0) => None,
        Ok(len) => Some(String::from_utf8_lossy(&buffer[..len]).trim().to_owned()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            None
        }
        Err(error) => return Err(error.into()),
    };
    Ok(TransportStatusReport {
        kind: "winlink-transport-status",
        ok: true,
        station,
        transport: WinlinkTransportKind::Telnet,
        dry_run: false,
        live: true,
        endpoint: Some(endpoint),
        connected: true,
        greeting,
        notes: vec![
            "TCP connectivity check only; no password was sent".to_owned(),
            "live Telnet/CMS sync can receive messages; sending requires --allow-send".to_owned(),
        ],
    })
}

pub fn winlink_password_from_env() -> Option<String> {
    std::env::var(WINLINK_PASSWORD_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub fn telnet_cms_receive_sync(
    store: &mut WinlinkStore,
    store_path: Option<PathBuf>,
    config: TelnetCmsConfig,
    password: Option<&str>,
    allow_send: bool,
) -> WinlinkResult<WinlinkSyncReport> {
    let station = normalize_call(&config.station)?;
    if !config.live {
        return Ok(WinlinkSyncReport {
            kind: "winlink-sync-report",
            ok: true,
            station: store.station.clone(),
            transport: WinlinkTransportKind::Telnet,
            dry_run: true,
            live: false,
            store_path,
            inbox_received: 0,
            outbox_sent: 0,
            queued_remaining: store.messages_in(MailFolder::Outbox).len(),
            notes: vec!["dry run only; pass --live to open a Telnet/CMS inbox session".to_owned()],
        });
    }

    let mut session = TelnetCmsSession::connect(&config)?;
    session.read_until_contains(&[b"Callsign", b"callsign"], TELNET_PROMPT_MAX_BYTES)?;
    session.write_line(&station)?;
    session.read_until_contains(&[b"Password", b"password"], TELNET_PROMPT_MAX_BYTES)?;
    session.write_line(CMS_TELNET_PASSWORD)?;

    let handshake = session.read_remote_handshake()?;
    session.write_line(&format!(";FW {station}"))?;
    session.write_line(&local_sid_line())?;
    if let Some(challenge) = handshake.secure_challenge.as_deref() {
        let password = password.ok_or(WinlinkError::MissingPasswordEnv(WINLINK_PASSWORD_ENV))?;
        let response = secure_login_response(challenge, password);
        session.write_line(&format!(";PR: {response}"))?;
    }

    let mut outbox_sent = 0;
    if allow_send {
        outbox_sent = send_outbox_messages(&mut session, store)?;
    } else {
        session.write_line("FF")?;
    }

    let mut pending = HashMap::<String, PendingCmsMessage>::new();
    let mut proposals = Vec::<InboundCmsProposal>::new();
    let mut proposal_lines = Vec::<String>::new();
    let mut added = 0;
    let mut notes = vec![
        "live Telnet/CMS authenticated; supported inbound proposals are downloaded".to_owned(),
        "live Telnet/CMS sending runs only when --allow-send is explicitly set".to_owned(),
    ];

    loop {
        let line = session.read_line(B2F_LINE_MAX_BYTES)?;
        if line.is_empty() {
            continue;
        }
        if let Some(message) = PendingCmsMessage::parse(&line) {
            pending.insert(message.mid.clone(), message);
            continue;
        }
        if line.starts_with(';') {
            continue;
        }
        match line.get(0..2).unwrap_or_default() {
            "FA" | "FB" | "FC" | "FD" => {
                let proposal = InboundCmsProposal::parse(&line)?;
                proposal_lines.push(line);
                proposals.push(proposal);
            }
            "F>" => {
                verify_proposal_checksum(&proposal_lines, &line)?;
                let answers = inbound_proposal_answers(store, &proposals);
                session.write_line(&format!("FS {answers}"))?;
                for (proposal, answer) in proposals.drain(..).zip(answers.bytes()) {
                    if answer != b'+' {
                        continue;
                    }
                    let transfer = session.read_b2_transfer(&proposal)?;
                    let payload = decode_b2_payload(&proposal, &transfer)?;
                    let parsed = parse_b2_message(
                        &station,
                        &proposal,
                        pending.get(&proposal.mid),
                        &transfer.title,
                        &payload,
                    )?;
                    if upsert_received_message(store, store_path.as_deref(), parsed)? {
                        added += 1;
                    }
                }
                proposal_lines.clear();
                session.write_line("FQ")?;
                break;
            }
            "FF" => {
                notes.push("CMS reported no pending inbound proposals".to_owned());
                session.write_line("FQ")?;
                break;
            }
            "FQ" => break,
            _ => {
                return Err(WinlinkError::Protocol(format!(
                    "unexpected protocol line: {line}"
                )));
            }
        }
    }

    Ok(WinlinkSyncReport {
        kind: "winlink-sync-report",
        ok: true,
        station: store.station.clone(),
        transport: WinlinkTransportKind::Telnet,
        dry_run: false,
        live: true,
        store_path,
        inbox_received: added,
        outbox_sent,
        queued_remaining: store.messages_in(MailFolder::Outbox).len(),
        notes,
    })
}

pub fn transport_plan_report(
    station: impl AsRef<str>,
    transport: WinlinkTransportKind,
    live: bool,
) -> WinlinkResult<TransportStatusReport> {
    let station = normalize_call(station.as_ref())?;
    let (endpoint, notes) = match transport {
        WinlinkTransportKind::Fake => (
            None,
            vec!["fake transport is implemented for local deterministic sync".to_owned()],
        ),
        WinlinkTransportKind::Telnet => (
            Some(format!("{DEFAULT_CMS_HOST}:{DEFAULT_CMS_PORT}")),
            vec!["Telnet/CMS supports dry-run and live TCP connectivity checks".to_owned()],
        ),
        WinlinkTransportKind::Vara => (
            Some(format!(
                "{DEFAULT_VARA_HOST}:{DEFAULT_VARA_COMMAND_PORT}/{DEFAULT_VARA_DATA_PORT}"
            )),
            vec![
                "VARA is planned as an external operator-installed modem adapter".to_owned(),
                "live VARA sync is disabled until the control/data session is implemented"
                    .to_owned(),
            ],
        ),
        WinlinkTransportKind::Orca => (
            Some("orca local-node/audio/radio session".to_owned()),
            vec![
                "orca is planned as the experimental open modem transport".to_owned(),
                "Winlink mailbox state will stay in chattybara; packet/audio/link mechanics stay in orca"
                    .to_owned(),
            ],
        ),
    };
    Ok(TransportStatusReport {
        kind: "winlink-transport-status",
        ok: true,
        station,
        transport,
        dry_run: !live,
        live,
        endpoint,
        connected: false,
        greeting: None,
        notes,
    })
}

pub fn guarded_dry_run_sync_report(
    store: &WinlinkStore,
    store_path: Option<PathBuf>,
    transport: WinlinkTransportKind,
    live: bool,
    allow_send: bool,
) -> WinlinkResult<WinlinkSyncReport> {
    let queued = store.messages_in(MailFolder::Outbox).len();
    if queued > 0 && live && !allow_send {
        return Err(WinlinkError::SendNotAllowed {
            transport: transport.label().to_owned(),
        });
    }
    if live {
        return Err(WinlinkError::LiveSyncNotImplemented {
            transport: transport.label().to_owned(),
        });
    }
    Ok(WinlinkSyncReport {
        kind: "winlink-sync-report",
        ok: true,
        station: store.station.clone(),
        transport,
        dry_run: true,
        live: false,
        store_path,
        inbox_received: 0,
        outbox_sent: 0,
        queued_remaining: queued,
        notes: vec![format!(
            "{} sync is a guarded dry run in this alpha",
            transport.label()
        )],
    })
}

struct TelnetCmsSession {
    stream: TcpStream,
}

impl TelnetCmsSession {
    fn connect(config: &TelnetCmsConfig) -> WinlinkResult<Self> {
        let endpoint = config.endpoint();
        let address = endpoint
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::other(format!("could not resolve {endpoint}")))?;
        let timeout = Duration::from_millis(config.timeout_ms);
        let stream = TcpStream::connect_timeout(&address, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(Self { stream })
    }

    fn write_line(&mut self, line: &str) -> WinlinkResult<()> {
        let mut buffer = Vec::with_capacity(line.len() + 1);
        buffer.extend_from_slice(line.as_bytes());
        buffer.push(b'\r');
        self.stream.write_all(&buffer)?;
        self.stream.flush()?;
        Ok(())
    }

    fn read_until_contains(
        &mut self,
        patterns: &[&[u8]],
        max_bytes: usize,
    ) -> WinlinkResult<String> {
        let mut buffer = Vec::new();
        let mut byte = [0_u8; 1];
        let mut matched = false;
        while buffer.len() < max_bytes {
            match self.stream.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    buffer.push(byte[0]);
                    matched |= patterns
                        .iter()
                        .any(|pattern| contains_ascii_case_insensitive(&buffer, pattern));
                    if matched && matches!(byte[0], b'\r' | b'\n') {
                        return Ok(String::from_utf8_lossy(&buffer).trim().to_owned());
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(WinlinkError::Protocol(
                        "timed out waiting for Telnet/CMS prompt".to_owned(),
                    ));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(WinlinkError::Protocol(
            "Telnet/CMS prompt exceeded maximum length".to_owned(),
        ))
    }

    fn read_remote_handshake(&mut self) -> WinlinkResult<RemoteHandshake> {
        let mut handshake = RemoteHandshake::default();
        loop {
            let line = self.read_line(B2F_LINE_MAX_BYTES)?;
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                handshake.sid = Some(line);
                continue;
            }
            if let Some(challenge) = parse_secure_challenge(&line) {
                handshake.secure_challenge = Some(challenge);
                continue;
            }
            if line.ends_with('>') {
                handshake.prompt = Some(line);
                break;
            }
            if line.starts_with("***") {
                return Err(WinlinkError::Protocol(line));
            }
        }
        Ok(handshake)
    }

    fn read_line(&mut self, max_bytes: usize) -> WinlinkResult<String> {
        let mut buffer = Vec::new();
        let mut byte = [0_u8; 1];
        while buffer.len() < max_bytes {
            match self.stream.read(&mut byte) {
                Ok(0) => {
                    if buffer.is_empty() {
                        return Err(WinlinkError::Protocol(
                            "Telnet/CMS connection closed".to_owned(),
                        ));
                    }
                    break;
                }
                Ok(_) => match byte[0] {
                    b'\r' | b'\n' => break,
                    value => buffer.push(value),
                },
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(WinlinkError::Protocol(
                        "timed out waiting for B2F line".to_owned(),
                    ));
                }
                Err(error) => return Err(error.into()),
            }
        }
        if buffer.len() >= max_bytes {
            return Err(WinlinkError::Protocol(
                "B2F line exceeded maximum length".to_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&buffer).trim_end().to_owned())
    }

    fn read_byte(&mut self) -> WinlinkResult<u8> {
        let mut byte = [0_u8; 1];
        match self.stream.read_exact(&mut byte) {
            Ok(()) => Ok(byte[0]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err(WinlinkError::Protocol(
                    "timed out waiting for B2F binary data".to_owned(),
                ))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn read_exact_bytes(&mut self, len: usize) -> WinlinkResult<Vec<u8>> {
        let mut buffer = vec![0_u8; len];
        match self.stream.read_exact(&mut buffer) {
            Ok(()) => Ok(buffer),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err(WinlinkError::Protocol(
                    "timed out waiting for B2F binary data".to_owned(),
                ))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn read_b2_transfer(&mut self, proposal: &InboundCmsProposal) -> WinlinkResult<B2Transfer> {
        let first = self.read_byte()?;
        if first == b'*' {
            let detail = self.read_line(B2F_LINE_MAX_BYTES)?;
            return Err(WinlinkError::Protocol(format!(
                "CMS transfer error: *{detail}"
            )));
        }
        if first != B2F_SOH {
            return Err(WinlinkError::Protocol(format!(
                "expected B2F SOH before payload, got 0x{first:02X}"
            )));
        }

        let header_len = usize::from(self.read_byte()?);
        let header = self.read_exact_bytes(header_len)?;
        let (title, offset) = parse_b2_transfer_header(&header)?;
        if offset != 0 {
            return Err(WinlinkError::Protocol(format!(
                "B2F resume offsets are not implemented yet; CMS sent offset {offset}"
            )));
        }

        let expected_len = usize::try_from(proposal.compressed_byte_count).map_err(|_| {
            WinlinkError::Protocol("B2F compressed byte count exceeds platform size".to_owned())
        })?;
        if expected_len > B2F_MAX_COMPRESSED_BYTES {
            return Err(WinlinkError::Protocol(format!(
                "B2F payload too large: {expected_len} compressed bytes exceeds limit {B2F_MAX_COMPRESSED_BYTES}"
            )));
        }

        let mut compressed = Vec::with_capacity(expected_len);
        let mut checksum = 0_u16;
        loop {
            match self.read_byte()? {
                B2F_STX => {
                    let len = match self.read_byte()? {
                        0 => 256,
                        value => usize::from(value),
                    };
                    if compressed.len() + len > B2F_MAX_COMPRESSED_BYTES {
                        return Err(WinlinkError::Protocol(format!(
                            "B2F payload exceeded limit {B2F_MAX_COMPRESSED_BYTES}"
                        )));
                    }
                    let chunk = self.read_exact_bytes(len)?;
                    for byte in &chunk {
                        checksum = (checksum + u16::from(*byte)) & 0xff;
                    }
                    compressed.extend_from_slice(&chunk);
                }
                B2F_EOT => {
                    let remote_checksum = u16::from(self.read_byte()?);
                    if ((checksum + remote_checksum) & 0xff) != 0 {
                        return Err(WinlinkError::Protocol(
                            "B2F binary payload checksum mismatch".to_owned(),
                        ));
                    }
                    break;
                }
                value => {
                    return Err(WinlinkError::Protocol(format!(
                        "unexpected B2F binary marker 0x{value:02X}"
                    )));
                }
            }
        }

        if compressed.len() != expected_len {
            return Err(WinlinkError::Protocol(format!(
                "B2F compressed length mismatch: expected {expected_len}, got {}",
                compressed.len()
            )));
        }

        Ok(B2Transfer { title, compressed })
    }

    fn write_b2_transfer(&mut self, title: &str, compressed: &[u8]) -> WinlinkResult<()> {
        let transfer = build_b2_transfer(title, compressed)?;
        self.stream.write_all(&transfer)?;
        self.stream.flush()?;
        Ok(())
    }
}

#[derive(Default)]
struct RemoteHandshake {
    sid: Option<String>,
    secure_challenge: Option<String>,
    prompt: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingCmsMessage {
    to: String,
    mid: String,
    _size: u64,
    from: String,
    subject: String,
}

impl PendingCmsMessage {
    fn parse(line: &str) -> Option<Self> {
        let payload = line.strip_prefix(";PM:")?.trim_start();
        let parts = payload.splitn(5, char::is_whitespace).collect::<Vec<_>>();
        if parts.len() != 5 {
            return None;
        }
        Some(Self {
            to: parts[0].to_owned(),
            mid: parts[1].to_owned(),
            _size: parts[2].parse().ok()?,
            from: parts[3].to_owned(),
            subject: parts[4].to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
struct InboundCmsProposal {
    code: String,
    message_type: String,
    mid: String,
    byte_count: u64,
    compressed_byte_count: u64,
}

impl InboundCmsProposal {
    fn parse(line: &str) -> WinlinkResult<Self> {
        let code = line.get(0..2).unwrap_or_default();
        let parts = line[2..]
            .split_whitespace()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if !matches!(code, "FA" | "FB" | "FC" | "FD") || parts.len() < 5 {
            return Err(WinlinkError::InvalidB2fProposal(line.to_owned()));
        }
        let byte_count = parts[2]
            .parse()
            .map_err(|_| WinlinkError::InvalidB2fProposal(line.to_owned()))?;
        let compressed_byte_count = parts[3]
            .parse()
            .map_err(|_| WinlinkError::InvalidB2fProposal(line.to_owned()))?;
        Ok(Self {
            code: code.to_owned(),
            message_type: parts[0].to_owned(),
            mid: parts[1].to_owned(),
            byte_count,
            compressed_byte_count,
        })
    }
}

#[derive(Debug, Clone)]
struct OutboundB2Message {
    id: String,
    title: String,
    proposal_line: String,
    compressed: Vec<u8>,
}

#[derive(Debug, Clone)]
struct B2Transfer {
    title: String,
    compressed: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ParsedB2Message {
    id: String,
    from: String,
    to: Vec<String>,
    subject: String,
    body: String,
    attachments: Vec<ReceivedAttachment>,
}

#[derive(Debug, Clone)]
struct ReceivedAttachment {
    filename: String,
    bytes: Vec<u8>,
}

fn inbound_proposal_answers(store: &WinlinkStore, proposals: &[InboundCmsProposal]) -> String {
    let mut seen = HashSet::new();
    proposals
        .iter()
        .map(|proposal| {
            if !seen.insert(proposal.mid.clone()) || !proposal.is_supported_download() {
                '='
            } else if store.has_complete_inbox_message(&proposal.mid) {
                '-'
            } else {
                '+'
            }
        })
        .collect()
}

impl InboundCmsProposal {
    fn is_supported_download(&self) -> bool {
        matches!(self.code.as_str(), "FC" | "FD")
            && matches!(self.message_type.as_str(), "EM" | "CM")
            && self.compressed_byte_count <= B2F_MAX_COMPRESSED_BYTES as u64
            && self.byte_count <= B2F_MAX_UNCOMPRESSED_BYTES as u64
    }
}

fn send_outbox_messages(
    session: &mut TelnetCmsSession,
    store: &mut WinlinkStore,
) -> WinlinkResult<usize> {
    let outbound = store
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.folder == MailFolder::Outbox)
        .take(5)
        .map(|(index, message)| build_outbound_b2_message(index, message))
        .collect::<WinlinkResult<Vec<_>>>()?;

    if outbound.is_empty() {
        session.write_line("FF")?;
        return Ok(0);
    }

    let proposal_lines = outbound
        .iter()
        .map(|(_, outbound)| outbound.proposal_line.clone())
        .collect::<Vec<_>>();
    for line in &proposal_lines {
        session.write_line(line)?;
    }
    session.write_line(&format!("F> {:02X}", proposal_checksum(&proposal_lines)))?;

    let answers = read_outbound_proposal_answers(session, outbound.len())?;
    let mut sent = 0;
    for ((index, outbound), answer) in outbound.into_iter().zip(answers.bytes()) {
        match answer {
            b'+' | b'Y' | b'y' => {
                session.write_b2_transfer(&outbound.title, &outbound.compressed)?;
                mark_message_sent(store, index, None)?;
                sent += 1;
            }
            b'-' | b'N' | b'n' | b'R' | b'r' => {
                mark_message_sent(store, index, Some("CMS reported message already received"))?;
                sent += 1;
            }
            b'=' | b'L' | b'l' | b'H' | b'h' => {
                mark_message_deferred(store, index, "CMS deferred live send")?;
            }
            value => {
                return Err(WinlinkError::Protocol(format!(
                    "invalid CMS proposal answer byte 0x{value:02X} for {}",
                    outbound.id
                )));
            }
        }
    }
    Ok(sent)
}

fn read_outbound_proposal_answers(
    session: &mut TelnetCmsSession,
    expected: usize,
) -> WinlinkResult<String> {
    loop {
        let line = session.read_line(B2F_LINE_MAX_BYTES)?;
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.starts_with("***") {
            return Err(WinlinkError::Protocol(line));
        }
        let Some(answers) = line.strip_prefix("FS ") else {
            return Err(WinlinkError::Protocol(format!(
                "expected CMS proposal answer, got {line}"
            )));
        };
        let answers = answers.trim().to_owned();
        if answers.len() != expected {
            return Err(WinlinkError::Protocol(format!(
                "CMS answered {} proposals, expected {expected}",
                answers.len()
            )));
        }
        return Ok(answers);
    }
}

fn mark_message_sent(
    store: &mut WinlinkStore,
    index: usize,
    note: Option<&str>,
) -> WinlinkResult<()> {
    let Some(message) = store.messages.get_mut(index) else {
        return Err(WinlinkError::Protocol(format!(
            "internal outbox index disappeared: {index}"
        )));
    };
    message.folder = MailFolder::Sent;
    message.state = MessageState::Sent;
    message.transport = Some(WinlinkTransportKind::Telnet);
    message.last_error = note.map(str::to_owned);
    Ok(())
}

fn mark_message_deferred(store: &mut WinlinkStore, index: usize, note: &str) -> WinlinkResult<()> {
    let Some(message) = store.messages.get_mut(index) else {
        return Err(WinlinkError::Protocol(format!(
            "internal outbox index disappeared: {index}"
        )));
    };
    message.last_error = Some(note.to_owned());
    Ok(())
}

fn build_outbound_b2_message(
    index: usize,
    message: &WinlinkMessage,
) -> WinlinkResult<(usize, OutboundB2Message)> {
    let payload = outbound_b2_payload(message)?;
    if payload.len() > B2F_MAX_UNCOMPRESSED_BYTES {
        return Err(WinlinkError::Protocol(format!(
            "outbound B2F payload too large: {} bytes exceeds limit {B2F_MAX_UNCOMPRESSED_BYTES}",
            payload.len()
        )));
    }
    let compressed = encode_lzhuf_b2_payload(&payload)?;
    if compressed.len() > B2F_MAX_COMPRESSED_BYTES {
        return Err(WinlinkError::Protocol(format!(
            "outbound B2F compressed payload too large: {} bytes exceeds limit {B2F_MAX_COMPRESSED_BYTES}",
            compressed.len()
        )));
    }
    let proposal_line = format!(
        "FC EM {} {} {} 0",
        message.id,
        payload.len(),
        compressed.len()
    );
    Ok((
        index,
        OutboundB2Message {
            id: message.id.clone(),
            title: message.subject.clone(),
            proposal_line,
            compressed,
        },
    ))
}

fn outbound_b2_payload(message: &WinlinkMessage) -> WinlinkResult<Vec<u8>> {
    let mut payload = Vec::new();
    write!(
        &mut payload,
        "Mid: {}\r\nDate: {}\r\nType: Private\r\nFrom: {}\r\n",
        message.id,
        winlink_now_date(),
        b2_wire_address(&message.from)
    )
    .expect("write to vec");
    for to in &message.to {
        write!(&mut payload, "To: {}\r\n", b2_wire_address(to)).expect("write to vec");
    }
    write!(
        &mut payload,
        "Subject: {}\r\nMbo: {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\nBody: {}\r\n",
        message.subject,
        b2_wire_address(&message.from),
        message.body.len()
    )
    .expect("write to vec");

    let mut attachment_bytes = Vec::<Vec<u8>>::new();
    for attachment in &message.attachments {
        let Some(path) = attachment.source_path.as_ref() else {
            return Err(WinlinkError::Protocol(format!(
                "outbound attachment {} has no source path",
                attachment.filename
            )));
        };
        let bytes = fs::read(path)?;
        write!(
            &mut payload,
            "File: {} {}\r\n",
            bytes.len(),
            attachment.filename
        )
        .expect("write to vec");
        attachment_bytes.push(bytes);
    }

    payload.extend_from_slice(b"\r\n");
    payload.extend_from_slice(message.body.as_bytes());
    for bytes in attachment_bytes {
        payload.extend_from_slice(&bytes);
    }
    Ok(payload)
}

fn b2_wire_address(value: &str) -> String {
    let value = value.trim();
    if let Some((local, domain)) = value.split_once('@') {
        if domain.eq_ignore_ascii_case("winlink.org") {
            return local.to_ascii_uppercase();
        }
        return format!("SMTP:{value}");
    }
    value.to_ascii_uppercase()
}

fn winlink_now_date() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    format!("{year:04}/{month:02}/{day:02} {hour:02}:{minute:02}")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let days = days_since_unix_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn upsert_received_message(
    store: &mut WinlinkStore,
    store_path: Option<&Path>,
    parsed: ParsedB2Message,
) -> WinlinkResult<bool> {
    let attachments = save_received_attachments(store_path, &parsed.id, parsed.attachments)?;
    let message = WinlinkMessage {
        id: parsed.id.clone(),
        folder: MailFolder::Inbox,
        state: MessageState::Received,
        from: parsed.from,
        to: parsed.to,
        subject: parsed.subject,
        body: parsed.body,
        attachments,
        transport: Some(WinlinkTransportKind::Telnet),
        last_error: None,
    };

    if let Ok(existing) = store.find_message_mut(&parsed.id) {
        *existing = message;
        return Ok(false);
    }
    store.messages.push(message);
    Ok(true)
}

fn save_received_attachments(
    store_path: Option<&Path>,
    message_id: &str,
    attachments: Vec<ReceivedAttachment>,
) -> WinlinkResult<Vec<WinlinkAttachment>> {
    let base = store_path.and_then(Path::parent).map(|path| {
        path.join("attachments")
            .join(sanitize_path_component(message_id))
    });
    let mut saved = Vec::new();
    for attachment in attachments {
        let byte_count = attachment.bytes.len() as u64;
        let sha256 = sha256_hex(&attachment.bytes);
        let source_path = if let Some(base) = base.as_ref() {
            fs::create_dir_all(base)?;
            let path = base.join(sanitize_path_component(&attachment.filename));
            fs::write(&path, &attachment.bytes)?;
            Some(path)
        } else {
            None
        };
        saved.push(WinlinkAttachment {
            filename: attachment.filename,
            byte_count,
            sha256,
            source_path,
        });
    }
    Ok(saved)
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "attachment.bin".to_owned()
    } else {
        sanitized
    }
}

fn parse_b2_transfer_header(header: &[u8]) -> WinlinkResult<(String, u64)> {
    let mut parts = header.split(|byte| *byte == 0);
    let title = parts
        .next()
        .map(|value| String::from_utf8_lossy(value).trim().to_owned())
        .unwrap_or_default();
    let offset = parts
        .next()
        .ok_or_else(|| WinlinkError::Protocol("B2F transfer header missing offset".to_owned()))
        .and_then(|value| {
            String::from_utf8_lossy(value)
                .trim()
                .parse::<u64>()
                .map_err(|_| WinlinkError::Protocol("invalid B2F transfer offset".to_owned()))
        })?;
    Ok((title, offset))
}

fn decode_b2_payload(
    proposal: &InboundCmsProposal,
    transfer: &B2Transfer,
) -> WinlinkResult<Vec<u8>> {
    let payload = match proposal.code.as_str() {
        "FC" => decode_lzhuf_b2_payload(&transfer.compressed)?,
        "FD" => decode_gzip_b2_payload(&transfer.compressed)?,
        code => {
            return Err(WinlinkError::Protocol(format!(
                "unsupported inbound B2F proposal code {code}"
            )));
        }
    };
    if payload.len() as u64 != proposal.byte_count {
        return Err(WinlinkError::Protocol(format!(
            "B2F uncompressed length mismatch: expected {}, got {}",
            proposal.byte_count,
            payload.len()
        )));
    }
    Ok(payload)
}

fn decode_lzhuf_b2_payload(compressed: &[u8]) -> WinlinkResult<Vec<u8>> {
    if compressed.len() < 6 {
        return Err(WinlinkError::Protocol(
            "B2F LZHUF payload is too short".to_owned(),
        ));
    }
    let expected_crc = u16::from_le_bytes([compressed[0], compressed[1]]);
    let actual_crc = b2_crc16(&compressed[2..]);
    if expected_crc != actual_crc {
        return Err(WinlinkError::Protocol(format!(
            "B2F LZHUF CRC mismatch: expected {expected_crc:04X}, got {actual_crc:04X}"
        )));
    }
    retrocompressor::lzss_huff::expand_slice(
        &compressed[2..],
        &retrocompressor::lzss_huff::STD_OPTIONS,
    )
    .map_err(|error| WinlinkError::Protocol(format!("B2F LZHUF decode failed: {error}")))
}

fn encode_lzhuf_b2_payload(payload: &[u8]) -> WinlinkResult<Vec<u8>> {
    let compressed = retrocompressor::lzss_huff::compress_slice(
        payload,
        &retrocompressor::lzss_huff::STD_OPTIONS,
    )
    .map_err(|error| WinlinkError::Protocol(format!("B2F LZHUF encode failed: {error}")))?;
    let crc = b2_crc16(&compressed);
    let mut out = Vec::with_capacity(compressed.len() + 2);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

fn build_b2_transfer(title: &str, compressed: &[u8]) -> WinlinkResult<Vec<u8>> {
    if compressed.len() > B2F_MAX_COMPRESSED_BYTES {
        return Err(WinlinkError::Protocol(format!(
            "B2F payload too large: {} compressed bytes exceeds limit {B2F_MAX_COMPRESSED_BYTES}",
            compressed.len()
        )));
    }
    let mut header = Vec::new();
    header.extend_from_slice(title.as_bytes());
    header.push(0);
    header.extend_from_slice(b"0");
    header.push(0);
    if header.len() > u8::MAX as usize {
        return Err(WinlinkError::Protocol(
            "B2F transfer title is too long".to_owned(),
        ));
    }

    let mut transfer = Vec::with_capacity(compressed.len() + header.len() + 8);
    transfer.push(B2F_SOH);
    transfer.push(header.len() as u8);
    transfer.extend_from_slice(&header);
    let mut checksum = 0_u16;
    for chunk in compressed.chunks(250) {
        transfer.push(B2F_STX);
        transfer.push(chunk.len() as u8);
        for byte in chunk {
            checksum = (checksum + u16::from(*byte)) & 0xff;
        }
        transfer.extend_from_slice(chunk);
    }
    transfer.push(B2F_EOT);
    transfer.push((0_u16.wrapping_sub(checksum) & 0xff) as u8);
    Ok(transfer)
}

fn decode_gzip_b2_payload(compressed: &[u8]) -> WinlinkResult<Vec<u8>> {
    let mut decoder = GzDecoder::new(Cursor::new(compressed));
    let mut payload = Vec::new();
    decoder
        .read_to_end(&mut payload)
        .map_err(|error| WinlinkError::Protocol(format!("B2F gzip decode failed: {error}")))?;
    Ok(payload)
}

fn parse_b2_message(
    station: &str,
    proposal: &InboundCmsProposal,
    pending: Option<&PendingCmsMessage>,
    transfer_title: &str,
    payload: &[u8],
) -> WinlinkResult<ParsedB2Message> {
    let payload = trim_ascii_left(payload);
    let (header_bytes, remainder) = split_b2_header_body(payload)?;
    let headers = parse_b2_headers(header_bytes)?;
    let body_size = header_first(&headers, "Body")
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(remainder.len());
    if body_size > remainder.len() {
        return Err(WinlinkError::Protocol(format!(
            "B2F body size {body_size} exceeds remaining payload {}",
            remainder.len()
        )));
    }
    let body_bytes = &remainder[..body_size];
    let mut attachment_bytes = &remainder[body_size..];
    let body = decode_b2_body(
        body_bytes,
        header_first(&headers, "Content-Transfer-Encoding"),
    )?;

    let id = header_first(&headers, "Mid")
        .map(str::to_owned)
        .unwrap_or_else(|| proposal.mid.clone());
    let from = header_first(&headers, "From")
        .map(normalize_b2_address)
        .transpose()?
        .or_else(|| {
            pending
                .map(|message| normalize_b2_address(&message.from))
                .transpose()
                .ok()
                .flatten()
        })
        .unwrap_or_else(|| "UNKNOWN@winlink.org".to_owned());
    let mut to = header_values(&headers, "To")
        .into_iter()
        .map(normalize_b2_address)
        .collect::<WinlinkResult<Vec<_>>>()?;
    for cc in header_values(&headers, "Cc") {
        to.push(normalize_b2_address(cc)?);
    }
    if to.is_empty() {
        if let Some(message) = pending {
            to.push(normalize_b2_address(&message.to)?);
        } else {
            to.push(format!("{station}@winlink.org"));
        }
    }
    let subject = header_first(&headers, "Subject")
        .map(str::to_owned)
        .or_else(|| pending.map(|message| message.subject.clone()))
        .or_else(|| {
            if transfer_title.is_empty() {
                None
            } else {
                Some(transfer_title.to_owned())
            }
        })
        .unwrap_or_else(|| format!("Winlink message {}", proposal.mid));

    let mut attachments = Vec::new();
    for file_header in header_values(&headers, "File") {
        let (size, filename) = parse_b2_file_header(file_header)?;
        if size > attachment_bytes.len() {
            return Err(WinlinkError::Protocol(format!(
                "B2F attachment {filename} size {size} exceeds remaining payload {}",
                attachment_bytes.len()
            )));
        }
        let bytes = attachment_bytes[..size].to_vec();
        attachment_bytes = &attachment_bytes[size..];
        attachments.push(ReceivedAttachment { filename, bytes });
    }

    Ok(ParsedB2Message {
        id,
        from,
        to,
        subject,
        body,
        attachments,
    })
}

fn trim_ascii_left(mut bytes: &[u8]) -> &[u8] {
    while let Some((first, rest)) = bytes.split_first() {
        if first.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

fn split_b2_header_body(payload: &[u8]) -> WinlinkResult<(&[u8], &[u8])> {
    if let Some(index) = payload.windows(4).position(|window| window == b"\r\n\r\n") {
        return Ok((&payload[..index], &payload[index + 4..]));
    }
    if let Some(index) = payload.windows(2).position(|window| window == b"\n\n") {
        return Ok((&payload[..index], &payload[index + 2..]));
    }
    Err(WinlinkError::Protocol(
        "B2F message payload is missing a header/body separator".to_owned(),
    ))
}

fn parse_b2_headers(header_bytes: &[u8]) -> WinlinkResult<Vec<(String, String)>> {
    let text = String::from_utf8_lossy(header_bytes);
    let mut headers = Vec::<(String, String)>::new();
    for raw_line in text.replace("\r\n", "\n").split('\n') {
        if raw_line.trim().is_empty() {
            continue;
        }
        if raw_line.starts_with(' ') || raw_line.starts_with('\t') {
            let Some((_, value)) = headers.last_mut() else {
                return Err(WinlinkError::Protocol(
                    "B2F folded header appears before any header".to_owned(),
                ));
            };
            value.push(' ');
            value.push_str(raw_line.trim());
            continue;
        }
        let Some((name, value)) = raw_line.split_once(':') else {
            return Err(WinlinkError::Protocol(format!(
                "invalid B2F message header: {raw_line}"
            )));
        };
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    Ok(headers)
}

fn header_values<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .collect()
}

fn header_first<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn decode_b2_body(body: &[u8], transfer_encoding: Option<&str>) -> WinlinkResult<String> {
    let decoded = match transfer_encoding
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "base64" => {
            let compact = body
                .iter()
                .copied()
                .filter(|byte| !byte.is_ascii_whitespace())
                .collect::<Vec<_>>();
            base64::engine::general_purpose::STANDARD
                .decode(compact)
                .map_err(|error| {
                    WinlinkError::Protocol(format!("B2F base64 body failed: {error}"))
                })?
        }
        "quoted-printable" => quoted_printable::decode(body, quoted_printable::ParseMode::Robust)
            .map_err(|error| {
            WinlinkError::Protocol(format!("B2F quoted-printable body failed: {error}"))
        })?,
        _ => body.to_vec(),
    };
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

fn parse_b2_file_header(value: &str) -> WinlinkResult<(usize, String)> {
    let Some((size, filename)) = value.trim().split_once(char::is_whitespace) else {
        return Err(WinlinkError::Protocol(format!(
            "invalid B2F File header: {value}"
        )));
    };
    let size = size
        .parse::<usize>()
        .map_err(|_| WinlinkError::Protocol(format!("invalid B2F File size: {value}")))?;
    let filename = filename.trim();
    if filename.is_empty() {
        return Err(WinlinkError::Protocol(
            "B2F File header has empty filename".to_owned(),
        ));
    }
    Ok((size, filename.to_owned()))
}

fn normalize_b2_address(value: &str) -> WinlinkResult<String> {
    let value = value.trim();
    let value = value
        .strip_prefix("SMTP:")
        .or_else(|| value.strip_prefix("smtp:"))
        .unwrap_or(value);
    normalize_recipient(value)
}

fn b2_crc16(payload: &[u8]) -> u16 {
    let mut sum = 0_u16;
    for byte in payload.iter().copied().chain([0, 0]) {
        let table = crc16_ccitt_table_value((sum >> 8) as u8);
        sum = ((sum << 8) & 0xff00) ^ table ^ u16::from(byte);
    }
    sum
}

fn crc16_ccitt_table_value(index: u8) -> u16 {
    let mut value = u16::from(index) << 8;
    for _ in 0..8 {
        if (value & 0x8000) != 0 {
            value = (value << 1) ^ 0x1021;
        } else {
            value <<= 1;
        }
    }
    value
}

fn local_sid_line() -> String {
    format!("[chattybara-{}-B2FHM$]", env!("CARGO_PKG_VERSION"))
}

fn parse_secure_challenge(line: &str) -> Option<String> {
    let value = line.strip_prefix(";PQ")?;
    Some(
        value
            .trim_start_matches(':')
            .split_whitespace()
            .next()?
            .to_owned(),
    )
}

fn secure_login_response(challenge: &str, password: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(challenge.as_bytes());
    hasher.update(password.as_bytes());
    hasher.update(WINLINK_SECURE_SALT);
    let digest = hasher.finalize();
    let mut response = u32::from(digest[3] & 0x3f);
    for index in (0..=2).rev() {
        response = (response << 8) | u32::from(digest[index]);
    }
    let token = format!("{response:08}");
    token[token.len().saturating_sub(8)..].to_owned()
}

fn verify_proposal_checksum(lines: &[String], checksum_line: &str) -> WinlinkResult<()> {
    let Some(value) = checksum_line.strip_prefix("F>") else {
        return Err(WinlinkError::Protocol(format!(
            "invalid proposal checksum line: {checksum_line}"
        )));
    };
    let actual = u8::from_str_radix(value.trim(), 16)
        .map_err(|_| WinlinkError::Protocol(format!("invalid B2F checksum: {checksum_line}")))?;
    let expected = proposal_checksum(lines);
    if actual != expected {
        return Err(WinlinkError::Protocol(format!(
            "B2F proposal checksum mismatch: expected {expected:02X}, got {actual:02X}"
        )));
    }
    Ok(())
}

fn proposal_checksum(lines: &[String]) -> u8 {
    let mut sum = 0_i64;
    for line in lines {
        for byte in line.bytes() {
            sum += i64::from(byte);
        }
        sum += i64::from(b'\r');
    }
    ((-sum) & 0xff) as u8
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

pub fn default_store_path(station: impl AsRef<str>) -> WinlinkResult<PathBuf> {
    let station = normalize_call(station.as_ref())?;
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join("chattybara")
            })
        })
        .unwrap_or_else(|| PathBuf::from(".").join(".chattybara"));
    Ok(base.join("winlink").join(station).join("store.json"))
}

pub fn normalize_call(value: &str) -> WinlinkResult<String> {
    let call = value.trim().to_ascii_uppercase();
    if call.is_empty() {
        return Err(WinlinkError::EmptyStation);
    }
    Ok(call)
}

fn normalize_recipient(value: &str) -> WinlinkResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(WinlinkError::EmptyRecipient);
    }
    if let Some((local, domain)) = trimmed.split_once('@') {
        let local = local.trim();
        let domain = domain.trim();
        if local.is_empty() || domain.is_empty() {
            return Err(WinlinkError::EmptyRecipient);
        }
        Ok(format!(
            "{}@{}",
            local.to_ascii_uppercase(),
            domain.to_ascii_lowercase()
        ))
    } else {
        Ok(format!("{}@winlink.org", trimmed.to_ascii_uppercase()))
    }
}

enum TextField {
    Subject,
    Body,
}

fn require_text(value: &str, field: TextField) -> WinlinkResult<String> {
    let value = value.trim();
    if value.is_empty() {
        match field {
            TextField::Subject => Err(WinlinkError::EmptySubject),
            TextField::Body => Err(WinlinkError::EmptyBody),
        }
    } else {
        Ok(value.to_owned())
    }
}

fn fake_inbox_id(station: &str) -> String {
    format!("FAKE-{station}-001")
}

fn fake_inbox_message(station: &str) -> WinlinkMessage {
    WinlinkMessage {
        id: fake_inbox_id(station),
        folder: MailFolder::Inbox,
        state: MessageState::Received,
        from: "JA1QSO@winlink.org".to_owned(),
        to: vec![format!("{station}@winlink.org")],
        subject: "Fake Winlink CMS check".to_owned(),
        body: "This deterministic message was generated by chattybara fake Winlink sync."
            .to_owned(),
        attachments: Vec::new(),
        transport: Some(WinlinkTransportKind::Fake),
        last_error: None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("write hex");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn store_queues_and_persists_messages() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        let mut store = WinlinkStore::new("ja1tst").expect("store");
        store.set_account(WinlinkAccount::new("ja1tst", CredentialSource::Env).expect("account"));
        let id = store
            .queue_message(vec!["ja1qso".to_owned()], "Subject", "Body", Vec::new())
            .expect("queue");
        store.save(&path).expect("save");

        let loaded = WinlinkStore::load_or_new(&path, "JA1TST").expect("load");
        assert_eq!(loaded.station, "JA1TST");
        assert_eq!(
            loaded.account.as_ref().expect("account").address,
            "JA1TST@winlink.org"
        );
        let message = loaded.find_message(&id).expect("message");
        assert_eq!(message.to, vec!["JA1QSO@winlink.org"]);
        assert_eq!(message.folder, MailFolder::Outbox);
    }

    #[test]
    fn fake_sync_receives_inbox_and_sends_outbox() {
        let mut store = WinlinkStore::new("ja1tst").expect("store");
        store
            .queue_message(
                vec!["ja1qso@winlink.org".to_owned()],
                "Subject",
                "Body",
                Vec::new(),
            )
            .expect("queue");

        let report = fake_sync(&mut store, None).expect("sync");
        assert_eq!(report.inbox_received, 1);
        assert_eq!(report.outbox_sent, 1);
        assert_eq!(report.queued_remaining, 0);
        assert_eq!(store.messages_in(MailFolder::Inbox).len(), 1);
        assert_eq!(store.messages_in(MailFolder::Sent).len(), 1);

        let report = fake_sync(&mut store, None).expect("sync again");
        assert_eq!(report.inbox_received, 0);
        assert_eq!(report.outbox_sent, 0);
    }

    #[test]
    fn b2f_proposal_roundtrips() {
        let mut store = WinlinkStore::new("ja1tst").expect("store");
        let id = store
            .queue_message(vec!["ja1qso".to_owned()], "B2F subject", "Body", Vec::new())
            .expect("queue");
        let message = store.find_message(&id).expect("message");
        let proposal = B2fProposal::from_message(message);
        let parsed = B2fProposal::parse(&proposal.to_line()).expect("parse");
        assert_eq!(parsed, proposal);
    }

    #[test]
    fn telnet_check_is_dry_run_unless_live() {
        let report = telnet_cms_check(TelnetCmsConfig {
            station: "ja1tst".to_owned(),
            host: "example.invalid".to_owned(),
            port: DEFAULT_CMS_PORT,
            timeout_ms: 1,
            live: false,
        })
        .expect("report");
        assert!(report.ok);
        assert!(report.dry_run);
        assert!(!report.connected);
    }

    #[test]
    fn telnet_check_can_connect_to_fake_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.write_all(b"Fake CMS ready\r\n").expect("write");
        });

        let report = telnet_cms_check(TelnetCmsConfig {
            station: "ja1tst".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: address.port(),
            timeout_ms: 1000,
            live: true,
        })
        .expect("report");
        handle.join().expect("join");

        assert!(report.ok);
        assert!(!report.dry_run);
        assert!(report.connected);
        assert_eq!(report.greeting.as_deref(), Some("Fake CMS ready"));
    }

    #[test]
    fn secure_login_response_matches_known_vector() {
        assert_eq!(secure_login_response("23753528", "FooBar"), "95074758");
    }

    #[test]
    fn telnet_receive_sync_lists_and_defers_fake_cms_inbox() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let read_stream = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(read_stream);

            stream.write_all(b"Callsign :\r").expect("write callsign");
            assert_eq!(read_cr_line(&mut reader), "JA1TST");
            stream.write_all(b"Password :\r").expect("write password");
            assert_eq!(read_cr_line(&mut reader), CMS_TELNET_PASSWORD);
            stream
                .write_all(b"[WL2K-5.0-B2FHM$]\r;PQ: 23753528\rCMS>\r")
                .expect("write handshake");
            assert_eq!(read_cr_line(&mut reader), ";FW JA1TST");
            assert!(
                read_cr_line(&mut reader).starts_with("[chattybara-"),
                "local SID"
            );
            assert_eq!(read_cr_line(&mut reader), ";PR: 95074758");
            assert_eq!(read_cr_line(&mut reader), "FF");

            let body = b"Downloaded body from fake CMS.";
            let payload =
                b2_fixture_message("TESTMID123", "JA1QSO", "JA1TST", "Test subject", body, &[]);
            let compressed = b2_lzhuf_fixture_payload(&payload);
            let transfer = b2_transfer_fixture("Test subject", &compressed);
            let proposal = format!("FC EM TESTMID123 {} {} 0", payload.len(), compressed.len());
            let checksum = proposal_checksum(std::slice::from_ref(&proposal));
            stream
                .write_all(
                    format!(
                        ";PM: JA1TST TESTMID123 {} JA1QSO Test subject\r{proposal}\rF> {checksum:02X}\r",
                        payload.len()
                    )
                    .as_bytes(),
                )
                .expect("write proposals");
            assert_eq!(read_cr_line(&mut reader), "FS +");
            stream.write_all(&transfer).expect("write transfer");
            assert_eq!(read_cr_line(&mut reader), "FQ");
        });

        let mut store = WinlinkStore::new("ja1tst").expect("store");
        let report = telnet_cms_receive_sync(
            &mut store,
            None,
            TelnetCmsConfig {
                station: "ja1tst".to_owned(),
                host: "127.0.0.1".to_owned(),
                port: address.port(),
                timeout_ms: 1000,
                live: true,
            },
            Some("FooBar"),
            false,
        )
        .expect("sync");
        handle.join().expect("join");

        assert!(report.ok);
        assert_eq!(report.inbox_received, 1);
        assert_eq!(report.queued_remaining, 0);
        let message = store.find_message("TESTMID123").expect("message");
        assert_eq!(message.folder, MailFolder::Inbox);
        assert_eq!(message.from, "JA1QSO@winlink.org");
        assert_eq!(message.to, vec!["JA1TST@winlink.org"]);
        assert_eq!(message.subject, "Test subject");
        assert_eq!(message.body, "Downloaded body from fake CMS.");
        assert_eq!(message.last_error.as_deref(), None);
    }

    #[test]
    fn b2_parser_saves_attachments() {
        let attachment = ("note.txt", b"attached text".as_slice());
        let payload = b2_fixture_message(
            "ATTACHMID1",
            "JA1QSO",
            "JA1TST",
            "Attachment subject",
            b"Body with file.",
            &[attachment],
        );
        let proposal = InboundCmsProposal {
            code: "FC".to_owned(),
            message_type: "EM".to_owned(),
            mid: "ATTACHMID1".to_owned(),
            byte_count: payload.len() as u64,
            compressed_byte_count: 0,
        };
        let parsed = parse_b2_message("JA1TST", &proposal, None, "Attachment subject", &payload)
            .expect("parse");
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].filename, "note.txt");
        assert_eq!(parsed.attachments[0].bytes, b"attached text");
    }

    #[test]
    fn telnet_sync_sends_fake_cms_outbox_when_allowed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let read_stream = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(read_stream);

            stream.write_all(b"Callsign :\r").expect("write callsign");
            assert_eq!(read_cr_line(&mut reader), "JA1TST");
            stream.write_all(b"Password :\r").expect("write password");
            assert_eq!(read_cr_line(&mut reader), CMS_TELNET_PASSWORD);
            stream
                .write_all(b"[WL2K-5.0-B2FHM$]\r;PQ: 23753528\rCMS>\r")
                .expect("write handshake");
            assert_eq!(read_cr_line(&mut reader), ";FW JA1TST");
            assert!(
                read_cr_line(&mut reader).starts_with("[chattybara-"),
                "local SID"
            );
            assert_eq!(read_cr_line(&mut reader), ";PR: 95074758");

            let proposal = read_cr_line(&mut reader);
            assert!(proposal.starts_with("FC EM JA1TST-0001 "), "{proposal}");
            let checksum_line = read_cr_line(&mut reader);
            assert_eq!(
                checksum_line,
                format!(
                    "F> {:02X}",
                    proposal_checksum(std::slice::from_ref(&proposal))
                )
            );
            stream.write_all(b"FS +\r").expect("write answer");
            let transfer = read_b2_transfer_fixture(&mut reader);
            assert!(!transfer.is_empty());
            stream.write_all(b"FF\r").expect("write no inbound");
            assert_eq!(read_cr_line(&mut reader), "FQ");
        });

        let mut store = WinlinkStore::new("ja1tst").expect("store");
        store
            .queue_message(
                vec!["ja1qso".to_owned()],
                "Outbound subject",
                "Outbound body",
                Vec::new(),
            )
            .expect("queue");
        let report = telnet_cms_receive_sync(
            &mut store,
            None,
            TelnetCmsConfig {
                station: "ja1tst".to_owned(),
                host: "127.0.0.1".to_owned(),
                port: address.port(),
                timeout_ms: 1000,
                live: true,
            },
            Some("FooBar"),
            true,
        )
        .expect("sync");
        handle.join().expect("join");

        assert_eq!(report.outbox_sent, 1);
        assert_eq!(report.queued_remaining, 0);
        let sent = store.find_message("JA1TST-0001").expect("sent");
        assert_eq!(sent.folder, MailFolder::Sent);
        assert_eq!(sent.state, MessageState::Sent);
    }

    fn b2_fixture_message(
        mid: &str,
        from: &str,
        to: &str,
        subject: &str,
        body: &[u8],
        attachments: &[(&str, &[u8])],
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        write!(
            &mut payload,
            "Mid: {mid}\r\nDate: 2026/05/13 10:00\r\nType: Private\r\nFrom: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nMbo: {from}\r\nBody: {}\r\n",
            body.len()
        )
        .expect("write headers");
        for (filename, bytes) in attachments {
            write!(&mut payload, "File: {} {}\r\n", bytes.len(), filename).expect("write file");
        }
        payload.extend_from_slice(b"\r\n");
        payload.extend_from_slice(body);
        for (_, bytes) in attachments {
            payload.extend_from_slice(bytes);
        }
        payload
    }

    fn b2_lzhuf_fixture_payload(payload: &[u8]) -> Vec<u8> {
        let compressed = retrocompressor::lzss_huff::compress_slice(
            payload,
            &retrocompressor::lzss_huff::STD_OPTIONS,
        )
        .expect("compress");
        let crc = b2_crc16(&compressed);
        let mut out = Vec::with_capacity(compressed.len() + 2);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&compressed);
        out
    }

    fn b2_transfer_fixture(title: &str, compressed: &[u8]) -> Vec<u8> {
        let mut transfer = Vec::new();
        let mut header = Vec::new();
        header.extend_from_slice(title.as_bytes());
        header.push(0);
        header.extend_from_slice(b"0");
        header.push(0);
        transfer.push(B2F_SOH);
        transfer.push(header.len() as u8);
        transfer.extend_from_slice(&header);
        let mut checksum = 0_u16;
        for chunk in compressed.chunks(250) {
            transfer.push(B2F_STX);
            transfer.push(chunk.len() as u8);
            for byte in chunk {
                checksum = (checksum + u16::from(*byte)) & 0xff;
            }
            transfer.extend_from_slice(chunk);
        }
        transfer.push(B2F_EOT);
        transfer.push((0_u16.wrapping_sub(checksum) & 0xff) as u8);
        transfer
    }

    fn read_b2_transfer_fixture(reader: &mut BufReader<TcpStream>) -> Vec<u8> {
        let mut marker = [0_u8; 1];
        reader.read_exact(&mut marker).expect("read soh");
        assert_eq!(marker[0], B2F_SOH);
        reader.read_exact(&mut marker).expect("read header len");
        let mut header = vec![0_u8; marker[0] as usize];
        reader.read_exact(&mut header).expect("read header");
        let mut compressed = Vec::new();
        loop {
            reader.read_exact(&mut marker).expect("read marker");
            match marker[0] {
                B2F_STX => {
                    reader.read_exact(&mut marker).expect("read chunk len");
                    let len = if marker[0] == 0 {
                        256
                    } else {
                        marker[0] as usize
                    };
                    let mut chunk = vec![0_u8; len];
                    reader.read_exact(&mut chunk).expect("read chunk");
                    compressed.extend_from_slice(&chunk);
                }
                B2F_EOT => {
                    reader.read_exact(&mut marker).expect("read checksum");
                    break;
                }
                other => panic!("unexpected marker {other}"),
            }
        }
        compressed
    }

    #[test]
    fn non_fake_live_sync_is_guarded() {
        let mut store = WinlinkStore::new("ja1tst").expect("store");
        store
            .queue_message(vec!["ja1qso".to_owned()], "Subject", "Body", Vec::new())
            .expect("queue");

        let error =
            guarded_dry_run_sync_report(&store, None, WinlinkTransportKind::Vara, true, false)
                .expect_err("guarded");
        assert!(error.to_string().contains("requires --allow-send"));

        let error =
            guarded_dry_run_sync_report(&store, None, WinlinkTransportKind::Vara, true, true)
                .expect_err("not implemented");
        assert!(error.to_string().contains("not implemented"));
    }

    fn read_cr_line(reader: &mut BufReader<TcpStream>) -> String {
        let mut bytes = Vec::new();
        reader.read_until(b'\r', &mut bytes).expect("read line");
        while matches!(bytes.last(), Some(b'\r' | b'\n')) {
            bytes.pop();
        }
        String::from_utf8(bytes).expect("utf8")
    }
}

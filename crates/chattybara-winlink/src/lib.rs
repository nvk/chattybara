//! Transport-neutral Winlink mailbox primitives.
//!
//! This crate intentionally separates mailbox state from transport mechanics.
//! The fake transport is complete enough for deterministic tests. Telnet, VARA,
//! and orca reports are guarded scaffolds that share the same store and safety
//! model while the full session protocols are built out.

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

pub const DEFAULT_CMS_HOST: &str = "server.winlink.org";
pub const DEFAULT_CMS_PORT: u16 = 8772;
pub const DEFAULT_TELNET_TIMEOUT_MS: u64 = 3000;
pub const WINLINK_PASSWORD_ENV: &str = "CHATTYBARA_WINLINK_PASSWORD";
const CMS_TELNET_PASSWORD: &str = "CMSTelnet";
const TELNET_PROMPT_MAX_BYTES: usize = 4096;
const B2F_LINE_MAX_BYTES: usize = 8192;
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

    pub fn has_message(&self, id: &str) -> bool {
        self.messages.iter().any(|message| message.id == id)
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
                "B2F authentication and message exchange are not enabled in this alpha".to_owned(),
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
            "full Telnet/CMS B2F sync remains guarded behind future implementation".to_owned(),
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
    if let Some(challenge) = handshake.secure_challenge.as_deref() {
        let password = password.ok_or(WinlinkError::MissingPasswordEnv(WINLINK_PASSWORD_ENV))?;
        let response = secure_login_response(challenge, password);
        session.write_line(&format!(";FW: {station}"))?;
        session.write_line(&local_sid_line())?;
        session.write_line(&format!(";PR: {response}"))?;
    } else {
        session.write_line(&format!(";FW: {station}"))?;
        session.write_line(&local_sid_line())?;
    }
    session.write_line(&format!("; wl2k DE {station} ()"))?;

    let mut pending = std::collections::HashMap::<String, PendingCmsMessage>::new();
    let mut proposals = Vec::<InboundCmsProposal>::new();
    let mut proposal_lines = Vec::<String>::new();
    let mut added = 0;
    let mut notes = vec![
        "live Telnet/CMS authenticated; inbound proposals are listed and deferred".to_owned(),
        "message bodies are not downloaded in this build, so CMS inbox contents remain pending"
            .to_owned(),
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
                let answers = "=".repeat(proposals.len());
                session.write_line(&format!("FS {answers}"))?;
                for proposal in proposals.drain(..) {
                    if add_metadata_message(store, &station, &proposal, pending.get(&proposal.mid))?
                    {
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
        outbox_sent: 0,
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
        self.stream.write_all(line.as_bytes())?;
        self.stream.write_all(b"\r")?;
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
        while buffer.len() < max_bytes {
            match self.stream.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    buffer.push(byte[0]);
                    if patterns
                        .iter()
                        .any(|pattern| contains_ascii_case_insensitive(&buffer, pattern))
                    {
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
    size: u64,
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
            size: parts[2].parse().ok()?,
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

fn add_metadata_message(
    store: &mut WinlinkStore,
    station: &str,
    proposal: &InboundCmsProposal,
    pending: Option<&PendingCmsMessage>,
) -> WinlinkResult<bool> {
    if store.has_message(&proposal.mid) {
        return Ok(false);
    }
    let from = pending
        .map(|message| normalize_recipient(&message.from))
        .transpose()?
        .unwrap_or_else(|| "UNKNOWN@winlink.org".to_owned());
    let to = pending
        .map(|message| normalize_recipient(&message.to))
        .transpose()?
        .unwrap_or_else(|| format!("{station}@winlink.org"));
    let subject = pending
        .map(|message| message.subject.clone())
        .unwrap_or_else(|| format!("Winlink message {}", proposal.mid));
    let reported_size = pending
        .map(|message| message.size)
        .unwrap_or(proposal.byte_count);
    let body = format!(
        "Winlink CMS reports this message is pending.\n\nMessage ID: {}\nFrom: {}\nTo: {}\nSubject: {}\nProposal: {} {}\nUncompressed bytes: {}\nCompressed bytes: {}\n\nchattybara deferred the payload during live Telnet/CMS sync, so the message remains pending on the CMS for a later full B2F download.",
        proposal.mid,
        from,
        to,
        subject,
        proposal.code,
        proposal.message_type,
        reported_size,
        proposal.compressed_byte_count
    );
    store.messages.push(WinlinkMessage {
        id: proposal.mid.clone(),
        folder: MailFolder::Inbox,
        state: MessageState::Received,
        from,
        to: vec![to],
        subject,
        body,
        attachments: Vec::new(),
        transport: Some(WinlinkTransportKind::Telnet),
        last_error: Some("payload deferred; body not downloaded".to_owned()),
    });
    Ok(true)
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

            stream.write_all(b"Callsign :").expect("write callsign");
            assert_eq!(read_cr_line(&mut reader), "JA1TST");
            stream.write_all(b"Password :").expect("write password");
            assert_eq!(read_cr_line(&mut reader), CMS_TELNET_PASSWORD);
            stream
                .write_all(b"[WL2K-5.0-B2FHM$]\r;PQ: 23753528\rCMS>\r")
                .expect("write handshake");
            assert_eq!(read_cr_line(&mut reader), ";FW: JA1TST");
            assert!(
                read_cr_line(&mut reader).starts_with("[chattybara-"),
                "local SID"
            );
            assert_eq!(read_cr_line(&mut reader), ";PR: 95074758");
            assert_eq!(read_cr_line(&mut reader), "; wl2k DE JA1TST ()");

            let proposal = "FC EM TESTMID123 128 64 0".to_owned();
            let checksum = proposal_checksum(std::slice::from_ref(&proposal));
            stream
                .write_all(
                    format!(
                        ";PM: JA1TST TESTMID123 128 JA1QSO Test subject\r{proposal}\rF> {checksum:02X}\r"
                    )
                    .as_bytes(),
                )
                .expect("write proposals");
            assert_eq!(read_cr_line(&mut reader), "FS =");
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
        assert!(message.body.contains("payload during live Telnet/CMS sync"));
        assert_eq!(
            message.last_error.as_deref(),
            Some("payload deferred; body not downloaded")
        );
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

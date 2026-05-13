//! Transport-neutral Winlink mailbox primitives.
//!
//! This crate intentionally separates mailbox state from transport mechanics.
//! The fake transport is complete enough for deterministic tests. Telnet, VARA,
//! and orca reports are guarded scaffolds that share the same store and safety
//! model while the full session protocols are built out.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn store_queues_and_persists_messages() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        let mut store = WinlinkStore::new("ve3tst").expect("store");
        store.set_account(WinlinkAccount::new("ve3tst", CredentialSource::Env).expect("account"));
        let id = store
            .queue_message(vec!["ja1qso".to_owned()], "Subject", "Body", Vec::new())
            .expect("queue");
        store.save(&path).expect("save");

        let loaded = WinlinkStore::load_or_new(&path, "VE3TST").expect("load");
        assert_eq!(loaded.station, "VE3TST");
        assert_eq!(
            loaded.account.as_ref().expect("account").address,
            "VE3TST@winlink.org"
        );
        let message = loaded.find_message(&id).expect("message");
        assert_eq!(message.to, vec!["JA1QSO@winlink.org"]);
        assert_eq!(message.folder, MailFolder::Outbox);
    }

    #[test]
    fn fake_sync_receives_inbox_and_sends_outbox() {
        let mut store = WinlinkStore::new("ve3tst").expect("store");
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
        let mut store = WinlinkStore::new("ve3tst").expect("store");
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
            station: "ve3tst".to_owned(),
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
            station: "ve3tst".to_owned(),
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
    fn non_fake_live_sync_is_guarded() {
        let mut store = WinlinkStore::new("ve3tst").expect("store");
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
}

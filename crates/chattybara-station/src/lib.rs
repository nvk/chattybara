//! Station-level state, events, mode capabilities, and safety guards.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StationProfile {
    pub call_sign: String,
    pub grid: Option<String>,
    pub operator_label: Option<String>,
    pub default_log_path: Option<PathBuf>,
}

impl StationProfile {
    pub fn new(call_sign: impl AsRef<str>) -> StationResult<Self> {
        let call_sign = normalize_call(call_sign.as_ref())?;
        Ok(Self {
            call_sign,
            grid: None,
            operator_label: None,
            default_log_path: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceId {
    Chat,
    WeakSignal,
    CwAssist,
    Spots,
    OperatorConsole,
    RigSetup,
}

impl WorkspaceId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::WeakSignal => "weak-signal",
            Self::CwAssist => "cw-assist",
            Self::Spots => "spots",
            Self::OperatorConsole => "operator-console",
            Self::RigSetup => "rig-setup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModeId {
    OrcaChat,
    Js8callExternal,
    WsjtxExternal,
    FldigiExternal,
    CwAssist,
    PskReporter,
}

impl ModeId {
    pub fn label(self) -> &'static str {
        match self {
            Self::OrcaChat => "orca-chat",
            Self::Js8callExternal => "js8call-external",
            Self::WsjtxExternal => "wsjtx-external",
            Self::FldigiExternal => "fldigi-external",
            Self::CwAssist => "cw-assist",
            Self::PskReporter => "pskreporter",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "orca-chat" | "chat" => Some(Self::OrcaChat),
            "js8call-external" | "js8call" => Some(Self::Js8callExternal),
            "wsjtx-external" | "wsjtx" | "ft8" => Some(Self::WsjtxExternal),
            "fldigi-external" | "fldigi" => Some(Self::FldigiExternal),
            "cw-assist" | "cw" | "morse" => Some(Self::CwAssist),
            "pskreporter" | "psk-reporter" | "spots" => Some(Self::PskReporter),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModeCapabilities {
    pub free_text: bool,
    pub directed_message: bool,
    pub conversation: bool,
    pub arq: bool,
    pub file_transfer: bool,
    pub mailbox: bool,
    pub store_forward: bool,
    pub fixed_time_slots: bool,
    pub decode_table: bool,
    pub spot_reporting: bool,
    pub logging: bool,
    pub external_app_api: bool,
    pub native_modem: bool,
    pub rx_only: bool,
    pub requires_time_sync: bool,
    pub tx_capable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeDescriptor {
    pub id: ModeId,
    pub label: String,
    pub workspace: WorkspaceId,
    pub capabilities: ModeCapabilities,
    pub implementation: ModeImplementation,
    pub status: ModeStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModeImplementation {
    Native,
    ExternalApp,
    FakeFixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModeStatus {
    Working,
    Scaffold,
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StationSafetyState {
    pub live: bool,
    pub tx_armed: bool,
    pub ptt_keyed: bool,
    pub reporting_enabled: bool,
}

impl StationSafetyState {
    pub fn label(self) -> &'static str {
        match (self.live, self.tx_armed, self.ptt_keyed) {
            (_, _, true) => "PTT KEYED",
            (true, true, false) => "LIVE TX ARMED",
            (true, false, false) => "LIVE RX ONLY",
            (false, _, false) => "DRY RUN",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StationEvent {
    RigStatus(RigStatusEvent),
    AudioStatus(AudioStatusEvent),
    PttChanged(PttChangedEvent),
    Decode(DecodeEvent),
    DirectedMessage(DirectedMessageEvent),
    ChatMessage(ChatMessageEvent),
    Spot(SpotEvent),
    InboxMessage(InboxMessageEvent),
    MailMessage(MailMessageEvent),
    FileOffer(FileOfferEvent),
    FileTransferProgress(FileTransferEvent),
    QsoLogged(QsoLogEvent),
    AdapterHealth(AdapterHealthEvent),
    ModeError(ModeErrorEvent),
}

impl StationEvent {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::RigStatus(_) => "rig-status",
            Self::AudioStatus(_) => "audio-status",
            Self::PttChanged(_) => "ptt-changed",
            Self::Decode(_) => "decode",
            Self::DirectedMessage(_) => "directed-message",
            Self::ChatMessage(_) => "chat-message",
            Self::Spot(_) => "spot",
            Self::InboxMessage(_) => "inbox-message",
            Self::MailMessage(_) => "mail-message",
            Self::FileOffer(_) => "file-offer",
            Self::FileTransferProgress(_) => "file-transfer-progress",
            Self::QsoLogged(_) => "qso-logged",
            Self::AdapterHealth(_) => "adapter-health",
            Self::ModeError(_) => "mode-error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StationAction {
    SetMode {
        mode: ModeId,
    },
    Tune {
        frequency_hz: u64,
    },
    Connect {
        call_sign: String,
    },
    Listen,
    SendMessage {
        to: String,
        text: String,
    },
    Reply {
        target_id: String,
        text: String,
    },
    QueueMail {
        to: String,
        subject: String,
        body: String,
    },
    AcceptFile {
        offer_id: String,
    },
    LogQso {
        call_sign: String,
        mode: String,
    },
    ReportSpot {
        call_sign: String,
        frequency_hz: u64,
        mode: String,
    },
    ArmTransmit,
    AbortTransmit,
    ChangeWorkspace {
        workspace: WorkspaceId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RigStatusEvent {
    pub mode: ModeId,
    pub frequency_hz: Option<u64>,
    pub radio: Option<String>,
    pub ptt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStatusEvent {
    pub mode: ModeId,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PttChangedEvent {
    pub keyed: bool,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeEvent {
    pub mode: ModeId,
    pub from: Option<String>,
    pub text: String,
    pub snr_db: Option<i16>,
    pub frequency_hz: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectedMessageEvent {
    pub mode: ModeId,
    pub from: String,
    pub to: String,
    pub text: String,
    pub snr_db: Option<i16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessageEvent {
    pub mode: ModeId,
    pub sequence: u64,
    pub from: String,
    pub to: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpotEvent {
    pub mode: ModeId,
    pub call_sign: String,
    pub frequency_hz: u64,
    pub snr_db: Option<i16>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxMessageEvent {
    pub mode: ModeId,
    pub message_id: String,
    pub from: String,
    pub to: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailMessageEvent {
    pub mode: ModeId,
    pub message_id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOfferEvent {
    pub mode: ModeId,
    pub offer_id: String,
    pub from: String,
    pub to: String,
    pub filename: String,
    pub byte_count: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTransferEvent {
    pub mode: ModeId,
    pub transfer_id: String,
    pub filename: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QsoLogEvent {
    pub mode: ModeId,
    pub call_sign: String,
    pub band: Option<String>,
    pub frequency_hz: Option<u64>,
    pub adif: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterHealthEvent {
    pub mode: ModeId,
    pub ok: bool,
    pub message: String,
    pub receive_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeErrorEvent {
    pub mode: ModeId,
    pub message: String,
    pub recoverable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StationLogRecord {
    pub sequence: u64,
    pub event: StationEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaySummary {
    pub kind: &'static str,
    pub ok: bool,
    pub record_count: usize,
    pub event_counts: BTreeMap<String, usize>,
    pub modes: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionGuardReport {
    pub kind: &'static str,
    pub ok: bool,
    pub action: StationAction,
    pub safety: StationSafetyState,
    pub error: Option<String>,
}

#[derive(Debug, Error)]
pub enum StationError {
    #[error("empty station call sign")]
    EmptyCallSign,
    #[error("unknown station mode: {0}")]
    UnknownMode(String),
    #[error("station action requires TX to be armed: {0}")]
    TransmitNotArmed(&'static str),
    #[error("station action requires external reporting to be enabled: {0}")]
    ReportingNotEnabled(&'static str),
    #[error("failed to serialize station record: {0}")]
    Serialize(serde_json::Error),
    #[error("failed to parse station record at line {line}: {source}")]
    ParseLine {
        line: usize,
        source: serde_json::Error,
    },
    #[error("station log I/O failed: {0}")]
    Io(std::io::Error),
}

pub type StationResult<T> = std::result::Result<T, StationError>;

impl From<std::io::Error> for StationError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn normalize_call(value: &str) -> StationResult<String> {
    let call = value.trim().to_ascii_uppercase();
    if call.is_empty() {
        return Err(StationError::EmptyCallSign);
    }
    Ok(call)
}

pub fn built_in_modes() -> Vec<ModeDescriptor> {
    vec![
        ModeDescriptor {
            id: ModeId::OrcaChat,
            label: ModeId::OrcaChat.label().to_owned(),
            workspace: WorkspaceId::Chat,
            implementation: ModeImplementation::Native,
            status: ModeStatus::Working,
            capabilities: ModeCapabilities {
                free_text: true,
                directed_message: true,
                conversation: true,
                arq: true,
                file_transfer: true,
                mailbox: true,
                store_forward: false,
                logging: true,
                native_modem: true,
                tx_capable: true,
                ..ModeCapabilities::default()
            },
        },
        ModeDescriptor {
            id: ModeId::Js8callExternal,
            label: ModeId::Js8callExternal.label().to_owned(),
            workspace: WorkspaceId::Chat,
            implementation: ModeImplementation::ExternalApp,
            status: ModeStatus::Scaffold,
            capabilities: ModeCapabilities {
                free_text: true,
                directed_message: true,
                conversation: true,
                mailbox: true,
                store_forward: true,
                logging: true,
                external_app_api: true,
                tx_capable: true,
                ..ModeCapabilities::default()
            },
        },
        ModeDescriptor {
            id: ModeId::WsjtxExternal,
            label: ModeId::WsjtxExternal.label().to_owned(),
            workspace: WorkspaceId::WeakSignal,
            implementation: ModeImplementation::ExternalApp,
            status: ModeStatus::Scaffold,
            capabilities: ModeCapabilities {
                free_text: true,
                fixed_time_slots: true,
                decode_table: true,
                logging: true,
                external_app_api: true,
                requires_time_sync: true,
                tx_capable: true,
                ..ModeCapabilities::default()
            },
        },
        ModeDescriptor {
            id: ModeId::FldigiExternal,
            label: ModeId::FldigiExternal.label().to_owned(),
            workspace: WorkspaceId::OperatorConsole,
            implementation: ModeImplementation::ExternalApp,
            status: ModeStatus::Scaffold,
            capabilities: ModeCapabilities {
                free_text: true,
                directed_message: true,
                conversation: true,
                decode_table: true,
                logging: true,
                external_app_api: true,
                tx_capable: true,
                ..ModeCapabilities::default()
            },
        },
        ModeDescriptor {
            id: ModeId::CwAssist,
            label: ModeId::CwAssist.label().to_owned(),
            workspace: WorkspaceId::CwAssist,
            implementation: ModeImplementation::FakeFixture,
            status: ModeStatus::Scaffold,
            capabilities: ModeCapabilities {
                free_text: true,
                decode_table: true,
                logging: true,
                rx_only: true,
                ..ModeCapabilities::default()
            },
        },
        ModeDescriptor {
            id: ModeId::PskReporter,
            label: ModeId::PskReporter.label().to_owned(),
            workspace: WorkspaceId::Spots,
            implementation: ModeImplementation::ExternalApp,
            status: ModeStatus::Scaffold,
            capabilities: ModeCapabilities {
                spot_reporting: true,
                logging: true,
                external_app_api: true,
                rx_only: true,
                ..ModeCapabilities::default()
            },
        },
    ]
}

pub fn mode_by_label(value: &str) -> StationResult<ModeId> {
    ModeId::parse(value).ok_or_else(|| StationError::UnknownMode(value.to_owned()))
}

pub fn validate_action(action: &StationAction, safety: StationSafetyState) -> StationResult<()> {
    match action {
        StationAction::SendMessage { .. }
        | StationAction::Reply { .. }
        | StationAction::QueueMail { .. }
            if !safety.tx_armed =>
        {
            return Err(StationError::TransmitNotArmed(action_label(action)));
        }
        StationAction::ReportSpot { .. } if !safety.reporting_enabled => {
            return Err(StationError::ReportingNotEnabled(action_label(action)));
        }
        _ => {}
    }
    Ok(())
}

pub fn action_guard_report(action: StationAction, safety: StationSafetyState) -> ActionGuardReport {
    match validate_action(&action, safety) {
        Ok(()) => ActionGuardReport {
            kind: "station-action-guard-report",
            ok: true,
            action,
            safety,
            error: None,
        },
        Err(error) => ActionGuardReport {
            kind: "station-action-guard-report",
            ok: false,
            action,
            safety,
            error: Some(error.to_string()),
        },
    }
}

pub fn fake_events_for_mode(mode: ModeId, station: &str) -> StationResult<Vec<StationLogRecord>> {
    let station = normalize_call(station)?;
    let peer = "JA1QSO".to_owned();
    let records = match mode {
        ModeId::OrcaChat => vec![
            StationEvent::AdapterHealth(AdapterHealthEvent {
                mode,
                ok: true,
                message: "orca chat adapter ready".to_owned(),
                receive_only: false,
            }),
            StationEvent::ChatMessage(ChatMessageEvent {
                mode,
                sequence: 1,
                from: station.clone(),
                to: peer.clone(),
                text: "hello over orca chat".to_owned(),
            }),
            StationEvent::MailMessage(MailMessageEvent {
                mode,
                message_id: "mail-001".to_owned(),
                from: station.clone(),
                to: peer.clone(),
                subject: "Synthetic mailbox test".to_owned(),
                body: "No-hardware station replay".to_owned(),
            }),
            StationEvent::FileOffer(FileOfferEvent {
                mode,
                offer_id: "file-001".to_owned(),
                from: station,
                to: peer,
                filename: "payload.txt".to_owned(),
                byte_count: 42,
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_owned(),
            }),
        ],
        ModeId::Js8callExternal => vec![
            StationEvent::AdapterHealth(AdapterHealthEvent {
                mode,
                ok: true,
                message: "JS8Call fixture API connected receive-only".to_owned(),
                receive_only: true,
            }),
            StationEvent::DirectedMessage(DirectedMessageEvent {
                mode,
                from: peer,
                to: station,
                text: "MSG HELLO FROM JS8 FIXTURE".to_owned(),
                snr_db: Some(-18),
            }),
        ],
        ModeId::WsjtxExternal => vec![
            StationEvent::AdapterHealth(AdapterHealthEvent {
                mode,
                ok: true,
                message: "WSJT-X UDP fixture stream ready".to_owned(),
                receive_only: true,
            }),
            StationEvent::Decode(DecodeEvent {
                mode,
                from: Some(peer),
                text: "CQ JA1QSO PM95".to_owned(),
                snr_db: Some(-12),
                frequency_hz: Some(14_074_000),
            }),
        ],
        ModeId::FldigiExternal => vec![
            StationEvent::AdapterHealth(AdapterHealthEvent {
                mode,
                ok: true,
                message: "fldigi XML-RPC fixture connected with TX denied".to_owned(),
                receive_only: true,
            }),
            StationEvent::Decode(DecodeEvent {
                mode,
                from: Some(peer),
                text: "CQ CQ DE JA1QSO".to_owned(),
                snr_db: None,
                frequency_hz: Some(14_070_000),
            }),
        ],
        ModeId::CwAssist => vec![
            StationEvent::AdapterHealth(AdapterHealthEvent {
                mode,
                ok: true,
                message: "CW assist fixture decoder ready".to_owned(),
                receive_only: true,
            }),
            StationEvent::Decode(DecodeEvent {
                mode,
                from: Some(peer),
                text: "CQ TEST JA1QSO".to_owned(),
                snr_db: None,
                frequency_hz: Some(7_030_000),
            }),
        ],
        ModeId::PskReporter => vec![
            StationEvent::AdapterHealth(AdapterHealthEvent {
                mode,
                ok: true,
                message: "PSK Reporter fixture queue ready; reporting disabled".to_owned(),
                receive_only: true,
            }),
            StationEvent::Spot(SpotEvent {
                mode,
                call_sign: peer,
                frequency_hz: 14_074_000,
                snr_db: Some(-10),
                source: "fixture".to_owned(),
            }),
        ],
    };
    Ok(records
        .into_iter()
        .enumerate()
        .map(|(index, event)| StationLogRecord {
            sequence: (index + 1) as u64,
            event,
        })
        .collect())
}

pub fn write_event_log(path: &Path, records: &[StationLogRecord]) -> StationResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    for record in records {
        serde_json::to_writer(&mut file, record).map_err(StationError::Serialize)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

pub fn read_event_log(path: &Path) -> StationResult<Vec<StationLogRecord>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str(&line).map_err(|source| StationError::ParseLine {
            line: index + 1,
            source,
        })?;
        records.push(record);
    }
    Ok(records)
}

pub fn replay_summary(records: &[StationLogRecord]) -> ReplaySummary {
    let mut event_counts = BTreeMap::new();
    let mut modes = BTreeMap::new();
    for record in records {
        *event_counts
            .entry(record.event.kind_label().to_owned())
            .or_insert(0) += 1;
        if let Some(mode) = event_mode(&record.event) {
            *modes.entry(mode.label().to_owned()).or_insert(0) += 1;
        }
    }
    ReplaySummary {
        kind: "station-replay-summary",
        ok: true,
        record_count: records.len(),
        event_counts,
        modes,
    }
}

fn action_label(action: &StationAction) -> &'static str {
    match action {
        StationAction::SetMode { .. } => "set-mode",
        StationAction::Tune { .. } => "tune",
        StationAction::Connect { .. } => "connect",
        StationAction::Listen => "listen",
        StationAction::SendMessage { .. } => "send-message",
        StationAction::Reply { .. } => "reply",
        StationAction::QueueMail { .. } => "queue-mail",
        StationAction::AcceptFile { .. } => "accept-file",
        StationAction::LogQso { .. } => "log-qso",
        StationAction::ReportSpot { .. } => "report-spot",
        StationAction::ArmTransmit => "arm-transmit",
        StationAction::AbortTransmit => "abort-transmit",
        StationAction::ChangeWorkspace { .. } => "change-workspace",
    }
}

fn event_mode(event: &StationEvent) -> Option<ModeId> {
    match event {
        StationEvent::RigStatus(event) => Some(event.mode),
        StationEvent::AudioStatus(event) => Some(event.mode),
        StationEvent::PttChanged(_) => None,
        StationEvent::Decode(event) => Some(event.mode),
        StationEvent::DirectedMessage(event) => Some(event.mode),
        StationEvent::ChatMessage(event) => Some(event.mode),
        StationEvent::Spot(event) => Some(event.mode),
        StationEvent::InboxMessage(event) => Some(event.mode),
        StationEvent::MailMessage(event) => Some(event.mode),
        StationEvent::FileOffer(event) => Some(event.mode),
        StationEvent::FileTransferProgress(event) => Some(event.mode),
        StationEvent::QsoLogged(event) => Some(event.mode),
        StationEvent::AdapterHealth(event) => Some(event.mode),
        StationEvent::ModeError(event) => Some(event.mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_cover_planned_workspaces() {
        let modes = built_in_modes();
        assert_eq!(modes.len(), 6);
        assert!(modes.iter().any(|mode| mode.id == ModeId::OrcaChat));
        assert!(
            modes
                .iter()
                .any(|mode| mode.workspace == WorkspaceId::WeakSignal)
        );
        assert!(modes.iter().any(|mode| mode.capabilities.spot_reporting));
    }

    #[test]
    fn safety_rejects_unarmed_transmit_actions() {
        let action = StationAction::SendMessage {
            to: "JA1QSO".to_owned(),
            text: "hello".to_owned(),
        };
        let report = action_guard_report(action, StationSafetyState::default());
        assert!(!report.ok);
        assert!(report.error.unwrap().contains("requires TX"));
    }

    #[test]
    fn safety_allows_armed_transmit_actions() {
        let action = StationAction::QueueMail {
            to: "JA1QSO".to_owned(),
            subject: "subject".to_owned(),
            body: "body".to_owned(),
        };
        let safety = StationSafetyState {
            tx_armed: true,
            ..StationSafetyState::default()
        };
        assert!(action_guard_report(action, safety).ok);
    }

    #[test]
    fn fake_event_replay_summarizes_modes() {
        let events = fake_events_for_mode(ModeId::Js8callExternal, "ja1tst").expect("events");
        let summary = replay_summary(&events);
        assert_eq!(summary.record_count, 2);
        assert_eq!(summary.modes["js8call-external"], 2);
        assert_eq!(summary.event_counts["directed-message"], 1);
    }

    #[test]
    fn event_log_roundtrips_jsonl() {
        let dir = tempfile_dir();
        let path = dir.join("events.jsonl");
        let records = fake_events_for_mode(ModeId::PskReporter, "ja1tst").expect("events");
        write_event_log(&path, &records).expect("write");
        let observed = read_event_log(&path).expect("read");
        assert_eq!(observed, records);
    }

    fn tempfile_dir() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("chattybara-station-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("tempdir");
        path
    }
}

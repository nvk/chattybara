//! Native chat session model and no-hardware backends for chattybara.

use serde::Serialize;
use thiserror::Error;

pub const LAYER: &str = "chat";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StationProfile {
    pub call_sign: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatState {
    Idle,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatMessage {
    pub sequence: u64,
    pub direction: MessageDirection,
    pub from: String,
    pub to: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ChatEvent {
    Connected {
        local_call: String,
        remote_call: String,
    },
    Message {
        sequence: u64,
        direction: MessageDirection,
        from: String,
        to: String,
        text: String,
    },
    Disconnected {
        local_call: String,
        remote_call: Option<String>,
    },
    Status {
        state: ChatState,
        local_call: String,
        remote_call: Option<String>,
        message_count: usize,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTranscript {
    pub kind: &'static str,
    pub station: StationProfile,
    pub state: ChatState,
    pub peer_call: Option<String>,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScriptCommandReport {
    pub line_number: usize,
    pub line: String,
    pub ok: bool,
    pub event: Option<ChatEvent>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatScriptReport {
    pub kind: &'static str,
    pub backend: &'static str,
    pub ok: bool,
    pub commands: Vec<ScriptCommandReport>,
    pub transcript: ChatTranscript,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimpleLogLineReport {
    pub line_number: usize,
    pub line: String,
    pub ok: bool,
    pub message: Option<ChatMessage>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatLogReport {
    pub kind: &'static str,
    pub backend: &'static str,
    pub ok: bool,
    pub commands: Vec<SimpleLogLineReport>,
    pub transcript: ChatTranscript,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTranscriptComparisonReport {
    pub kind: &'static str,
    pub ok: bool,
    pub expected_message_count: usize,
    pub observed_message_count: usize,
    pub mismatches: Vec<ChatTranscriptMismatch>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatTranscriptMismatch {
    pub message_index: Option<usize>,
    pub field: String,
    pub expected: Option<String>,
    pub observed: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatScriptLogComparisonReport {
    pub kind: &'static str,
    pub ok: bool,
    pub expected: ChatScriptReport,
    pub observed: ChatLogReport,
    pub comparison: ChatTranscriptComparisonReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatPeerLogComparisonReport {
    pub kind: &'static str,
    pub ok: bool,
    pub station_a: ChatLogReport,
    pub station_b: ChatLogReport,
    pub mismatches: Vec<ChatPeerLogMismatch>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatPeerLogMismatch {
    pub message_index: Option<usize>,
    pub field: String,
    pub station_a: Option<String>,
    pub station_b: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatAppScriptReport {
    pub kind: &'static str,
    pub backend: &'static str,
    pub ok: bool,
    pub station: StationProfile,
    pub commands: Vec<ChatAppCommandReport>,
    pub state: ChatAppState,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatAppCommandReport {
    pub line_number: usize,
    pub line: String,
    pub ok: bool,
    pub event: Option<ChatAppEvent>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatAppState {
    pub kind: &'static str,
    pub station: StationProfile,
    pub beacons: Vec<BeaconPost>,
    pub cq_calls: Vec<CqCall>,
    pub mailbox: Vec<MailboxMessage>,
    pub file_offers: Vec<FileOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BeaconPost {
    pub sequence: u64,
    pub from: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CqCall {
    pub sequence: u64,
    pub from: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MailboxMessage {
    pub sequence: u64,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileOffer {
    pub sequence: u64,
    pub from: String,
    pub to: String,
    pub filename: String,
    pub byte_count: u64,
    pub sha256: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ChatAppEvent {
    Beacon(BeaconPost),
    Cq(CqCall),
    MailboxMessage(MailboxMessage),
    FileOffer(FileOffer),
    Status {
        station_call: String,
        beacon_count: usize,
        cq_count: usize,
        mailbox_count: usize,
        file_offer_count: usize,
    },
}

pub trait ChatBackend {
    fn connect(&mut self, remote_call: &str) -> Result<ChatEvent>;
    fn send_text(&mut self, text: &str) -> Result<ChatEvent>;
    fn receive_text(&mut self, from_call: &str, text: &str) -> Result<ChatEvent>;
    fn disconnect(&mut self) -> Result<ChatEvent>;
    fn status(&self) -> ChatEvent;
    fn transcript(&self) -> ChatTranscript;
}

#[derive(Debug, Error)]
pub enum ChatError {
    #[error("empty station call sign")]
    EmptyCallSign,
    #[error("empty chat text")]
    EmptyText,
    #[error("unknown chat script command: {0}")]
    UnknownCommand(String),
    #[error("missing argument for {0}")]
    MissingArgument(String),
    #[error("invalid byte count for {command}: {value}")]
    InvalidByteCount { command: String, value: String },
    #[error("invalid SHA-256 for {command}: {value}")]
    InvalidSha256 { command: String, value: String },
    #[error("chat session is already connected to {0}")]
    AlreadyConnected(String),
    #[error("chat session is not connected")]
    NotConnected,
    #[error("inbound peer {actual} does not match connected peer {expected}")]
    PeerMismatch { expected: String, actual: String },
}

pub type Result<T> = std::result::Result<T, ChatError>;

#[derive(Debug, Clone)]
pub struct FakeBackend {
    station: StationProfile,
    state: ChatState,
    peer_call: Option<String>,
    next_sequence: u64,
    messages: Vec<ChatMessage>,
}

impl FakeBackend {
    pub fn new(station_call: impl AsRef<str>) -> Result<Self> {
        Ok(Self {
            station: StationProfile {
                call_sign: normalize_call(station_call.as_ref())?,
            },
            state: ChatState::Idle,
            peer_call: None,
            next_sequence: 1,
            messages: Vec::new(),
        })
    }

    fn push_message(
        &mut self,
        direction: MessageDirection,
        from: String,
        to: String,
        text: String,
    ) -> ChatEvent {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.messages.push(ChatMessage {
            sequence,
            direction,
            from: from.clone(),
            to: to.clone(),
            text: text.clone(),
        });
        ChatEvent::Message {
            sequence,
            direction,
            from,
            to,
            text,
        }
    }
}

impl ChatBackend for FakeBackend {
    fn connect(&mut self, remote_call: &str) -> Result<ChatEvent> {
        if let Some(peer) = &self.peer_call {
            return Err(ChatError::AlreadyConnected(peer.clone()));
        }

        let remote_call = normalize_call(remote_call)?;
        self.state = ChatState::Connected;
        self.peer_call = Some(remote_call.clone());
        Ok(ChatEvent::Connected {
            local_call: self.station.call_sign.clone(),
            remote_call,
        })
    }

    fn send_text(&mut self, text: &str) -> Result<ChatEvent> {
        let text = require_text(text)?;
        let to = self.peer_call.clone().ok_or(ChatError::NotConnected)?;
        Ok(self.push_message(
            MessageDirection::Outbound,
            self.station.call_sign.clone(),
            to,
            text,
        ))
    }

    fn receive_text(&mut self, from_call: &str, text: &str) -> Result<ChatEvent> {
        let text = require_text(text)?;
        let actual = normalize_call(from_call)?;
        let expected = self.peer_call.clone().ok_or(ChatError::NotConnected)?;
        if actual != expected {
            return Err(ChatError::PeerMismatch { expected, actual });
        }

        Ok(self.push_message(
            MessageDirection::Inbound,
            actual,
            self.station.call_sign.clone(),
            text,
        ))
    }

    fn disconnect(&mut self) -> Result<ChatEvent> {
        let remote_call = self.peer_call.take();
        self.state = ChatState::Idle;
        Ok(ChatEvent::Disconnected {
            local_call: self.station.call_sign.clone(),
            remote_call,
        })
    }

    fn status(&self) -> ChatEvent {
        ChatEvent::Status {
            state: self.state,
            local_call: self.station.call_sign.clone(),
            remote_call: self.peer_call.clone(),
            message_count: self.messages.len(),
        }
    }

    fn transcript(&self) -> ChatTranscript {
        ChatTranscript {
            kind: "chat-transcript",
            station: self.station.clone(),
            state: self.state,
            peer_call: self.peer_call.clone(),
            messages: self.messages.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatAppModel {
    station: StationProfile,
    next_sequence: u64,
    beacons: Vec<BeaconPost>,
    cq_calls: Vec<CqCall>,
    mailbox: Vec<MailboxMessage>,
    file_offers: Vec<FileOffer>,
}

impl ChatAppModel {
    pub fn new(station_call: impl AsRef<str>) -> Result<Self> {
        Ok(Self {
            station: StationProfile {
                call_sign: normalize_call(station_call.as_ref())?,
            },
            next_sequence: 1,
            beacons: Vec::new(),
            cq_calls: Vec::new(),
            mailbox: Vec::new(),
            file_offers: Vec::new(),
        })
    }

    pub fn beacon(&mut self, text: &str) -> Result<ChatAppEvent> {
        let from = self.station.call_sign.clone();
        self.record_beacon(&from, text)
    }

    pub fn observe_beacon(&mut self, from: &str, text: &str) -> Result<ChatAppEvent> {
        self.record_beacon(&normalize_call(from)?, text)
    }

    fn record_beacon(&mut self, from: &str, text: &str) -> Result<ChatAppEvent> {
        let post = BeaconPost {
            sequence: self.next_sequence(),
            from: from.to_owned(),
            text: require_text(text)?,
        };
        self.beacons.push(post.clone());
        Ok(ChatAppEvent::Beacon(post))
    }

    pub fn cq(&mut self, text: &str) -> Result<ChatAppEvent> {
        let from = self.station.call_sign.clone();
        self.record_cq(&from, text)
    }

    pub fn observe_cq(&mut self, from: &str, text: &str) -> Result<ChatAppEvent> {
        self.record_cq(&normalize_call(from)?, text)
    }

    fn record_cq(&mut self, from: &str, text: &str) -> Result<ChatAppEvent> {
        let cq = CqCall {
            sequence: self.next_sequence(),
            from: from.to_owned(),
            text: require_text(text)?,
        };
        self.cq_calls.push(cq.clone());
        Ok(ChatAppEvent::Cq(cq))
    }

    pub fn mailbox_message(&mut self, to: &str, subject: &str, body: &str) -> Result<ChatAppEvent> {
        let from = self.station.call_sign.clone();
        self.record_mailbox_message(&from, to, subject, body)
    }

    pub fn receive_mailbox_message(
        &mut self,
        from: &str,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<ChatAppEvent> {
        self.record_mailbox_message(&normalize_call(from)?, to, subject, body)
    }

    fn record_mailbox_message(
        &mut self,
        from: &str,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<ChatAppEvent> {
        let message = MailboxMessage {
            sequence: self.next_sequence(),
            from: from.to_owned(),
            to: normalize_call(to)?,
            subject: require_text(subject)?,
            body: require_text(body)?,
        };
        self.mailbox.push(message.clone());
        Ok(ChatAppEvent::MailboxMessage(message))
    }

    pub fn file_offer(
        &mut self,
        to: &str,
        filename: &str,
        byte_count: u64,
        sha256: &str,
        note: Option<String>,
    ) -> Result<ChatAppEvent> {
        let from = self.station.call_sign.clone();
        self.record_file_offer(&from, to, filename, byte_count, sha256, note)
    }

    pub fn receive_file_offer(
        &mut self,
        from: &str,
        to: &str,
        filename: &str,
        byte_count: u64,
        sha256: &str,
        note: Option<String>,
    ) -> Result<ChatAppEvent> {
        self.record_file_offer(
            &normalize_call(from)?,
            to,
            filename,
            byte_count,
            sha256,
            note,
        )
    }

    fn record_file_offer(
        &mut self,
        from: &str,
        to: &str,
        filename: &str,
        byte_count: u64,
        sha256: &str,
        note: Option<String>,
    ) -> Result<ChatAppEvent> {
        let offer = FileOffer {
            sequence: self.next_sequence(),
            from: from.to_owned(),
            to: normalize_call(to)?,
            filename: require_text(filename)?,
            byte_count,
            sha256: validate_sha256(sha256, "FILE-OFFER")?,
            note: note
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        };
        self.file_offers.push(offer.clone());
        Ok(ChatAppEvent::FileOffer(offer))
    }

    pub fn status(&self) -> ChatAppEvent {
        ChatAppEvent::Status {
            station_call: self.station.call_sign.clone(),
            beacon_count: self.beacons.len(),
            cq_count: self.cq_calls.len(),
            mailbox_count: self.mailbox.len(),
            file_offer_count: self.file_offers.len(),
        }
    }

    pub fn state(&self) -> ChatAppState {
        ChatAppState {
            kind: "chat-app-state",
            station: self.station.clone(),
            beacons: self.beacons.clone(),
            cq_calls: self.cq_calls.clone(),
            mailbox: self.mailbox.clone(),
            file_offers: self.file_offers.clone(),
        }
    }

    fn next_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        sequence
    }
}

pub fn run_fake_script(station_call: impl AsRef<str>, script: &str) -> Result<ChatScriptReport> {
    let mut backend = FakeBackend::new(station_call)?;
    let mut commands = Vec::new();

    for (index, line) in script.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let result = execute_script_line(&mut backend, trimmed);
        match result {
            Ok(event) => commands.push(ScriptCommandReport {
                line_number: index + 1,
                line: trimmed.to_owned(),
                ok: true,
                event: Some(event),
                error: None,
            }),
            Err(error) => commands.push(ScriptCommandReport {
                line_number: index + 1,
                line: trimmed.to_owned(),
                ok: false,
                event: None,
                error: Some(error.to_string()),
            }),
        }
    }

    let ok = commands.iter().all(|command| command.ok);
    Ok(ChatScriptReport {
        kind: "chat-script-report",
        backend: "fake",
        ok,
        commands,
        transcript: backend.transcript(),
    })
}

pub fn run_app_script(station_call: impl AsRef<str>, script: &str) -> Result<ChatAppScriptReport> {
    let mut model = ChatAppModel::new(station_call)?;
    let mut commands = Vec::new();

    for (index, line) in script.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        match execute_app_line(&mut model, trimmed) {
            Ok(event) => commands.push(ChatAppCommandReport {
                line_number: index + 1,
                line: trimmed.to_owned(),
                ok: true,
                event: Some(event),
                error: None,
            }),
            Err(error) => commands.push(ChatAppCommandReport {
                line_number: index + 1,
                line: trimmed.to_owned(),
                ok: false,
                event: None,
                error: Some(error.to_string()),
            }),
        }
    }

    let ok = commands.iter().all(|command| command.ok);
    Ok(ChatAppScriptReport {
        kind: "chat-app-script-report",
        backend: "native-app-model",
        ok,
        station: model.station.clone(),
        commands,
        state: model.state(),
    })
}

pub fn parse_simple_log(station_call: impl AsRef<str>, log: &str) -> Result<ChatLogReport> {
    let station = StationProfile {
        call_sign: normalize_call(station_call.as_ref())?,
    };
    let mut next_sequence = 1;
    let mut messages = Vec::new();
    let mut commands = Vec::new();

    for (index, line) in log.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        match parse_simple_log_line(&station.call_sign, next_sequence, trimmed) {
            Ok(message) => {
                next_sequence += 1;
                commands.push(SimpleLogLineReport {
                    line_number: index + 1,
                    line: trimmed.to_owned(),
                    ok: true,
                    message: Some(message.clone()),
                    error: None,
                });
                messages.push(message);
            }
            Err(error) => commands.push(SimpleLogLineReport {
                line_number: index + 1,
                line: trimmed.to_owned(),
                ok: false,
                message: None,
                error: Some(error.to_string()),
            }),
        }
    }

    let ok = commands.iter().all(|command| command.ok);
    let peer_call = infer_peer(&station.call_sign, &messages);
    Ok(ChatLogReport {
        kind: "chat-log-report",
        backend: "simple-log",
        ok,
        commands,
        transcript: ChatTranscript {
            kind: "chat-transcript",
            station,
            state: ChatState::Idle,
            peer_call,
            messages,
        },
    })
}

pub fn compare_chat_transcripts(
    expected: &ChatTranscript,
    observed: &ChatTranscript,
) -> ChatTranscriptComparisonReport {
    let mut mismatches = Vec::new();
    if expected.station.call_sign != observed.station.call_sign {
        mismatches.push(ChatTranscriptMismatch {
            message_index: None,
            field: "station.call_sign".to_owned(),
            expected: Some(expected.station.call_sign.clone()),
            observed: Some(observed.station.call_sign.clone()),
        });
    }

    let max_messages = expected.messages.len().max(observed.messages.len());
    for index in 0..max_messages {
        match (expected.messages.get(index), observed.messages.get(index)) {
            (Some(expected), Some(observed)) => {
                compare_message_field(
                    &mut mismatches,
                    index,
                    "direction",
                    direction_label(expected.direction),
                    direction_label(observed.direction),
                );
                compare_message_field(
                    &mut mismatches,
                    index,
                    "from",
                    &expected.from,
                    &observed.from,
                );
                compare_message_field(&mut mismatches, index, "to", &expected.to, &observed.to);
                compare_message_field(
                    &mut mismatches,
                    index,
                    "text",
                    &expected.text,
                    &observed.text,
                );
            }
            (Some(expected), None) => mismatches.push(ChatTranscriptMismatch {
                message_index: Some(index + 1),
                field: "message".to_owned(),
                expected: Some(message_summary(expected)),
                observed: None,
            }),
            (None, Some(observed)) => mismatches.push(ChatTranscriptMismatch {
                message_index: Some(index + 1),
                field: "message".to_owned(),
                expected: None,
                observed: Some(message_summary(observed)),
            }),
            (None, None) => {}
        }
    }

    ChatTranscriptComparisonReport {
        kind: "chat-transcript-comparison-report",
        ok: mismatches.is_empty(),
        expected_message_count: expected.messages.len(),
        observed_message_count: observed.messages.len(),
        mismatches,
    }
}

pub fn compare_fake_script_to_simple_log(
    station_call: impl AsRef<str>,
    script: &str,
    log: &str,
) -> Result<ChatScriptLogComparisonReport> {
    let expected = run_fake_script(station_call.as_ref(), script)?;
    let observed = parse_simple_log(station_call.as_ref(), log)?;
    let comparison = compare_chat_transcripts(&expected.transcript, &observed.transcript);
    let ok = expected.ok && observed.ok && comparison.ok;

    Ok(ChatScriptLogComparisonReport {
        kind: "chat-script-log-comparison-report",
        ok,
        expected,
        observed,
        comparison,
    })
}

pub fn compare_peer_logs(
    station_a_call: impl AsRef<str>,
    station_a_log: &str,
    station_b_call: impl AsRef<str>,
    station_b_log: &str,
) -> Result<ChatPeerLogComparisonReport> {
    let station_a = parse_simple_log(station_a_call.as_ref(), station_a_log)?;
    let station_b = parse_simple_log(station_b_call.as_ref(), station_b_log)?;
    let mismatches = compare_peer_log_transcripts(&station_a.transcript, &station_b.transcript);
    let ok = station_a.ok && station_b.ok && mismatches.is_empty();

    Ok(ChatPeerLogComparisonReport {
        kind: "chat-peer-log-comparison-report",
        ok,
        station_a,
        station_b,
        mismatches,
    })
}

fn execute_script_line(backend: &mut impl ChatBackend, line: &str) -> Result<ChatEvent> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or_default().to_ascii_uppercase();
    let rest = parts.next().unwrap_or_default().trim();

    match verb.as_str() {
        "CONNECT" | "C" => backend.connect(&required(rest, &verb)?),
        "SEND" | "TX" => backend.send_text(&required(rest, &verb)?),
        "RX" | "RECV" | "RECEIVE" => {
            let (from, text) = split_two(rest, &verb)?;
            backend.receive_text(&from, &text)
        }
        "DISCONNECT" | "DISC" | "D" => backend.disconnect(),
        "STATUS" | "STATE" => Ok(backend.status()),
        _ => Err(ChatError::UnknownCommand(verb)),
    }
}

fn execute_app_line(model: &mut ChatAppModel, line: &str) -> Result<ChatAppEvent> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or_default().to_ascii_uppercase();
    let rest = parts.next().unwrap_or_default().trim();

    match verb.as_str() {
        "BEACON" | "BCN" => model.beacon(&required(rest, &verb)?),
        "CQ" => model.cq(&required(rest, &verb)?),
        "MAIL" | "MAILBOX" | "VMAIL" => {
            let (to, subject, body) = split_mailbox(rest, &verb)?;
            model.mailbox_message(&to, &subject, &body)
        }
        "FILE-OFFER" | "FILE_OFFER" | "FILE" => {
            let (to, filename, byte_count, sha256, note) = split_file_offer(rest, &verb)?;
            model.file_offer(&to, &filename, byte_count, &sha256, note)
        }
        "STATUS" | "STATE" => Ok(model.status()),
        _ => Err(ChatError::UnknownCommand(verb)),
    }
}

fn parse_simple_log_line(station_call: &str, sequence: u64, line: &str) -> Result<ChatMessage> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or_default().to_ascii_uppercase();
    let rest = parts.next().unwrap_or_default().trim();

    match verb.as_str() {
        "OUT" | "TX" | "SEND" => {
            let (to, text) = split_two(rest, &verb)?;
            Ok(ChatMessage {
                sequence,
                direction: MessageDirection::Outbound,
                from: station_call.to_owned(),
                to: normalize_call(&to)?,
                text,
            })
        }
        "IN" | "RX" | "RECV" | "RECEIVE" => {
            let (from, text) = split_two(rest, &verb)?;
            Ok(ChatMessage {
                sequence,
                direction: MessageDirection::Inbound,
                from: normalize_call(&from)?,
                to: station_call.to_owned(),
                text,
            })
        }
        _ => Err(ChatError::UnknownCommand(verb)),
    }
}

fn infer_peer(station_call: &str, messages: &[ChatMessage]) -> Option<String> {
    let mut peer = None;
    for message in messages {
        let candidate = match message.direction {
            MessageDirection::Inbound => &message.from,
            MessageDirection::Outbound => &message.to,
        };
        if candidate == station_call {
            continue;
        }
        match &peer {
            Some(current) if current != candidate => return None,
            Some(_) => {}
            None => peer = Some(candidate.clone()),
        }
    }
    peer
}

fn compare_message_field(
    mismatches: &mut Vec<ChatTranscriptMismatch>,
    index: usize,
    field: &str,
    expected: &str,
    observed: &str,
) {
    if expected != observed {
        mismatches.push(ChatTranscriptMismatch {
            message_index: Some(index + 1),
            field: field.to_owned(),
            expected: Some(expected.to_owned()),
            observed: Some(observed.to_owned()),
        });
    }
}

fn compare_peer_log_transcripts(
    station_a: &ChatTranscript,
    station_b: &ChatTranscript,
) -> Vec<ChatPeerLogMismatch> {
    let mut mismatches = Vec::new();
    if station_a.station.call_sign == station_b.station.call_sign {
        mismatches.push(ChatPeerLogMismatch {
            message_index: None,
            field: "station.call_sign".to_owned(),
            station_a: Some(station_a.station.call_sign.clone()),
            station_b: Some(station_b.station.call_sign.clone()),
        });
    }

    let max_messages = station_a.messages.len().max(station_b.messages.len());
    for index in 0..max_messages {
        match (station_a.messages.get(index), station_b.messages.get(index)) {
            (Some(message_a), Some(message_b)) => {
                let expected_b_direction = complement_direction(message_a.direction);
                if message_b.direction != expected_b_direction {
                    mismatches.push(ChatPeerLogMismatch {
                        message_index: Some(index + 1),
                        field: "direction-complement".to_owned(),
                        station_a: Some(direction_label(message_a.direction).to_owned()),
                        station_b: Some(direction_label(message_b.direction).to_owned()),
                    });
                }
                compare_peer_message_field(
                    &mut mismatches,
                    index,
                    "from",
                    &message_a.from,
                    &message_b.from,
                );
                compare_peer_message_field(
                    &mut mismatches,
                    index,
                    "to",
                    &message_a.to,
                    &message_b.to,
                );
                compare_peer_message_field(
                    &mut mismatches,
                    index,
                    "text",
                    &message_a.text,
                    &message_b.text,
                );
            }
            (Some(message_a), None) => mismatches.push(ChatPeerLogMismatch {
                message_index: Some(index + 1),
                field: "message".to_owned(),
                station_a: Some(message_summary(message_a)),
                station_b: None,
            }),
            (None, Some(message_b)) => mismatches.push(ChatPeerLogMismatch {
                message_index: Some(index + 1),
                field: "message".to_owned(),
                station_a: None,
                station_b: Some(message_summary(message_b)),
            }),
            (None, None) => {}
        }
    }

    mismatches
}

fn compare_peer_message_field(
    mismatches: &mut Vec<ChatPeerLogMismatch>,
    index: usize,
    field: &str,
    station_a: &str,
    station_b: &str,
) {
    if station_a != station_b {
        mismatches.push(ChatPeerLogMismatch {
            message_index: Some(index + 1),
            field: field.to_owned(),
            station_a: Some(station_a.to_owned()),
            station_b: Some(station_b.to_owned()),
        });
    }
}

fn complement_direction(direction: MessageDirection) -> MessageDirection {
    match direction {
        MessageDirection::Inbound => MessageDirection::Outbound,
        MessageDirection::Outbound => MessageDirection::Inbound,
    }
}

fn direction_label(direction: MessageDirection) -> &'static str {
    match direction {
        MessageDirection::Inbound => "inbound",
        MessageDirection::Outbound => "outbound",
    }
}

fn message_summary(message: &ChatMessage) -> String {
    format!(
        "{} {} -> {}: {}",
        direction_label(message.direction),
        message.from,
        message.to,
        message.text
    )
}

fn split_mailbox(rest: &str, verb: &str) -> Result<(String, String, String)> {
    let (to, subject_and_body) = split_two(rest, verb)?;
    let Some((subject, body)) = subject_and_body.split_once('|') else {
        return Err(ChatError::MissingArgument(format!(
            "{verb} to-call subject | body"
        )));
    };
    let subject = subject.trim();
    let body = body.trim();
    if subject.is_empty() || body.is_empty() {
        return Err(ChatError::MissingArgument(format!(
            "{verb} to-call subject | body"
        )));
    }
    Ok((to, subject.to_owned(), body.to_owned()))
}

fn split_file_offer(
    rest: &str,
    verb: &str,
) -> Result<(String, String, u64, String, Option<String>)> {
    let rest = required(rest, verb)?;
    let mut parts = rest.splitn(5, char::is_whitespace);
    let to = parts.next().unwrap_or_default().trim();
    let filename = parts.next().unwrap_or_default().trim();
    let byte_count = parts.next().unwrap_or_default().trim();
    let sha256 = parts.next().unwrap_or_default().trim();
    let note = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if to.is_empty() || filename.is_empty() || byte_count.is_empty() || sha256.is_empty() {
        return Err(ChatError::MissingArgument(format!(
            "{verb} to-call filename byte-count sha256 [note]"
        )));
    }
    let byte_count = byte_count
        .parse::<u64>()
        .map_err(|_| ChatError::InvalidByteCount {
            command: verb.to_owned(),
            value: byte_count.to_owned(),
        })?;
    Ok((
        to.to_owned(),
        filename.to_owned(),
        byte_count,
        sha256.to_owned(),
        note.map(str::to_owned),
    ))
}

fn split_two(rest: &str, verb: &str) -> Result<(String, String)> {
    let rest = required(rest, verb)?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default().trim();
    let second = parts.next().unwrap_or_default().trim();
    if first.is_empty() || second.is_empty() {
        return Err(ChatError::MissingArgument(verb.to_owned()));
    }
    Ok((first.to_owned(), second.to_owned()))
}

fn required(rest: &str, verb: &str) -> Result<String> {
    if rest.is_empty() {
        Err(ChatError::MissingArgument(verb.to_owned()))
    } else {
        Ok(rest.to_owned())
    }
}

fn normalize_call(call: &str) -> Result<String> {
    let normalized = call.trim().to_ascii_uppercase();
    if normalized.is_empty() {
        Err(ChatError::EmptyCallSign)
    } else {
        Ok(normalized)
    }
}

fn validate_sha256(value: &str, command: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() == 64
        && normalized
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        Ok(normalized)
    } else {
        Err(ChatError::InvalidSha256 {
            command: command.to_owned(),
            value: value.to_owned(),
        })
    }
}

fn require_text(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        Err(ChatError::EmptyText)
    } else {
        Ok(text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_backend_runs_basic_qso() {
        let mut backend = FakeBackend::new("ja1tst").expect("backend");

        let event = backend.connect("ja1qso").expect("connect");
        assert_eq!(
            event,
            ChatEvent::Connected {
                local_call: "JA1TST".to_owned(),
                remote_call: "JA1QSO".to_owned(),
            }
        );

        backend.send_text("hello").expect("send");
        backend.receive_text("ja1qso", "roger").expect("receive");
        backend.disconnect().expect("disconnect");

        let transcript = backend.transcript();
        assert_eq!(transcript.state, ChatState::Idle);
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0].direction, MessageDirection::Outbound);
        assert_eq!(transcript.messages[1].direction, MessageDirection::Inbound);
    }

    #[test]
    fn fake_script_reports_commands_and_transcript() {
        let report = run_fake_script(
            "ja1tst",
            r#"
# synthetic basic QSO
CONNECT ja1qso
SEND hello over open radio
RX ja1qso roger
DISCONNECT
"#,
        )
        .expect("script");

        assert!(report.ok);
        assert_eq!(report.commands.len(), 4);
        assert_eq!(report.transcript.messages.len(), 2);
        assert_eq!(report.transcript.messages[0].text, "hello over open radio");
        assert_eq!(report.transcript.messages[1].from, "JA1QSO");
    }

    #[test]
    fn fake_script_records_errors_without_stopping() {
        let report = run_fake_script(
            "ja1tst",
            r#"
SEND too early
CONNECT ja1qso
RX ve3other wrong peer
STATUS
"#,
        )
        .expect("script");

        assert!(!report.ok);
        assert_eq!(report.commands.len(), 4);
        assert_eq!(
            report.commands[0].error.as_deref(),
            Some("chat session is not connected")
        );
        assert!(
            report.commands[2]
                .error
                .as_deref()
                .expect("peer mismatch")
                .contains("does not match")
        );
        assert!(matches!(
            report.commands[3].event,
            Some(ChatEvent::Status { .. })
        ));
    }

    #[test]
    fn app_script_tracks_clean_room_features() {
        let report = run_app_script(
            "ja1tst",
            r#"
BEACON monitoring 14.105 USB
CQ testing local app model
MAIL ja1qso Test subject | Synthetic mailbox body
FILE-OFFER ja1qso sample.txt 42 E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855 metadata only
STATUS
"#,
        )
        .expect("app script");

        assert!(report.ok);
        assert_eq!(report.backend, "native-app-model");
        assert_eq!(report.station.call_sign, "JA1TST");
        assert_eq!(report.commands.len(), 5);
        assert_eq!(report.state.station.call_sign, "JA1TST");
        assert_eq!(report.state.beacons.len(), 1);
        assert_eq!(report.state.beacons[0].sequence, 1);
        assert_eq!(report.state.cq_calls.len(), 1);
        assert_eq!(report.state.mailbox.len(), 1);
        assert_eq!(report.state.mailbox[0].to, "JA1QSO");
        assert_eq!(report.state.mailbox[0].subject, "Test subject");
        assert_eq!(report.state.file_offers.len(), 1);
        assert_eq!(report.state.file_offers[0].byte_count, 42);
        assert_eq!(
            report.state.file_offers[0].sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            report.state.file_offers[0].note.as_deref(),
            Some("metadata only")
        );
        assert!(matches!(
            report.commands[4].event,
            Some(ChatAppEvent::Status {
                beacon_count: 1,
                cq_count: 1,
                mailbox_count: 1,
                file_offer_count: 1,
                ..
            })
        ));
    }

    #[test]
    fn app_model_records_observed_peer_features() {
        let mut model = ChatAppModel::new("ja1tst").expect("model");

        model
            .observe_beacon("ja1qso", "monitoring")
            .expect("beacon");
        model.observe_cq("ja1qso", "calling cq").expect("cq");
        model
            .receive_mailbox_message("ja1qso", "ja1tst", "Subject", "Body")
            .expect("mail");
        model
            .receive_file_offer(
                "ja1qso",
                "ja1tst",
                "sample.txt",
                42,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                Some("note".to_owned()),
            )
            .expect("file");

        let state = model.state();
        assert_eq!(state.beacons[0].from, "JA1QSO");
        assert_eq!(state.cq_calls[0].from, "JA1QSO");
        assert_eq!(state.mailbox[0].from, "JA1QSO");
        assert_eq!(state.mailbox[0].to, "JA1TST");
        assert_eq!(state.file_offers[0].from, "JA1QSO");
        assert_eq!(state.file_offers[0].to, "JA1TST");
    }

    #[test]
    fn app_script_reports_parse_errors_without_stopping() {
        let report = run_app_script(
            "ja1tst",
            r#"
FILE-OFFER ja1qso sample.txt nope e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
FILE-OFFER ja1qso sample.txt 42 not-a-sha
STATUS
"#,
        )
        .expect("app script");

        assert!(!report.ok);
        assert_eq!(report.commands.len(), 3);
        assert!(
            report.commands[0]
                .error
                .as_deref()
                .expect("byte count error")
                .contains("invalid byte count")
        );
        assert!(
            report.commands[1]
                .error
                .as_deref()
                .expect("sha error")
                .contains("invalid SHA-256")
        );
        assert!(matches!(
            report.commands[2].event,
            Some(ChatAppEvent::Status { .. })
        ));
    }

    #[test]
    fn simple_log_parses_public_message_flow() {
        let report = parse_simple_log(
            "ja1tst",
            r#"
# manually normalized public chat log
OUT ja1qso hello
IN ja1qso roger
"#,
        )
        .expect("log");

        assert!(report.ok);
        assert_eq!(report.transcript.station.call_sign, "JA1TST");
        assert_eq!(report.transcript.peer_call.as_deref(), Some("JA1QSO"));
        assert_eq!(report.transcript.messages.len(), 2);
        assert_eq!(
            report.transcript.messages[0].direction,
            MessageDirection::Outbound
        );
        assert_eq!(report.transcript.messages[1].from, "JA1QSO");
    }

    #[test]
    fn simple_log_comparison_reports_mismatches() {
        let report = compare_fake_script_to_simple_log(
            "ja1tst",
            r#"
CONNECT ja1qso
SEND hello
RX ja1qso roger
"#,
            r#"
OUT ja1qso hello
IN ja1qso nope
"#,
        )
        .expect("comparison");

        assert!(!report.ok);
        assert_eq!(report.comparison.expected_message_count, 2);
        assert_eq!(report.comparison.observed_message_count, 2);
        assert_eq!(report.comparison.mismatches.len(), 1);
        assert_eq!(report.comparison.mismatches[0].message_index, Some(2));
        assert_eq!(report.comparison.mismatches[0].field, "text");
    }

    #[test]
    fn peer_log_comparison_matches_complementary_logs() {
        let report = compare_peer_logs(
            "ja1tst",
            r#"
OUT ja1qso hello
IN ja1qso roger
"#,
            "ja1qso",
            r#"
IN ja1tst hello
OUT ja1tst roger
"#,
        )
        .expect("peer comparison");

        assert!(report.ok);
        assert!(report.mismatches.is_empty());
        assert_eq!(report.station_a.transcript.messages.len(), 2);
        assert_eq!(report.station_b.transcript.messages.len(), 2);
    }

    #[test]
    fn peer_log_comparison_reports_cross_station_mismatch() {
        let report =
            compare_peer_logs("ja1tst", "OUT ja1qso hello\n", "ja1qso", "IN ja1tst nope\n")
                .expect("peer comparison");

        assert!(!report.ok);
        assert_eq!(report.mismatches.len(), 1);
        assert_eq!(report.mismatches[0].message_index, Some(1));
        assert_eq!(report.mismatches[0].field, "text");
    }
}

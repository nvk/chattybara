use crate::hamlib::DEFAULT_RIGCTLD_HOST;
use crate::local_peer::{
    LocalLiveAppEvent, LocalLiveEvent, LocalLiveNode, LocalLiveNodeConfig, LocalNodeMode,
};
use anyhow::{Context, Result, bail};
use chattybara_chat::{
    ChatAppEvent, ChatAppModel, ChatAppState, ChatBackend, ChatEvent, ChatMessage, ChatState,
    ChatTranscript, FakeBackend, FileOffer, MailboxMessage, MessageDirection,
};
use chattybara_station::{
    ChatMessageEvent as StationChatMessageEvent, FileOfferEvent as StationFileOfferEvent,
    MailMessageEvent as StationMailMessageEvent, ModeId, StationEvent, StationLogRecord,
    WorkspaceId, write_event_log,
};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use orca_audio::AudioBuffer;
use orca_dsp::ChannelConfig;
use orca_frames::{
    PacketCodecConfig, PacketDecodeReport, PacketEncodeReport, decode_packet_samples,
    encode_packet_payload,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use std::fs;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatTuiBackend {
    Fake,
    NativeLocalNode,
    NativeLoopback,
    NativeWavLoopback,
}

impl ChatTuiBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fake => "fake",
            Self::NativeLocalNode => "native-local-node",
            Self::NativeLoopback => "native-loopback",
            Self::NativeWavLoopback => "native-wav-loopback",
        }
    }

    pub fn from_label(value: &str) -> Option<Self> {
        match value {
            "fake" => Some(Self::Fake),
            "native-local-node" => Some(Self::NativeLocalNode),
            "native-loopback" => Some(Self::NativeLoopback),
            "native-wav-loopback" => Some(Self::NativeWavLoopback),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatTuiConfig {
    pub station_call: String,
    pub backend: ChatTuiBackend,
    pub local_node: Option<ChatTuiLocalNodeConfig>,
    pub setup: Option<ChatTuiSetupConfig>,
}

#[derive(Debug, Clone)]
pub struct ChatTuiSetupConfig {
    pub backend: ChatTuiBackend,
    pub peer_call: Option<String>,
    pub mode: Option<LocalNodeMode>,
    pub channel: ChannelConfig,
}

#[derive(Debug, Clone)]
pub struct ChatTuiLocalNodeConfig {
    pub peer_call: String,
    pub mode: LocalNodeMode,
    pub channel: ChannelConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiCommand {
    Connect(String),
    Send(String),
    Receive {
        from: String,
        text: String,
    },
    Disconnect,
    Beacon(String),
    Cq(String),
    Mail {
        to: String,
        subject: String,
        body: String,
    },
    MailRead {
        sequence: u64,
    },
    MailReply {
        sequence: u64,
        subject: String,
        body: String,
    },
    FileOffer {
        to: String,
        filename: String,
        byte_count: u64,
        sha256: String,
        note: Option<String>,
    },
    FileInspect {
        sequence: u64,
    },
    FileAccept {
        sequence: u64,
        out_dir: PathBuf,
    },
    AppStatus,
    Status,
    SaveApp(PathBuf),
    SaveLog(PathBuf),
    SaveArtifacts(PathBuf),
    SaveSession(PathBuf),
    Workspace(WorkspaceId),
    Setup,
    SetupStation(String),
    SetupBackend(ChatTuiBackend),
    SetupPeer(String),
    SetupListen(String),
    SetupConnectNode(String),
    SetupAudioInput(String),
    SetupAudioOutput(String),
    SetupAudioRate(u32),
    SetupAudioChannels(u16),
    SetupRadioHamlib(String),
    SetupRadioOff,
    SetupStart,
    Help,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiCommandOutcome {
    Continue,
    Quit,
}

pub struct ChatTuiApp {
    station_call: String,
    backend_kind: ChatTuiBackend,
    backend_label: &'static str,
    backend: TuiBackendState,
    app_model: ChatAppModel,
    setup: Option<TuiSetupState>,
    runtime: TuiRuntimeProfile,
    workspace: WorkspaceId,
    input: String,
    lines: Vec<String>,
    focus: TuiPane,
    composer_mode: ComposerMode,
    setup_selected: usize,
    mailbox_selected: usize,
    file_offer_selected: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiPane {
    Setup,
    Transcript,
    Monitor,
    Mailbox,
    FileOffers,
    Composer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerMode {
    Chat,
    Command,
}

#[derive(Debug, Clone)]
struct TuiSetupState {
    backend: ChatTuiBackend,
    peer_call: Option<String>,
    mode: Option<LocalNodeMode>,
    channel: ChannelConfig,
    audio_input: Option<String>,
    audio_output: Option<String>,
    audio_sample_rate: u32,
    audio_channels: u16,
    hamlib_host: Option<String>,
}

#[derive(Debug, Clone)]
struct TuiRuntimeProfile {
    audio_input: Option<String>,
    audio_output: Option<String>,
    audio_sample_rate: u32,
    audio_channels: u16,
    hamlib_host: Option<String>,
    live_audio: bool,
    transmit_armed: bool,
}

impl TuiPane {
    fn next(self) -> Self {
        match self {
            Self::Setup => Self::Transcript,
            Self::Transcript => Self::Monitor,
            Self::Monitor => Self::Mailbox,
            Self::Mailbox => Self::FileOffers,
            Self::FileOffers => Self::Composer,
            Self::Composer => Self::Setup,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Setup => Self::Composer,
            Self::Transcript => Self::Setup,
            Self::Monitor => Self::Transcript,
            Self::Mailbox => Self::Monitor,
            Self::FileOffers => Self::Mailbox,
            Self::Composer => Self::FileOffers,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Setup => "setup/radio",
            Self::Transcript => "transcript",
            Self::Monitor => "monitor",
            Self::Mailbox => "mailbox",
            Self::FileOffers => "files",
            Self::Composer => "composer",
        }
    }
}

impl ComposerMode {
    fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Command => "command",
        }
    }
}

enum TuiBackendState {
    Fake(FakeBackend),
    NativeLocalNode(LocalLiveNode),
    NativeLoopback(NativeLoopbackBackend),
    NativeWavLoopback(NativeLoopbackBackend),
}

enum TuiBackendEvent {
    Chat(ChatEvent),
    App(LocalLiveAppEvent),
}

struct NativeLoopbackBackend {
    label: &'static str,
    medium: LoopbackMedium,
    chat: FakeBackend,
    config: PacketCodecConfig,
    packets: Vec<LoopbackPacketReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopbackMedium {
    InMemory,
    Wav,
}

#[derive(Debug, Clone, serde::Serialize)]
struct LoopbackPacketReport {
    sequence: usize,
    direction: MessageDirection,
    peer_call: String,
    payload_text: String,
    wav_filename: Option<String>,
    encode: PacketEncodeReport,
    decode: PacketDecodeReport,
    #[serde(skip_serializing)]
    samples: Vec<f32>,
}

impl TuiBackendState {
    fn connect(&mut self, call: &str) -> Result<Vec<ChatEvent>> {
        match self {
            Self::Fake(backend) => Ok(vec![backend.connect(call)?]),
            Self::NativeLocalNode(backend) => backend.connect(call),
            Self::NativeLoopback(backend) | Self::NativeWavLoopback(backend) => {
                backend.connect(call)
            }
        }
    }

    fn send_text(&mut self, text: &str) -> Result<Vec<ChatEvent>> {
        match self {
            Self::Fake(backend) => Ok(vec![backend.send_text(text)?]),
            Self::NativeLocalNode(backend) => backend.send_text(text),
            Self::NativeLoopback(backend) | Self::NativeWavLoopback(backend) => {
                backend.send_text(text)
            }
        }
    }

    fn receive_text(&mut self, from: &str, text: &str) -> Result<Vec<ChatEvent>> {
        match self {
            Self::Fake(backend) => Ok(vec![backend.receive_text(from, text)?]),
            Self::NativeLocalNode(backend) => backend.receive_text(from, text),
            Self::NativeLoopback(backend) | Self::NativeWavLoopback(backend) => {
                Ok(vec![backend.receive_text(from, text)?])
            }
        }
    }

    fn disconnect(&mut self) -> Result<Vec<ChatEvent>> {
        match self {
            Self::Fake(backend) => Ok(vec![backend.disconnect()?]),
            Self::NativeLocalNode(backend) => backend.disconnect(),
            Self::NativeLoopback(backend) | Self::NativeWavLoopback(backend) => {
                Ok(vec![backend.disconnect()?])
            }
        }
    }

    fn status(&self) -> ChatEvent {
        match self {
            Self::Fake(backend) => backend.status(),
            Self::NativeLocalNode(backend) => backend.status(),
            Self::NativeLoopback(backend) | Self::NativeWavLoopback(backend) => backend.status(),
        }
    }

    fn transcript(&self) -> ChatTranscript {
        match self {
            Self::Fake(backend) => backend.transcript(),
            Self::NativeLocalNode(backend) => backend.transcript(),
            Self::NativeLoopback(backend) | Self::NativeWavLoopback(backend) => {
                backend.transcript()
            }
        }
    }

    fn artifacts_report(&self) -> serde_json::Value {
        match self {
            Self::Fake(backend) => serde_json::json!({
                "kind": "tui-backend-artifacts",
                "backend": "fake",
                "packet_count": 0,
                "transcript": backend.transcript(),
                "packets": [],
            }),
            Self::NativeLocalNode(backend) => backend.artifacts_report(None),
            Self::NativeLoopback(backend) | Self::NativeWavLoopback(backend) => {
                backend.artifacts_report(None)
            }
        }
    }

    fn write_packet_wavs(&self, dir: &Path) -> Result<Vec<Option<PathBuf>>> {
        match self {
            Self::Fake(_) => Ok(Vec::new()),
            Self::NativeLocalNode(backend) => backend.write_packet_wavs(dir),
            Self::NativeLoopback(backend) | Self::NativeWavLoopback(backend) => {
                backend.write_packet_wavs(dir)
            }
        }
    }

    fn send_beacon(&mut self, text: &str) -> Result<()> {
        match self {
            Self::NativeLocalNode(backend) => backend.send_beacon(text),
            Self::Fake(_) | Self::NativeLoopback(_) | Self::NativeWavLoopback(_) => Ok(()),
        }
    }

    fn send_cq(&mut self, text: &str) -> Result<()> {
        match self {
            Self::NativeLocalNode(backend) => backend.send_cq(text),
            Self::Fake(_) | Self::NativeLoopback(_) | Self::NativeWavLoopback(_) => Ok(()),
        }
    }

    fn send_mail(&mut self, subject: &str, body: &str) -> Result<()> {
        match self {
            Self::NativeLocalNode(backend) => backend.send_mail(subject, body),
            Self::Fake(_) | Self::NativeLoopback(_) | Self::NativeWavLoopback(_) => Ok(()),
        }
    }

    fn send_file_offer(
        &mut self,
        filename: &str,
        byte_count: u64,
        sha256: &str,
        note: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::NativeLocalNode(backend) => {
                backend.send_file_offer(filename, byte_count, sha256, note)
            }
            Self::Fake(_) | Self::NativeLoopback(_) | Self::NativeWavLoopback(_) => Ok(()),
        }
    }

    fn poll_events(&mut self) -> Result<Vec<TuiBackendEvent>> {
        match self {
            Self::NativeLocalNode(backend) => Ok(backend
                .poll_events()?
                .into_iter()
                .map(|event| match event {
                    LocalLiveEvent::Chat(event) => TuiBackendEvent::Chat(event),
                    LocalLiveEvent::App(event) => TuiBackendEvent::App(event),
                })
                .collect()),
            Self::Fake(_) | Self::NativeLoopback(_) | Self::NativeWavLoopback(_) => Ok(Vec::new()),
        }
    }
}

impl NativeLoopbackBackend {
    fn new(station_call: &str, label: &'static str, medium: LoopbackMedium) -> Result<Self> {
        Ok(Self {
            label,
            medium,
            chat: FakeBackend::new(station_call)?,
            config: PacketCodecConfig::default(),
            packets: Vec::new(),
        })
    }

    fn connect(&mut self, call: &str) -> Result<Vec<ChatEvent>> {
        Ok(vec![self.chat.connect(call)?])
    }

    fn send_text(&mut self, text: &str) -> Result<Vec<ChatEvent>> {
        let outbound = self.chat.send_text(text)?;
        let transcript = self.chat.transcript();
        let peer = transcript
            .peer_call
            .clone()
            .ok_or_else(|| anyhow::anyhow!("native loopback has no connected peer"))?;
        let payload = format!("MSG {} {}", transcript.station.call_sign, text);
        let outbound_decode = self.roundtrip_packet(MessageDirection::Outbound, &peer, &payload)?;
        if !outbound_decode.ok {
            bail!("native loopback outbound packet failed to decode");
        }

        let ack_text = format!("ack: {text}");
        let ack_payload = format!("MSG {peer} {ack_text}");
        let inbound_decode =
            self.roundtrip_packet(MessageDirection::Inbound, &peer, &ack_payload)?;
        if !inbound_decode.ok {
            bail!("native loopback inbound packet failed to decode");
        }
        let inbound = self.chat.receive_text(&peer, &ack_text)?;
        Ok(vec![outbound, inbound])
    }

    fn receive_text(&mut self, from: &str, text: &str) -> chattybara_chat::Result<ChatEvent> {
        self.chat.receive_text(from, text)
    }

    fn disconnect(&mut self) -> chattybara_chat::Result<ChatEvent> {
        self.chat.disconnect()
    }

    fn status(&self) -> ChatEvent {
        self.chat.status()
    }

    fn transcript(&self) -> ChatTranscript {
        self.chat.transcript()
    }

    fn roundtrip_packet(
        &mut self,
        direction: MessageDirection,
        peer_call: &str,
        payload: &str,
    ) -> Result<PacketDecodeReport> {
        let signal = encode_packet_payload(payload.as_bytes(), self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        let samples = signal.samples.clone();
        let wav_filename = if self.medium == LoopbackMedium::Wav {
            Some(packet_wav_filename(
                self.packets.len() + 1,
                direction,
                peer_call,
            ))
        } else {
            None
        };
        let decode =
            self.decode_loopback_samples(self.packets.len() + 1, direction, peer_call, &samples)?;
        self.packets.push(LoopbackPacketReport {
            sequence: self.packets.len() + 1,
            direction,
            peer_call: peer_call.to_owned(),
            payload_text: payload.to_owned(),
            wav_filename,
            encode: signal.report,
            decode: decode.clone(),
            samples,
        });
        Ok(decode)
    }

    fn decode_loopback_samples(
        &self,
        sequence: usize,
        direction: MessageDirection,
        peer_call: &str,
        samples: &[f32],
    ) -> Result<PacketDecodeReport> {
        match self.medium {
            LoopbackMedium::InMemory => {
                decode_packet_samples(samples, self.config.sample_rate, self.config)
                    .map_err(|error| anyhow::anyhow!(error))
            }
            LoopbackMedium::Wav => {
                let temp_path = std::env::temp_dir().join(format!(
                    "chattybara-tui-{}-{}",
                    std::process::id(),
                    packet_wav_filename(sequence, direction, peer_call)
                ));
                AudioBuffer::new(self.config.sample_rate, 1, samples.to_vec())
                    .with_context(|| "building native WAV loopback buffer")?
                    .write_wav(&temp_path)
                    .with_context(|| format!("writing temporary {}", temp_path.display()))?;
                let wav = AudioBuffer::from_wav(&temp_path)
                    .with_context(|| format!("reading temporary {}", temp_path.display()))?;
                let _ = fs::remove_file(&temp_path);
                decode_packet_samples(&wav.mono_mixdown(), wav.sample_rate, self.config)
                    .map_err(|error| anyhow::anyhow!(error))
            }
        }
    }

    fn write_packet_wavs(&self, dir: &Path) -> Result<Vec<Option<PathBuf>>> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        self.packets
            .iter()
            .map(|packet| {
                let Some(filename) = &packet.wav_filename else {
                    return Ok(None);
                };
                let path = dir.join(filename);
                AudioBuffer::new(self.config.sample_rate, 1, packet.samples.clone())
                    .with_context(|| "building session packet WAV")?
                    .write_wav(&path)
                    .with_context(|| format!("writing {}", path.display()))?;
                Ok(Some(path))
            })
            .collect()
    }

    fn artifacts_report(&self, wav_paths: Option<&[Option<PathBuf>]>) -> serde_json::Value {
        let packets = self
            .packets
            .iter()
            .enumerate()
            .map(|(index, packet)| {
                let wav_path = wav_paths
                    .and_then(|paths| paths.get(index))
                    .and_then(|path| path.as_ref());
                serde_json::json!({
                    "sequence": packet.sequence,
                    "direction": packet.direction,
                    "peer_call": packet.peer_call,
                    "payload_text": packet.payload_text,
                    "wav_filename": packet.wav_filename,
                    "wav_path": wav_path,
                    "encode": packet.encode,
                    "decode": packet.decode,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "kind": "tui-backend-artifacts",
            "backend": self.label,
            "packet_count": self.packets.len(),
            "transcript": self.chat.transcript(),
            "packets": packets,
        })
    }
}

impl TuiSetupState {
    fn from_config(config: ChatTuiSetupConfig) -> Self {
        Self {
            backend: config.backend,
            peer_call: config.peer_call,
            mode: config.mode,
            channel: config.channel,
            audio_input: None,
            audio_output: None,
            audio_sample_rate: 48_000,
            audio_channels: 1,
            hamlib_host: None,
        }
    }

    fn from_runtime(backend: ChatTuiBackend, runtime: &TuiRuntimeProfile) -> Self {
        Self {
            backend,
            peer_call: None,
            mode: None,
            channel: ChannelConfig::default(),
            audio_input: runtime.audio_input.clone(),
            audio_output: runtime.audio_output.clone(),
            audio_sample_rate: runtime.audio_sample_rate,
            audio_channels: runtime.audio_channels,
            hamlib_host: runtime.hamlib_host.clone(),
        }
    }
}

impl Default for TuiRuntimeProfile {
    fn default() -> Self {
        Self {
            audio_input: None,
            audio_output: None,
            audio_sample_rate: 48_000,
            audio_channels: 1,
            hamlib_host: None,
            live_audio: false,
            transmit_armed: false,
        }
    }
}

impl TuiRuntimeProfile {
    fn from_setup(setup: &TuiSetupState) -> Self {
        Self {
            audio_input: setup.audio_input.clone(),
            audio_output: setup.audio_output.clone(),
            audio_sample_rate: setup.audio_sample_rate,
            audio_channels: setup.audio_channels,
            hamlib_host: setup.hamlib_host.clone(),
            live_audio: false,
            transmit_armed: false,
        }
    }

    fn audio_label(&self) -> String {
        format!(
            "{} -> {} @ {} Hz/{}ch",
            self.audio_input.as_deref().unwrap_or("default input"),
            self.audio_output.as_deref().unwrap_or("default output"),
            self.audio_sample_rate,
            self.audio_channels
        )
    }

    fn radio_label(&self) -> String {
        self.hamlib_host
            .as_deref()
            .map(|host| format!("hamlib {host}"))
            .unwrap_or_else(|| "off".to_owned())
    }

    fn safety_label(&self) -> &'static str {
        match (self.live_audio, self.transmit_armed) {
            (true, true) => "LIVE TX ARMED",
            (true, false) => "LIVE RX ONLY",
            (false, _) => "DRY RUN",
        }
    }
}

fn build_tui_backend(
    station_call: &str,
    backend: ChatTuiBackend,
    local_node: Option<ChatTuiLocalNodeConfig>,
) -> Result<(&'static str, TuiBackendState)> {
    match backend {
        ChatTuiBackend::Fake => Ok((
            ChatTuiBackend::Fake.label(),
            TuiBackendState::Fake(FakeBackend::new(station_call)?),
        )),
        ChatTuiBackend::NativeLocalNode => {
            let local_node = local_node.ok_or_else(|| {
                anyhow::anyhow!("native-local-node TUI backend requires peer and local node mode")
            })?;
            Ok((
                ChatTuiBackend::NativeLocalNode.label(),
                TuiBackendState::NativeLocalNode(LocalLiveNode::new(LocalLiveNodeConfig {
                    station: station_call.to_owned(),
                    peer: local_node.peer_call,
                    mode: local_node.mode,
                    channel: local_node.channel,
                })?),
            ))
        }
        ChatTuiBackend::NativeLoopback => Ok((
            ChatTuiBackend::NativeLoopback.label(),
            TuiBackendState::NativeLoopback(NativeLoopbackBackend::new(
                station_call,
                ChatTuiBackend::NativeLoopback.label(),
                LoopbackMedium::InMemory,
            )?),
        )),
        ChatTuiBackend::NativeWavLoopback => Ok((
            ChatTuiBackend::NativeWavLoopback.label(),
            TuiBackendState::NativeWavLoopback(NativeLoopbackBackend::new(
                station_call,
                ChatTuiBackend::NativeWavLoopback.label(),
                LoopbackMedium::Wav,
            )?),
        )),
    }
}

impl ChatTuiApp {
    pub fn new(config: ChatTuiConfig) -> Result<Self> {
        let setup = config.setup.map(TuiSetupState::from_config);
        let starts_in_setup = setup.is_some();
        let initial_backend = if setup.is_some() {
            ChatTuiBackend::NativeLoopback
        } else {
            config.backend
        };
        let (backend_label, backend) =
            build_tui_backend(&config.station_call, initial_backend, config.local_node)?;
        let station = backend.transcript().station.call_sign.clone();
        let mut app = Self {
            station_call: station.clone(),
            backend_kind: initial_backend,
            backend_label,
            backend,
            app_model: ChatAppModel::new(&station)?,
            setup,
            runtime: TuiRuntimeProfile::default(),
            workspace: WorkspaceId::Chat,
            input: String::new(),
            focus: if starts_in_setup {
                TuiPane::Setup
            } else {
                TuiPane::Composer
            },
            composer_mode: ComposerMode::Chat,
            setup_selected: 0,
            mailbox_selected: 0,
            file_offer_selected: 0,
            lines: vec![
                format!("chattybara TUI ready as {station}"),
                "type /help for commands".to_owned(),
            ],
        };
        if app.setup.is_some() {
            app.push_setup_summary();
        }
        Ok(app)
    }

    pub fn apply_line(&mut self, line: &str) -> Result<TuiCommandOutcome> {
        let command = parse_tui_command(line)?;
        self.apply_command(command)
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<TuiCommandOutcome> {
        match code {
            KeyCode::Esc => {
                self.input.clear();
                self.refresh_composer_mode();
                self.focus = if self.setup.is_some() {
                    TuiPane::Setup
                } else {
                    TuiPane::Composer
                };
                Ok(TuiCommandOutcome::Continue)
            }
            KeyCode::F(1) => self.apply_command(TuiCommand::Help),
            KeyCode::Tab => {
                self.focus = self.focus.next();
                Ok(TuiCommandOutcome::Continue)
            }
            KeyCode::BackTab => {
                self.focus = self.focus.previous();
                Ok(TuiCommandOutcome::Continue)
            }
            KeyCode::Up => {
                self.move_selection(-1);
                Ok(TuiCommandOutcome::Continue)
            }
            KeyCode::Down => {
                self.move_selection(1);
                Ok(TuiCommandOutcome::Continue)
            }
            KeyCode::Enter => self.submit_or_activate(),
            KeyCode::Char(character) => {
                if modifiers.contains(KeyModifiers::CONTROL) && matches!(character, 'q' | 'Q') {
                    return Ok(TuiCommandOutcome::Quit);
                }
                if modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(TuiCommandOutcome::Continue);
                }
                if character == '?' && self.input.is_empty() {
                    return self.apply_command(TuiCommand::Help);
                }
                if self.focus != TuiPane::Composer {
                    self.focus = TuiPane::Composer;
                }
                self.input.push(character);
                self.refresh_composer_mode();
                Ok(TuiCommandOutcome::Continue)
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refresh_composer_mode();
                Ok(TuiCommandOutcome::Continue)
            }
            _ => Ok(TuiCommandOutcome::Continue),
        }
    }

    fn submit_or_activate(&mut self) -> Result<TuiCommandOutcome> {
        let line = self.input.trim().to_owned();
        if !line.is_empty() {
            self.input.clear();
            self.refresh_composer_mode();
            return self.apply_line(&line);
        }
        match self.focus {
            TuiPane::Mailbox => {
                if let Some(sequence) = self.selected_mailbox_sequence() {
                    self.apply_command(TuiCommand::MailRead { sequence })
                } else {
                    Ok(TuiCommandOutcome::Continue)
                }
            }
            TuiPane::FileOffers => {
                if let Some(sequence) = self.selected_file_offer_sequence() {
                    self.apply_command(TuiCommand::FileInspect { sequence })
                } else {
                    Ok(TuiCommandOutcome::Continue)
                }
            }
            TuiPane::Setup => self.activate_setup_selection(),
            _ => Ok(TuiCommandOutcome::Continue),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let state = self.app_state();
        match self.focus {
            TuiPane::Setup => {
                self.setup_selected = move_index(self.setup_selected, setup_row_count(self), delta)
            }
            TuiPane::Mailbox => {
                self.mailbox_selected =
                    move_index(self.mailbox_selected, state.mailbox.len(), delta)
            }
            TuiPane::FileOffers => {
                self.file_offer_selected =
                    move_index(self.file_offer_selected, state.file_offers.len(), delta)
            }
            _ => {}
        }
    }

    fn selected_mailbox_sequence(&self) -> Option<u64> {
        let state = self.app_state();
        selected_mailbox_sequence(&state, self.mailbox_selected)
    }

    fn selected_file_offer_sequence(&self) -> Option<u64> {
        let state = self.app_state();
        selected_file_offer_sequence(&state, self.file_offer_selected)
    }

    fn refresh_composer_mode(&mut self) {
        self.composer_mode = if self.input.trim_start().starts_with('/') {
            ComposerMode::Command
        } else {
            ComposerMode::Chat
        };
    }

    fn ensure_setup(&mut self) -> &mut TuiSetupState {
        if self.setup.is_none() {
            self.lines
                .push("setup: opened; current chat backend stays active until /start".to_owned());
            self.setup = Some(TuiSetupState::from_runtime(
                self.backend_kind,
                &self.runtime,
            ));
            self.focus = TuiPane::Setup;
            self.setup_selected = 0;
        }
        self.setup.as_mut().expect("setup state")
    }

    fn push_setup_summary(&mut self) {
        let Some(setup) = &self.setup else {
            self.lines
                .push("setup: inactive; use /setup to change station or backend".to_owned());
            return;
        };
        let peer = setup.peer_call.as_deref().unwrap_or("not set");
        let mode = setup_mode_label(setup.mode.as_ref());
        let audio_input = setup.audio_input.as_deref().unwrap_or("default input");
        let audio_output = setup.audio_output.as_deref().unwrap_or("default output");
        let radio = setup.hamlib_host.as_deref().unwrap_or("off");
        self.lines.extend(
            [
                "setup: bare `chattybara chat tui` starts here with no-hardware loopback active"
                    .to_owned(),
                format!(
                    "setup: station={} selected-backend={} peer={} local-node={}",
                    self.station_call,
                    setup.backend.label(),
                    peer,
                    mode
                ),
                format!(
                    "setup: audio in={} out={} rate={} channels={} radio={}",
                    audio_input, audio_output, setup.audio_sample_rate, setup.audio_channels, radio
                ),
                "setup: /station CALL, /backend fake|native-loopback|native-wav-loopback|native-local-node".to_owned(),
                "setup: /peer CALL, /listen [HOST:PORT], /connect-node HOST:PORT, then /start"
                    .to_owned(),
                "setup: /audio-input NAME, /audio-output NAME, /audio-rate HZ, /radio-hamlib [HOST]".to_owned(),
            ],
        );
    }

    fn set_setup_station(&mut self, call: String) -> Result<TuiCommandOutcome> {
        let call = normalize_tui_call(&call)?;
        self.station_call = call.clone();
        self.ensure_setup();
        self.lines.push(format!("setup: station set to {call}"));
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_backend(&mut self, backend: ChatTuiBackend) -> Result<TuiCommandOutcome> {
        self.ensure_setup().backend = backend;
        self.lines
            .push(format!("setup: selected backend {}", backend.label()));
        if backend == ChatTuiBackend::NativeLocalNode {
            self.lines.push(
                "setup: native-local-node also needs /peer plus /listen or /connect-node"
                    .to_owned(),
            );
        }
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_peer(&mut self, call: String) -> Result<TuiCommandOutcome> {
        let call = normalize_tui_call(&call)?;
        let setup = self.ensure_setup();
        setup.backend = ChatTuiBackend::NativeLocalNode;
        setup.peer_call = Some(call.clone());
        self.lines.push(format!("setup: peer set to {call}"));
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_listen(&mut self, bind: String) -> Result<TuiCommandOutcome> {
        let setup = self.ensure_setup();
        setup.backend = ChatTuiBackend::NativeLocalNode;
        setup.mode = Some(LocalNodeMode::Listen {
            bind: bind.clone(),
            ready_file: Some(default_tui_ready_file()),
        });
        self.lines.push(format!(
            "setup: local node will listen on {bind}; address will be written to {}",
            default_tui_ready_file().display()
        ));
        self.lines
            .push("setup: listener start waits for a peer connection".to_owned());
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_connect_node(&mut self, host: String) -> Result<TuiCommandOutcome> {
        let setup = self.ensure_setup();
        setup.backend = ChatTuiBackend::NativeLocalNode;
        setup.mode = Some(LocalNodeMode::Connect { host: host.clone() });
        self.lines
            .push(format!("setup: local node will connect to {host}"));
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_audio_input(&mut self, device: String) -> Result<TuiCommandOutcome> {
        self.ensure_setup().audio_input = Some(device.clone());
        self.lines
            .push(format!("setup: audio input set to {device}"));
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_audio_output(&mut self, device: String) -> Result<TuiCommandOutcome> {
        self.ensure_setup().audio_output = Some(device.clone());
        self.lines
            .push(format!("setup: audio output set to {device}"));
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_audio_rate(&mut self, sample_rate: u32) -> Result<TuiCommandOutcome> {
        if sample_rate == 0 {
            bail!("audio sample rate must be greater than zero");
        }
        self.ensure_setup().audio_sample_rate = sample_rate;
        self.lines
            .push(format!("setup: audio sample rate set to {sample_rate}"));
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_audio_channels(&mut self, channels: u16) -> Result<TuiCommandOutcome> {
        if channels == 0 {
            bail!("audio channels must be greater than zero");
        }
        self.ensure_setup().audio_channels = channels;
        self.lines
            .push(format!("setup: audio channels set to {channels}"));
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_radio_hamlib(&mut self, host: String) -> Result<TuiCommandOutcome> {
        self.ensure_setup().hamlib_host = Some(host.clone());
        self.lines
            .push(format!("setup: Hamlib rigctld control set to {host}"));
        Ok(TuiCommandOutcome::Continue)
    }

    fn set_setup_radio_off(&mut self) -> Result<TuiCommandOutcome> {
        self.ensure_setup().hamlib_host = None;
        self.lines.push("setup: radio control disabled".to_owned());
        Ok(TuiCommandOutcome::Continue)
    }

    fn seed_command(&mut self, command: &str) -> Result<TuiCommandOutcome> {
        self.input = command.to_owned();
        self.refresh_composer_mode();
        self.focus = TuiPane::Composer;
        Ok(TuiCommandOutcome::Continue)
    }

    fn activate_setup_selection(&mut self) -> Result<TuiCommandOutcome> {
        if self.setup.is_none() {
            return self.apply_command(TuiCommand::Setup);
        }
        match self
            .setup_selected
            .min(setup_row_count(self).saturating_sub(1))
        {
            0 => self.seed_command("/station "),
            1 => {
                let current = self.ensure_setup().backend;
                self.set_setup_backend(next_setup_backend(current))
            }
            2 => self.seed_command("/peer "),
            3 => self.toggle_setup_link_mode(),
            4 => self.seed_command("/audio-input "),
            5 => self.seed_command("/audio-output "),
            6 => {
                if self
                    .setup
                    .as_ref()
                    .and_then(|setup| setup.hamlib_host.as_ref())
                    .is_some()
                {
                    self.set_setup_radio_off()
                } else {
                    self.set_setup_radio_hamlib(DEFAULT_RIGCTLD_HOST.to_owned())
                }
            }
            _ => self.start_setup(),
        }
    }

    fn toggle_setup_link_mode(&mut self) -> Result<TuiCommandOutcome> {
        let setup = self.ensure_setup();
        setup.backend = ChatTuiBackend::NativeLocalNode;
        match setup.mode {
            Some(LocalNodeMode::Listen { .. }) => {
                self.input = "/connect-node ".to_owned();
                self.refresh_composer_mode();
                self.focus = TuiPane::Composer;
                self.lines
                    .push("setup: enter peer node address for connect mode".to_owned());
                Ok(TuiCommandOutcome::Continue)
            }
            Some(LocalNodeMode::Connect { .. }) | None => {
                self.set_setup_listen("127.0.0.1:0".to_owned())
            }
        }
    }

    fn start_setup(&mut self) -> Result<TuiCommandOutcome> {
        let Some(setup) = self.setup.clone() else {
            self.lines.push("setup: already complete".to_owned());
            return Ok(TuiCommandOutcome::Continue);
        };
        let local_node = local_node_config_from_setup(&setup)?;
        let (backend_label, backend) =
            build_tui_backend(&self.station_call, setup.backend, local_node)?;
        let station = backend.transcript().station.call_sign.clone();
        self.station_call = station.clone();
        self.backend_kind = setup.backend;
        self.backend_label = backend_label;
        self.backend = backend;
        self.app_model = ChatAppModel::new(&station)?;
        self.runtime = TuiRuntimeProfile::from_setup(&setup);
        self.setup = None;
        self.focus = TuiPane::Composer;
        self.lines.push(format!(
            "setup complete: {station} using {}",
            self.backend_label
        ));
        self.lines
            .push("type /connect CALL, /cq TEXT, /mail CALL SUBJECT | BODY, or /help".to_owned());
        Ok(TuiCommandOutcome::Continue)
    }

    pub fn apply_command(&mut self, command: TuiCommand) -> Result<TuiCommandOutcome> {
        match command {
            TuiCommand::Connect(call) => {
                let result = self.backend.connect(&call);
                self.record_backend_events(result)
            }
            TuiCommand::Send(text) => {
                let result = self.backend.send_text(&text);
                self.record_backend_events(result)
            }
            TuiCommand::Receive { from, text } => {
                let result = self.backend.receive_text(&from, &text);
                self.record_backend_events(result)
            }
            TuiCommand::Disconnect => {
                let result = self.backend.disconnect();
                self.record_backend_events(result)
            }
            TuiCommand::Beacon(text) => {
                let event = self
                    .app_model
                    .beacon(&text)
                    .map_err(|error| anyhow::anyhow!(error))?;
                self.backend.send_beacon(&text)?;
                self.record_app_event(event);
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::Cq(text) => {
                let event = self
                    .app_model
                    .cq(&text)
                    .map_err(|error| anyhow::anyhow!(error))?;
                self.backend.send_cq(&text)?;
                self.record_app_event(event);
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::Mail { to, subject, body } => {
                let event = self
                    .app_model
                    .mailbox_message(&to, &subject, &body)
                    .map_err(|error| anyhow::anyhow!(error))?;
                self.backend.send_mail(&subject, &body)?;
                self.record_app_event(event);
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::MailRead { sequence } => {
                let message = self.mailbox_message(sequence)?;
                self.lines.push(format!(
                    "mail #{sequence}: {} -> {} | {}",
                    message.from, message.to, message.subject
                ));
                self.lines.push(format!("body: {}", message.body));
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::MailReply {
                sequence,
                subject,
                body,
            } => {
                let message = self.mailbox_message(sequence)?;
                let to = message.from;
                let event = self
                    .app_model
                    .mailbox_message(&to, &subject, &body)
                    .map_err(|error| anyhow::anyhow!(error))?;
                self.backend.send_mail(&subject, &body)?;
                self.record_app_event(event);
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::FileOffer {
                to,
                filename,
                byte_count,
                sha256,
                note,
            } => {
                let event = self
                    .app_model
                    .file_offer(&to, &filename, byte_count, &sha256, note)
                    .map_err(|error| anyhow::anyhow!(error))?;
                let note = match &event {
                    ChatAppEvent::FileOffer(offer) => offer.note.as_deref(),
                    _ => None,
                };
                self.backend
                    .send_file_offer(&filename, byte_count, &sha256, note)?;
                self.record_app_event(event);
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::FileInspect { sequence } => {
                let offer = self.file_offer(sequence)?;
                self.lines.push(format!(
                    "file #{sequence}: {} -> {} {} ({} bytes)",
                    offer.from, offer.to, offer.filename, offer.byte_count
                ));
                self.lines.push(format!("sha256: {}", offer.sha256));
                if let Some(note) = offer.note {
                    self.lines.push(format!("note: {note}"));
                }
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::FileAccept { sequence, out_dir } => {
                let offer = self.file_offer(sequence)?;
                fs::create_dir_all(&out_dir)
                    .with_context(|| format!("creating {}", out_dir.display()))?;
                let receipt_path = out_dir.join(format!(
                    "file-offer-{sequence:03}-{}.json",
                    sanitize_filename(&offer.filename)
                ));
                let receipt = serde_json::json!({
                    "kind": "chattybara-file-offer-receipt",
                    "status": "accepted",
                    "local_station": self.transcript().station.call_sign,
                    "offer": offer,
                });
                fs::write(&receipt_path, serde_json::to_string_pretty(&receipt)?)
                    .with_context(|| format!("writing {}", receipt_path.display()))?;
                self.lines.push(format!(
                    "accepted file offer #{sequence}; wrote {}",
                    receipt_path.display()
                ));
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::AppStatus => {
                let event = self.app_model.status();
                self.record_app_event(event);
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::Status => {
                let event = self.backend.status();
                self.record_event(event);
                self.record_app_event(self.app_model.status());
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::SaveApp(path) => {
                let json = serde_json::to_string_pretty(&self.app_model.state())?;
                fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
                self.lines
                    .push(format!("saved app state to {}", path.display()));
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::SaveLog(path) => {
                let log = normalized_log(&self.backend.transcript());
                fs::write(&path, log).with_context(|| format!("writing {}", path.display()))?;
                self.lines
                    .push(format!("saved normalized log to {}", path.display()));
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::SaveArtifacts(path) => {
                let report = self.backend.artifacts_report();
                let json = serde_json::to_string_pretty(&report)?;
                fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
                self.lines
                    .push(format!("saved backend artifacts to {}", path.display()));
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::SaveSession(path) => {
                let report = self.write_session(&path)?;
                self.lines.push(format!(
                    "saved session with {} packet(s) to {}",
                    report["artifacts"]["packet_count"].as_u64().unwrap_or(0),
                    path.display()
                ));
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::Workspace(workspace) => {
                self.workspace = workspace;
                self.lines.push(format!("workspace: {}", workspace.label()));
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::Setup => {
                self.ensure_setup();
                self.push_setup_summary();
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::SetupStation(call) => self.set_setup_station(call),
            TuiCommand::SetupBackend(backend) => self.set_setup_backend(backend),
            TuiCommand::SetupPeer(call) => self.set_setup_peer(call),
            TuiCommand::SetupListen(bind) => self.set_setup_listen(bind),
            TuiCommand::SetupConnectNode(host) => self.set_setup_connect_node(host),
            TuiCommand::SetupAudioInput(device) => self.set_setup_audio_input(device),
            TuiCommand::SetupAudioOutput(device) => self.set_setup_audio_output(device),
            TuiCommand::SetupAudioRate(sample_rate) => self.set_setup_audio_rate(sample_rate),
            TuiCommand::SetupAudioChannels(channels) => self.set_setup_audio_channels(channels),
            TuiCommand::SetupRadioHamlib(host) => self.set_setup_radio_hamlib(host),
            TuiCommand::SetupRadioOff => self.set_setup_radio_off(),
            TuiCommand::SetupStart => self.start_setup(),
            TuiCommand::Help => {
                self.lines.extend(
                    [
                        "keys: tab/shift-tab panes, up/down select, enter open/start, esc composer, ? help, ctrl-q quit",
                        "topology: chattybara is the TUI chat client; orca is the modem engine",
                        "/setup",
                        "/station CALL",
                        "/backend fake|native-loopback|native-wav-loopback|native-local-node",
                        "/peer CALL",
                        "/listen [HOST:PORT]",
                        "/connect-node HOST:PORT",
                        "/audio-input DEVICE",
                        "/audio-output DEVICE",
                        "/audio-rate HZ",
                        "/audio-channels N",
                        "/radio-hamlib [HOST:PORT]",
                        "/radio-off",
                        "/start",
                        "/connect CALL",
                        "/send TEXT or just type TEXT while connected",
                        "/rx CALL TEXT",
                        "/disconnect",
                        "/beacon TEXT",
                        "/cq TEXT",
                        "/mail CALL SUBJECT | BODY",
                        "/mail-read SEQ",
                        "/mail-reply SEQ SUBJECT | BODY",
                        "/file-offer CALL FILENAME BYTES SHA256 [NOTE]",
                        "/file-inspect SEQ",
                        "/file-accept SEQ DIR",
                        "/status",
                        "/save-app PATH",
                        "/save-log PATH",
                        "/save-artifacts PATH",
                        "/save-session DIR",
                        "/workspace chat|weak-signal|cw-assist|spots|operator-console|rig-setup",
                        "/quit",
                    ]
                    .into_iter()
                    .map(str::to_owned),
                );
                Ok(TuiCommandOutcome::Continue)
            }
            TuiCommand::Quit => Ok(TuiCommandOutcome::Quit),
        }
    }

    fn record_backend_events(
        &mut self,
        result: Result<Vec<ChatEvent>>,
    ) -> Result<TuiCommandOutcome> {
        match result {
            Ok(events) => {
                for event in events {
                    self.record_event(event);
                }
            }
            Err(error) => self.lines.push(format!("error: {error}")),
        }
        Ok(TuiCommandOutcome::Continue)
    }

    fn poll_backend(&mut self) {
        match self.backend.poll_events() {
            Ok(events) => {
                for event in events {
                    match event {
                        TuiBackendEvent::Chat(event) => self.record_event(event),
                        TuiBackendEvent::App(event) => {
                            if let Err(error) = self.record_live_app_event(event) {
                                self.lines.push(format!("error: {error}"));
                            }
                        }
                    }
                }
            }
            Err(error) => self.lines.push(format!("error: {error}")),
        }
    }

    fn record_event(&mut self, event: ChatEvent) {
        match event {
            ChatEvent::Connected {
                local_call,
                remote_call,
            } => self
                .lines
                .push(format!("connected: {local_call} <-> {remote_call}")),
            ChatEvent::Message {
                direction,
                from,
                to,
                text,
                ..
            } => {
                let marker = match direction {
                    MessageDirection::Inbound => "<",
                    MessageDirection::Outbound => ">",
                };
                self.lines.push(format!("{marker} {from} -> {to}: {text}"));
            }
            ChatEvent::Disconnected {
                local_call,
                remote_call,
            } => self.lines.push(format!(
                "disconnected: {local_call} from {}",
                remote_call.as_deref().unwrap_or("none")
            )),
            ChatEvent::Status {
                state,
                local_call,
                remote_call,
                message_count,
            } => self.lines.push(format!(
                "status: {local_call} {state:?} peer={} messages={message_count}",
                remote_call.as_deref().unwrap_or("none")
            )),
        }
    }

    fn record_app_event(&mut self, event: ChatAppEvent) {
        match event {
            ChatAppEvent::Beacon(post) => self
                .lines
                .push(format!("beacon: {} {}", post.from, post.text)),
            ChatAppEvent::Cq(call) => self.lines.push(format!("cq: {} {}", call.from, call.text)),
            ChatAppEvent::MailboxMessage(message) => self.lines.push(format!(
                "mail: {} -> {} | {}",
                message.from, message.to, message.subject
            )),
            ChatAppEvent::FileOffer(offer) => self.lines.push(format!(
                "file offer: {} -> {} {} ({} bytes)",
                offer.from, offer.to, offer.filename, offer.byte_count
            )),
            ChatAppEvent::Status {
                station_call,
                beacon_count,
                cq_count,
                mailbox_count,
                file_offer_count,
            } => self.lines.push(format!(
                "app: {station_call} beacons={beacon_count} cq={cq_count} mail={mailbox_count} files={file_offer_count}"
            )),
        }
    }

    fn record_live_app_event(&mut self, event: LocalLiveAppEvent) -> Result<()> {
        let event = match event {
            LocalLiveAppEvent::Beacon { from, text, .. } => self
                .app_model
                .observe_beacon(&from, &text)
                .map_err(|error| anyhow::anyhow!(error))?,
            LocalLiveAppEvent::Cq { from, text, .. } => self
                .app_model
                .observe_cq(&from, &text)
                .map_err(|error| anyhow::anyhow!(error))?,
            LocalLiveAppEvent::Mail {
                from,
                to,
                subject,
                body,
            } => self
                .app_model
                .receive_mailbox_message(&from, &to, &subject, &body)
                .map_err(|error| anyhow::anyhow!(error))?,
            LocalLiveAppEvent::FileOffer {
                from,
                to,
                filename,
                byte_count,
                sha256,
                note,
            } => self
                .app_model
                .receive_file_offer(&from, &to, &filename, byte_count, &sha256, note)
                .map_err(|error| anyhow::anyhow!(error))?,
        };
        self.record_app_event(event);
        Ok(())
    }

    fn transcript(&self) -> ChatTranscript {
        self.backend.transcript()
    }

    fn app_state(&self) -> ChatAppState {
        self.app_model.state()
    }

    fn mailbox_message(&self, sequence: u64) -> Result<MailboxMessage> {
        self.app_state()
            .mailbox
            .into_iter()
            .find(|message| message.sequence == sequence)
            .ok_or_else(|| anyhow::anyhow!("mailbox message #{sequence} not found"))
    }

    fn file_offer(&self, sequence: u64) -> Result<FileOffer> {
        self.app_state()
            .file_offers
            .into_iter()
            .find(|offer| offer.sequence == sequence)
            .ok_or_else(|| anyhow::anyhow!("file offer #{sequence} not found"))
    }

    fn status_text(&self) -> String {
        let transcript = self.transcript();
        let app_state = self.app_state();
        let station_call = if self.setup.is_some() {
            self.station_call.as_str()
        } else {
            transcript.station.call_sign.as_str()
        };
        let setup_label = if let Some(setup) = &self.setup {
            format!(" | setup {}", setup.backend.label())
        } else {
            String::new()
        };
        format!(
            "{} | backend {}{} | {} | peer {} | {} | radio {} | focus {} | mode {} | msg {} | b:{} cq:{} m:{} f:{}",
            station_call,
            self.backend_label,
            setup_label,
            state_label(transcript.state),
            transcript.peer_call.as_deref().unwrap_or("none"),
            self.runtime.safety_label(),
            self.runtime.radio_label(),
            self.focus.label(),
            format_args!(
                "{} workspace {}",
                self.composer_mode.label(),
                self.workspace.label()
            ),
            transcript.messages.len(),
            app_state.beacons.len(),
            app_state.cq_calls.len(),
            app_state.mailbox.len(),
            app_state.file_offers.len()
        )
    }

    fn runtime_report(&self) -> serde_json::Value {
        serde_json::json!({
            "audio": {
                "input_device": self.runtime.audio_input.clone(),
                "output_device": self.runtime.audio_output.clone(),
                "sample_rate": self.runtime.audio_sample_rate,
                "channels": self.runtime.audio_channels,
            },
            "radio": {
                "control": if self.runtime.hamlib_host.is_some() { "hamlib" } else { "off" },
                "hamlib_host": self.runtime.hamlib_host.clone(),
            },
            "safety": {
                "live_audio": self.runtime.live_audio,
                "transmit_armed": self.runtime.transmit_armed,
                "label": self.runtime.safety_label(),
            }
        })
    }

    fn write_session(&self, dir: &Path) -> Result<serde_json::Value> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let transcript = self.transcript();
        let transcript_path = dir.join("transcript.json");
        let app_state_path = dir.join("app-state.json");
        let events_path = dir.join("events.jsonl");
        let log_path = dir.join("chat.log");
        let packets_dir = dir.join("packets");
        let artifacts_path = dir.join("artifacts.json");
        let session_path = dir.join("session.json");

        fs::write(&transcript_path, serde_json::to_string_pretty(&transcript)?)
            .with_context(|| format!("writing {}", transcript_path.display()))?;
        fs::write(
            &app_state_path,
            serde_json::to_string_pretty(&self.app_state())?,
        )
        .with_context(|| format!("writing {}", app_state_path.display()))?;
        fs::write(&log_path, normalized_log(&transcript))
            .with_context(|| format!("writing {}", log_path.display()))?;
        write_event_log(&events_path, &self.station_records())
            .with_context(|| format!("writing {}", events_path.display()))?;
        let packet_wavs = self.backend.write_packet_wavs(&packets_dir)?;
        let artifacts = match &self.backend {
            TuiBackendState::Fake(_) => self.backend.artifacts_report(),
            TuiBackendState::NativeLocalNode(backend) => {
                backend.artifacts_report(Some(&packet_wavs))
            }
            TuiBackendState::NativeLoopback(backend)
            | TuiBackendState::NativeWavLoopback(backend) => {
                backend.artifacts_report(Some(&packet_wavs))
            }
        };
        fs::write(&artifacts_path, serde_json::to_string_pretty(&artifacts)?)
            .with_context(|| format!("writing {}", artifacts_path.display()))?;
        let report = serde_json::json!({
            "kind": "tui-session-report",
            "backend": self.backend_label,
            "profile": self.runtime_report(),
            "workspace": self.workspace.label(),
            "directory": dir,
            "transcript": transcript_path,
            "events": events_path,
            "app_state": app_state_path,
            "log": log_path,
            "artifacts": artifacts,
            "artifacts_path": artifacts_path,
            "session_path": session_path,
        });
        fs::write(&session_path, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("writing {}", session_path.display()))?;
        Ok(report)
    }

    fn station_records(&self) -> Vec<StationLogRecord> {
        let transcript = self.transcript();
        let app_state = self.app_state();
        let mut sequence = 1_u64;
        let mut records = Vec::new();
        for message in transcript.messages {
            records.push(StationLogRecord {
                sequence,
                event: StationEvent::ChatMessage(StationChatMessageEvent {
                    mode: ModeId::OrcaChat,
                    sequence: message.sequence,
                    from: message.from,
                    to: message.to,
                    text: message.text,
                }),
            });
            sequence += 1;
        }
        for message in app_state.mailbox {
            records.push(StationLogRecord {
                sequence,
                event: StationEvent::MailMessage(StationMailMessageEvent {
                    mode: ModeId::OrcaChat,
                    message_id: format!("mail-{:03}", message.sequence),
                    from: message.from,
                    to: message.to,
                    subject: message.subject,
                    body: message.body,
                }),
            });
            sequence += 1;
        }
        for offer in app_state.file_offers {
            records.push(StationLogRecord {
                sequence,
                event: StationEvent::FileOffer(StationFileOfferEvent {
                    mode: ModeId::OrcaChat,
                    offer_id: format!("file-{:03}", offer.sequence),
                    from: offer.from,
                    to: offer.to,
                    filename: offer.filename,
                    byte_count: offer.byte_count,
                    sha256: offer.sha256,
                }),
            });
            sequence += 1;
        }
        records
    }
}

pub fn run_chat_tui(config: ChatTuiConfig) -> Result<()> {
    let app = ChatTuiApp::new(config)?;
    let mut terminal = setup_terminal()?;
    let result = run_terminal_app(&mut terminal, app);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enabling terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("creating terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("disabling terminal raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leaving alternate screen")?;
    terminal.show_cursor().context("showing cursor")
}

fn run_terminal_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: ChatTuiApp,
) -> Result<()> {
    loop {
        app.poll_backend();
        terminal
            .draw(|frame| draw(frame, &app))
            .context("drawing chat TUI")?;
        if !event::poll(Duration::from_millis(200)).context("polling terminal events")? {
            continue;
        }
        let Event::Key(key) = event::read().context("reading terminal event")? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            break;
        }
        match app.handle_key(key.code, key.modifiers) {
            Ok(TuiCommandOutcome::Continue) => {}
            Ok(TuiCommandOutcome::Quit) => break,
            Err(error) => app.lines.push(format!("error: {error}")),
        }
    }
    Ok(())
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &ChatTuiApp) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let status = Paragraph::new(app.status_text()).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(status, layout[0]);

    let (main_direction, main_constraints) = if layout[1].width < 100 {
        (
            Direction::Vertical,
            [Constraint::Percentage(58), Constraint::Percentage(42)],
        )
    } else {
        (
            Direction::Horizontal,
            [Constraint::Percentage(60), Constraint::Percentage(40)],
        )
    };
    let main = Layout::default()
        .direction(main_direction)
        .constraints(main_constraints)
        .split(layout[1]);

    let visible_rows = main[0].height.saturating_sub(2) as usize;
    let start = app.lines.len().saturating_sub(visible_rows);
    let items = app.lines[start..]
        .iter()
        .map(|line| ListItem::new(line.as_str()))
        .collect::<Vec<_>>();
    let transcript =
        List::new(items).block(pane_block("transcript", TuiPane::Transcript, app.focus));
    frame.render_widget(transcript, main[0]);

    let app_direction = if main[1].height < 10 && main[1].width >= 72 {
        Direction::Horizontal
    } else {
        Direction::Vertical
    };
    let app_panes = Layout::default()
        .direction(app_direction)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ])
        .split(main[1]);
    let app_state = app.app_state();
    let setup = List::new(setup_status_items(app, app_panes[0].height)).block(pane_block(
        "setup / radio",
        TuiPane::Setup,
        app.focus,
    ));
    frame.render_widget(setup, app_panes[0]);
    let monitor = List::new(app_monitor_items(&app_state, app_panes[1].height)).block(pane_block(
        "beacon / cq monitor",
        TuiPane::Monitor,
        app.focus,
    ));
    frame.render_widget(monitor, app_panes[1]);
    let mailbox = List::new(mailbox_items(
        &app_state,
        app_panes[2].height,
        selected_mailbox_sequence(&app_state, app.mailbox_selected),
    ))
    .block(pane_block("mailbox", TuiPane::Mailbox, app.focus));
    frame.render_widget(mailbox, app_panes[2]);
    let file_offers = List::new(file_offer_items(
        &app_state,
        app_panes[3].height,
        selected_file_offer_sequence(&app_state, app.file_offer_selected),
    ))
    .block(pane_block("file offers", TuiPane::FileOffers, app.focus));
    frame.render_widget(file_offers, app_panes[3]);

    let input = Paragraph::new(app.input.as_str()).block(
        Block::default()
            .title(format!("input [{}]", app.composer_mode.label()))
            .borders(Borders::ALL)
            .border_style(if app.focus == TuiPane::Composer {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            }),
    );
    frame.render_widget(input, layout[2]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled("? help", Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled("tab panes", Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled("enter", Style::default().fg(Color::Cyan)),
        Span::raw(" "),
        Span::raw(context_help(app)),
        Span::raw("  "),
        Span::styled("ctrl-q quit", Style::default().fg(Color::Cyan)),
    ]))
    .wrap(Wrap { trim: true });
    frame.render_widget(help, layout[3]);

    if layout[2].width > 1 && layout[2].height > 1 {
        let input_width = app.input.len().min(u16::MAX as usize) as u16;
        let cursor_x = layout[2]
            .x
            .saturating_add(input_width)
            .saturating_add(1)
            .min(layout[2].right().saturating_sub(1));
        let cursor_y = layout[2]
            .y
            .saturating_add(1)
            .min(layout[2].bottom().saturating_sub(1));
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn pane_block<'a>(title: &'a str, pane: TuiPane, focus: TuiPane) -> Block<'a> {
    let style = if pane == focus {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Block::default()
        .title(format!("{title}{}", if pane == focus { " *" } else { "" }))
        .borders(Borders::ALL)
        .border_style(style)
}

fn selected_mailbox_sequence(state: &ChatAppState, selected: usize) -> Option<u64> {
    state
        .mailbox
        .get(selected.min(state.mailbox.len().saturating_sub(1)))
        .map(|message| message.sequence)
}

fn selected_file_offer_sequence(state: &ChatAppState, selected: usize) -> Option<u64> {
    state
        .file_offers
        .get(selected.min(state.file_offers.len().saturating_sub(1)))
        .map(|offer| offer.sequence)
}

fn move_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let last = len - 1;
    if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs()).min(last)
    } else {
        current.saturating_add(delta as usize).min(last)
    }
}

fn setup_status_items(app: &ChatTuiApp, height: u16) -> Vec<ListItem<'static>> {
    let mut rows = Vec::new();
    if let Some(setup) = &app.setup {
        rows.push((
            app.setup_selected == 0,
            format!("{} station {}", checklist(true), app.station_call),
        ));
        rows.push((
            app.setup_selected == 1,
            format!(
                "{} backend {}",
                checklist(true),
                setup_backend_pane_label(setup.backend)
            ),
        ));
        if setup.backend == ChatTuiBackend::NativeLocalNode {
            rows.push((
                app.setup_selected == 2,
                format!(
                    "{} peer {}",
                    checklist(setup.peer_call.is_some()),
                    setup.peer_call.as_deref().unwrap_or("not set")
                ),
            ));
            rows.push((
                app.setup_selected == 3,
                format!(
                    "{} node {}",
                    checklist(setup.mode.is_some()),
                    setup_mode_label(setup.mode.as_ref())
                ),
            ));
        } else {
            rows.push((
                app.setup_selected == 2,
                format!("{} peer not required", checklist(true)),
            ));
            rows.push((
                app.setup_selected == 3,
                format!("{} node not required", checklist(true)),
            ));
        }
        rows.push((
            app.setup_selected == 4,
            format!(
                "{} audio in {}",
                checklist(true),
                setup.audio_input.as_deref().unwrap_or("default input")
            ),
        ));
        rows.push((
            app.setup_selected == 5,
            format!(
                "{} audio out {}",
                checklist(true),
                setup.audio_output.as_deref().unwrap_or("default output")
            ),
        ));
        rows.push((
            app.setup_selected == 6,
            format!(
                "{} radio {}",
                checklist(true),
                setup
                    .hamlib_host
                    .as_deref()
                    .map(|host| format!("rig {host}"))
                    .unwrap_or_else(|| "off".to_owned())
            ),
        ));
        rows.push((
            app.setup_selected == 7,
            format!("{} start | safety DRY RUN", checklist(true)),
        ));
        top_styled_items(rows, height, "setup unavailable")
    } else {
        rows.push((false, "session ready".to_owned()));
        rows.push((false, format!("backend {}", app.backend_label)));
        rows.push((false, format!("workspace {}", app.workspace.label())));
        rows.push((
            false,
            format!("engine {}", backend_engine_label(app.backend_kind)),
        ));
        rows.push((false, format!("audio {}", app.runtime.audio_label())));
        rows.push((false, format!("radio {}", app.runtime.radio_label())));
        rows.push((false, format!("safety {}", app.runtime.safety_label())));
        rows.push((false, "/setup reopens setup".to_owned()));
        top_styled_items(rows, height, "setup unavailable")
    }
}

fn checklist(done: bool) -> &'static str {
    if done { "[x]" } else { "[ ]" }
}

fn setup_row_count(app: &ChatTuiApp) -> usize {
    if app.setup.is_some() { 8 } else { 1 }
}

fn setup_backend_pane_label(backend: ChatTuiBackend) -> &'static str {
    match backend {
        ChatTuiBackend::Fake => "fake",
        ChatTuiBackend::NativeLocalNode => "local-node",
        ChatTuiBackend::NativeLoopback => "loopback",
        ChatTuiBackend::NativeWavLoopback => "wav-loopback",
    }
}

fn next_setup_backend(backend: ChatTuiBackend) -> ChatTuiBackend {
    match backend {
        ChatTuiBackend::Fake => ChatTuiBackend::NativeLoopback,
        ChatTuiBackend::NativeLoopback => ChatTuiBackend::NativeWavLoopback,
        ChatTuiBackend::NativeWavLoopback => ChatTuiBackend::NativeLocalNode,
        ChatTuiBackend::NativeLocalNode => ChatTuiBackend::Fake,
    }
}

fn backend_engine_label(backend: ChatTuiBackend) -> &'static str {
    match backend {
        ChatTuiBackend::Fake => "fake app state",
        ChatTuiBackend::NativeLocalNode => "orca local node",
        ChatTuiBackend::NativeLoopback => "orca loopback",
        ChatTuiBackend::NativeWavLoopback => "orca wav loopback",
    }
}

fn context_help(app: &ChatTuiApp) -> &'static str {
    match app.focus {
        TuiPane::Setup if app.setup.is_some() => "start setup or type /station /backend /peer",
        TuiPane::Setup => "review backend/audio/radio safety",
        TuiPane::Transcript => "read transcript; type to send",
        TuiPane::Monitor => "watch beacon and CQ traffic",
        TuiPane::Mailbox => "up/down select; enter read; /mail-reply reply",
        TuiPane::FileOffers => "up/down select; enter inspect; /file-accept save receipt",
        TuiPane::Composer => "type message or slash command",
    }
}

fn app_monitor_items(state: &ChatAppState, height: u16) -> Vec<ListItem<'static>> {
    let mut rows = state
        .beacons
        .iter()
        .map(|beacon| {
            (
                beacon.sequence,
                format!("#{} BCN {} {}", beacon.sequence, beacon.from, beacon.text),
            )
        })
        .chain(state.cq_calls.iter().map(|cq| {
            (
                cq.sequence,
                format!("#{} CQ {} {}", cq.sequence, cq.from, cq.text),
            )
        }))
        .collect::<Vec<_>>();
    rows.sort_by_key(|(sequence, _)| *sequence);
    recent_items(
        rows.into_iter().map(|(_, line)| line).collect(),
        height,
        "no beacon/CQ traffic",
    )
}

fn mailbox_items(
    state: &ChatAppState,
    height: u16,
    selected_sequence: Option<u64>,
) -> Vec<ListItem<'static>> {
    recent_styled_items(
        state
            .mailbox
            .iter()
            .map(|message| {
                let selected = Some(message.sequence) == selected_sequence;
                let line = format!(
                    "#{} {} -> {} | {}",
                    message.sequence, message.from, message.to, message.subject
                );
                (selected, line)
            })
            .collect(),
        height,
        "no mailbox traffic",
    )
}

fn file_offer_items(
    state: &ChatAppState,
    height: u16,
    selected_sequence: Option<u64>,
) -> Vec<ListItem<'static>> {
    recent_styled_items(
        state
            .file_offers
            .iter()
            .map(|offer| {
                let selected = Some(offer.sequence) == selected_sequence;
                let note = offer
                    .note
                    .as_deref()
                    .map(|value| format!(" | {value}"))
                    .unwrap_or_default();
                let line = format!(
                    "#{} {} -> {} | {} ({} bytes){}",
                    offer.sequence, offer.from, offer.to, offer.filename, offer.byte_count, note
                );
                (selected, line)
            })
            .collect(),
        height,
        "no file offers",
    )
}

fn recent_items(lines: Vec<String>, height: u16, empty: &str) -> Vec<ListItem<'static>> {
    let visible_rows = height.saturating_sub(2) as usize;
    if lines.is_empty() {
        return vec![ListItem::new(empty.to_owned())];
    }
    let start = lines.len().saturating_sub(visible_rows.max(1));
    lines.into_iter().skip(start).map(ListItem::new).collect()
}

fn top_styled_items(
    lines: Vec<(bool, String)>,
    height: u16,
    empty: &str,
) -> Vec<ListItem<'static>> {
    let visible_rows = height.saturating_sub(2).max(1) as usize;
    if lines.is_empty() {
        return vec![ListItem::new(empty.to_owned())];
    }
    let selected_index = lines
        .iter()
        .position(|(selected, _)| *selected)
        .unwrap_or(0);
    let start = if selected_index >= visible_rows {
        selected_index + 1 - visible_rows
    } else {
        0
    }
    .min(lines.len().saturating_sub(visible_rows));
    lines
        .into_iter()
        .skip(start)
        .take(visible_rows)
        .map(|(selected, line)| {
            if selected {
                ListItem::new(format!("> {line}")).style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ListItem::new(format!("  {line}"))
            }
        })
        .collect()
}

fn recent_styled_items(
    lines: Vec<(bool, String)>,
    height: u16,
    empty: &str,
) -> Vec<ListItem<'static>> {
    let visible_rows = height.saturating_sub(2) as usize;
    if lines.is_empty() {
        return vec![ListItem::new(empty.to_owned())];
    }
    let start = lines.len().saturating_sub(visible_rows.max(1));
    lines
        .into_iter()
        .skip(start)
        .map(|(selected, line)| {
            if selected {
                ListItem::new(format!("> {line}")).style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ListItem::new(format!("  {line}"))
            }
        })
        .collect()
}

pub fn parse_tui_command(line: &str) -> Result<TuiCommand> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        bail!("empty TUI command");
    }
    if !trimmed.starts_with('/') {
        return Ok(TuiCommand::Send(trimmed.to_owned()));
    }

    let mut parts = trimmed[1..].splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or_default().to_ascii_lowercase();
    let rest = parts.next().unwrap_or_default().trim();
    match verb.as_str() {
        "setup" => Ok(TuiCommand::Setup),
        "station" | "call" | "callsign" | "call-sign" => {
            Ok(TuiCommand::SetupStation(required(rest, &verb)?))
        }
        "backend" => Ok(TuiCommand::SetupBackend(parse_tui_backend_label(
            required(rest, &verb)?.as_str(),
            &verb,
        )?)),
        "peer" => Ok(TuiCommand::SetupPeer(required(rest, &verb)?)),
        "listen" => Ok(TuiCommand::SetupListen(if rest.is_empty() {
            "127.0.0.1:0".to_owned()
        } else {
            rest.to_owned()
        })),
        "connect-node" | "connect_node" | "node-connect" | "node_connect" => {
            Ok(TuiCommand::SetupConnectNode(required(rest, &verb)?))
        }
        "audio-input" | "audio_input" | "input-device" | "input_device" => {
            Ok(TuiCommand::SetupAudioInput(required(rest, &verb)?))
        }
        "audio-output" | "audio_output" | "output-device" | "output_device" => {
            Ok(TuiCommand::SetupAudioOutput(required(rest, &verb)?))
        }
        "audio-rate" | "audio_rate" | "sample-rate" | "sample_rate" => Ok(
            TuiCommand::SetupAudioRate(parse_u32_arg(required(rest, &verb)?.as_str(), &verb)?),
        ),
        "audio-channels" | "audio_channels" | "channels" => Ok(TuiCommand::SetupAudioChannels(
            parse_u16_arg(required(rest, &verb)?.as_str(), &verb)?,
        )),
        "radio-hamlib" | "radio_hamlib" | "hamlib" => {
            Ok(TuiCommand::SetupRadioHamlib(if rest.is_empty() {
                DEFAULT_RIGCTLD_HOST.to_owned()
            } else {
                rest.to_owned()
            }))
        }
        "radio-off" | "radio_off" => Ok(TuiCommand::SetupRadioOff),
        "start" => Ok(TuiCommand::SetupStart),
        "connect" | "c" => Ok(TuiCommand::Connect(required(rest, &verb)?)),
        "send" | "s" => Ok(TuiCommand::Send(required(rest, &verb)?)),
        "rx" | "recv" | "receive" => {
            let (from, text) = split_two(rest, &verb)?;
            Ok(TuiCommand::Receive { from, text })
        }
        "disconnect" | "disc" | "d" => Ok(TuiCommand::Disconnect),
        "beacon" | "bcn" => Ok(TuiCommand::Beacon(required(rest, &verb)?)),
        "cq" => Ok(TuiCommand::Cq(required(rest, &verb)?)),
        "mail" | "mailbox" | "vmail" => {
            let (to, subject, body) = split_mailbox(rest, &verb)?;
            Ok(TuiCommand::Mail { to, subject, body })
        }
        "mail-read" | "mail_read" | "read-mail" | "read_mail" => Ok(TuiCommand::MailRead {
            sequence: parse_sequence(required(rest, &verb)?.as_str(), &verb)?,
        }),
        "mail-reply" | "mail_reply" | "reply-mail" | "reply_mail" => {
            let (sequence, subject, body) = split_sequence_mailbox(rest, &verb)?;
            Ok(TuiCommand::MailReply {
                sequence,
                subject,
                body,
            })
        }
        "file-offer" | "file_offer" | "file" => {
            let (to, filename, byte_count, sha256, note) = split_file_offer(rest, &verb)?;
            Ok(TuiCommand::FileOffer {
                to,
                filename,
                byte_count,
                sha256,
                note,
            })
        }
        "file-inspect" | "file_inspect" | "file-open" | "file_open" => {
            Ok(TuiCommand::FileInspect {
                sequence: parse_sequence(required(rest, &verb)?.as_str(), &verb)?,
            })
        }
        "file-accept" | "file_accept" | "accept-file" | "accept_file" => {
            let (sequence, path) = split_sequence_path(rest, &verb)?;
            Ok(TuiCommand::FileAccept {
                sequence,
                out_dir: path,
            })
        }
        "app-status" | "app_status" => Ok(TuiCommand::AppStatus),
        "status" | "state" => Ok(TuiCommand::Status),
        "save-app" | "save_app" => Ok(TuiCommand::SaveApp(PathBuf::from(required(rest, &verb)?))),
        "save-log" | "save_log" => Ok(TuiCommand::SaveLog(PathBuf::from(required(rest, &verb)?))),
        "save-artifacts" | "save_artifacts" => Ok(TuiCommand::SaveArtifacts(PathBuf::from(
            required(rest, &verb)?,
        ))),
        "save-session" | "save_session" => Ok(TuiCommand::SaveSession(PathBuf::from(required(
            rest, &verb,
        )?))),
        "workspace" | "ws" => Ok(TuiCommand::Workspace(parse_workspace_label(
            &required(rest, &verb)?,
            &verb,
        )?)),
        "help" | "h" | "?" => Ok(TuiCommand::Help),
        "quit" | "q" | "exit" => Ok(TuiCommand::Quit),
        _ => bail!("unknown TUI command: /{verb}"),
    }
}

fn parse_tui_backend_label(value: &str, verb: &str) -> Result<ChatTuiBackend> {
    ChatTuiBackend::from_label(value).ok_or_else(|| {
        anyhow::anyhow!(
            "invalid backend for /{verb}: expected fake, native-loopback, native-wav-loopback, or native-local-node"
        )
    })
}

fn parse_workspace_label(value: &str, verb: &str) -> Result<WorkspaceId> {
    match value.trim() {
        "chat" => Ok(WorkspaceId::Chat),
        "weak-signal" | "weak_signal" | "ft8" | "wsjtx" => Ok(WorkspaceId::WeakSignal),
        "cw-assist" | "cw_assist" | "cw" | "morse" => Ok(WorkspaceId::CwAssist),
        "spots" | "spot" | "pskreporter" | "psk-reporter" => Ok(WorkspaceId::Spots),
        "operator-console" | "operator_console" | "console" | "fldigi" => {
            Ok(WorkspaceId::OperatorConsole)
        }
        "rig-setup" | "rig_setup" | "setup" | "radio" => Ok(WorkspaceId::RigSetup),
        _ => bail!(
            "invalid workspace for /{verb}: expected chat, weak-signal, cw-assist, spots, operator-console, or rig-setup"
        ),
    }
}

fn normalize_tui_call(value: &str) -> Result<String> {
    Ok(FakeBackend::new(value)?.transcript().station.call_sign)
}

fn local_node_config_from_setup(setup: &TuiSetupState) -> Result<Option<ChatTuiLocalNodeConfig>> {
    if setup.backend != ChatTuiBackend::NativeLocalNode {
        return Ok(None);
    }
    let peer_call = setup.peer_call.clone().ok_or_else(|| {
        anyhow::anyhow!("setup needs /peer CALL before /start with native-local-node")
    })?;
    let mode = setup.mode.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "setup needs /listen [HOST:PORT] or /connect-node HOST:PORT before /start with native-local-node"
        )
    })?;
    Ok(Some(ChatTuiLocalNodeConfig {
        peer_call,
        mode,
        channel: setup.channel,
    }))
}

fn setup_mode_label(mode: Option<&LocalNodeMode>) -> String {
    match mode {
        Some(LocalNodeMode::Listen { bind, ready_file }) => {
            let ready = ready_file
                .as_ref()
                .map(|path| format!(" ready-file={}", path.display()))
                .unwrap_or_default();
            format!("listen {bind}{ready}")
        }
        Some(LocalNodeMode::Connect { host }) => format!("connect {host}"),
        None => "not set".to_owned(),
    }
}

fn default_tui_ready_file() -> PathBuf {
    PathBuf::from("out/chattybara-tui.ready")
}

fn normalized_log(transcript: &ChatTranscript) -> String {
    transcript
        .messages
        .iter()
        .map(|message| normalized_log_line(&transcript.station.call_sign, message))
        .collect::<Vec<_>>()
        .join("")
}

fn normalized_log_line(local_call: &str, message: &ChatMessage) -> String {
    match message.direction {
        MessageDirection::Inbound => format!("IN {} {}\n", message.from, message.text),
        MessageDirection::Outbound => {
            let to = if message.to == local_call {
                message.from.as_str()
            } else {
                message.to.as_str()
            };
            format!("OUT {to} {}\n", message.text)
        }
    }
}

fn packet_wav_filename(sequence: usize, direction: MessageDirection, peer_call: &str) -> String {
    format!(
        "packet-{sequence:03}-{}-{}.wav",
        direction_label(direction),
        sanitize_filename(peer_call)
    )
}

fn direction_label(direction: MessageDirection) -> &'static str {
    match direction {
        MessageDirection::Inbound => "inbound",
        MessageDirection::Outbound => "outbound",
    }
}

fn sanitize_filename(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn required(rest: &str, verb: &str) -> Result<String> {
    if rest.is_empty() {
        bail!("missing argument for /{verb}")
    }
    Ok(rest.to_owned())
}

fn split_two(rest: &str, verb: &str) -> Result<(String, String)> {
    let rest = required(rest, verb)?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default().trim();
    let second = parts.next().unwrap_or_default().trim();
    if first.is_empty() || second.is_empty() {
        bail!("missing argument for /{verb}")
    }
    Ok((first.to_owned(), second.to_owned()))
}

fn split_mailbox(rest: &str, verb: &str) -> Result<(String, String, String)> {
    let (to, subject_and_body) = split_two(rest, verb)?;
    let Some((subject, body)) = subject_and_body.split_once('|') else {
        bail!("missing argument for /{verb}: expected CALL SUBJECT | BODY")
    };
    let subject = subject.trim();
    let body = body.trim();
    if subject.is_empty() || body.is_empty() {
        bail!("missing argument for /{verb}: expected CALL SUBJECT | BODY")
    }
    Ok((to, subject.to_owned(), body.to_owned()))
}

fn split_sequence_mailbox(rest: &str, verb: &str) -> Result<(u64, String, String)> {
    let (sequence, subject_and_body) = split_two(rest, verb)?;
    let Some((subject, body)) = subject_and_body.split_once('|') else {
        bail!("missing argument for /{verb}: expected SEQ SUBJECT | BODY")
    };
    let subject = subject.trim();
    let body = body.trim();
    if subject.is_empty() || body.is_empty() {
        bail!("missing argument for /{verb}: expected SEQ SUBJECT | BODY")
    }
    Ok((
        parse_sequence(&sequence, verb)?,
        subject.to_owned(),
        body.to_owned(),
    ))
}

fn split_sequence_path(rest: &str, verb: &str) -> Result<(u64, PathBuf)> {
    let (sequence, path) = split_two(rest, verb)?;
    Ok((parse_sequence(&sequence, verb)?, PathBuf::from(path)))
}

fn parse_sequence(value: &str, verb: &str) -> Result<u64> {
    let sequence = value
        .trim_start_matches('#')
        .parse::<u64>()
        .with_context(|| format!("invalid sequence for /{verb}: {value}"))?;
    if sequence == 0 {
        bail!("invalid sequence for /{verb}: sequence starts at 1");
    }
    Ok(sequence)
}

fn parse_u32_arg(value: &str, verb: &str) -> Result<u32> {
    value
        .parse::<u32>()
        .with_context(|| format!("invalid integer for /{verb}: {value}"))
}

fn parse_u16_arg(value: &str, verb: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .with_context(|| format!("invalid integer for /{verb}: {value}"))
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
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if to.is_empty() || filename.is_empty() || byte_count.is_empty() || sha256.is_empty() {
        bail!("missing argument for /{verb}: expected CALL FILENAME BYTES SHA256 [NOTE]")
    }
    let byte_count = byte_count
        .parse::<u64>()
        .with_context(|| format!("invalid byte count for /{verb}: {byte_count}"))?;
    Ok((
        to.to_owned(),
        filename.to_owned(),
        byte_count,
        sha256.to_owned(),
        note,
    ))
}

fn state_label(state: ChatState) -> &'static str {
    match state {
        ChatState::Idle => "idle",
        ChatState::Connected => "connected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    const TEST_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn render_app_lines(app: &ChatTuiApp, width: u16, height: u16) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, app)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect()
    }

    fn render_app_text(app: &ChatTuiApp, width: u16, height: u16) -> String {
        render_app_lines(app, width, height).join("\n")
    }

    fn visual_app() -> ChatTuiApp {
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "ja1tst".to_owned(),
            backend: ChatTuiBackend::Fake,
            local_node: None,
            setup: None,
        })
        .expect("app");
        app.apply_line("/connect ja1qso").expect("connect");
        for index in 0..14 {
            app.apply_line(&format!(
                "visual layout transcript message {index:02} with enough words to exercise clipping and wrapping"
            ))
            .expect("send");
        }
        app.apply_line("/rx ja1qso inbound visual check with a moderately long payload")
            .expect("receive");
        app.apply_line("/beacon monitoring 14.105 USB visual matrix")
            .expect("beacon");
        app.apply_line("/cq testing visual pane layout")
            .expect("cq");
        app.apply_line("/mail ja1qso Visual Subject | Visual mailbox body")
            .expect("mail");
        app.apply_line(&format!(
            "/file-offer ja1qso visual-layout-fixture-with-a-long-name.txt 42 {TEST_SHA256} visual note"
        ))
        .expect("file offer");
        app
    }

    fn setup_visual_app() -> ChatTuiApp {
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "ja1tst".to_owned(),
            backend: ChatTuiBackend::NativeLoopback,
            local_node: None,
            setup: Some(ChatTuiSetupConfig {
                backend: ChatTuiBackend::NativeLoopback,
                peer_call: None,
                mode: None,
                channel: ChannelConfig::default(),
            }),
        })
        .expect("app");
        app.apply_line("/station ve3tst").expect("station");
        app.apply_line("/backend native-local-node")
            .expect("backend");
        app.apply_line("/peer ja1qso").expect("peer");
        app.apply_line("/listen 127.0.0.1:0").expect("listen");
        app.apply_line("/audio-input USB Audio CODEC Extremely Long Input Device Name")
            .expect("audio input");
        app.apply_line("/audio-output USB Audio CODEC Extremely Long Output Device Name")
            .expect("audio output");
        app.apply_line("/radio-hamlib 127.0.0.1:4532")
            .expect("radio");
        app
    }

    #[test]
    fn parses_tui_commands() {
        assert_eq!(
            parse_tui_command("/connect ja1qso").unwrap(),
            TuiCommand::Connect("ja1qso".to_owned())
        );
        assert_eq!(
            parse_tui_command("hello peer").unwrap(),
            TuiCommand::Send("hello peer".to_owned())
        );
        assert_eq!(
            parse_tui_command("/rx ja1qso roger").unwrap(),
            TuiCommand::Receive {
                from: "ja1qso".to_owned(),
                text: "roger".to_owned(),
            }
        );
        assert_eq!(
            parse_tui_command("/beacon monitoring 14.105 USB").unwrap(),
            TuiCommand::Beacon("monitoring 14.105 USB".to_owned())
        );
        assert_eq!(
            parse_tui_command("/cq testing").unwrap(),
            TuiCommand::Cq("testing".to_owned())
        );
        assert_eq!(
            parse_tui_command("/mail ja1qso Subject | Body text").unwrap(),
            TuiCommand::Mail {
                to: "ja1qso".to_owned(),
                subject: "Subject".to_owned(),
                body: "Body text".to_owned(),
            }
        );
        assert_eq!(
            parse_tui_command("/mail-read #3").unwrap(),
            TuiCommand::MailRead { sequence: 3 }
        );
        assert_eq!(
            parse_tui_command("/mail-reply 3 Re: Subject | Reply body").unwrap(),
            TuiCommand::MailReply {
                sequence: 3,
                subject: "Re: Subject".to_owned(),
                body: "Reply body".to_owned(),
            }
        );
        assert_eq!(
            parse_tui_command(
                "/file-offer ja1qso sample.txt 42 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 note"
            )
            .unwrap(),
            TuiCommand::FileOffer {
                to: "ja1qso".to_owned(),
                filename: "sample.txt".to_owned(),
                byte_count: 42,
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_owned(),
                note: Some("note".to_owned()),
            }
        );
        assert_eq!(
            parse_tui_command("/file-inspect 4").unwrap(),
            TuiCommand::FileInspect { sequence: 4 }
        );
        assert_eq!(
            parse_tui_command("/file-accept 4 out/offers").unwrap(),
            TuiCommand::FileAccept {
                sequence: 4,
                out_dir: PathBuf::from("out/offers"),
            }
        );
        assert_eq!(
            parse_tui_command("/app-status").unwrap(),
            TuiCommand::AppStatus
        );
        assert_eq!(
            parse_tui_command("/station ja1tst").unwrap(),
            TuiCommand::SetupStation("ja1tst".to_owned())
        );
        assert_eq!(
            parse_tui_command("/backend native-wav-loopback").unwrap(),
            TuiCommand::SetupBackend(ChatTuiBackend::NativeWavLoopback)
        );
        assert_eq!(
            parse_tui_command("/peer ja1qso").unwrap(),
            TuiCommand::SetupPeer("ja1qso".to_owned())
        );
        assert_eq!(
            parse_tui_command("/listen").unwrap(),
            TuiCommand::SetupListen("127.0.0.1:0".to_owned())
        );
        assert_eq!(
            parse_tui_command("/connect-node 127.0.0.1:9000").unwrap(),
            TuiCommand::SetupConnectNode("127.0.0.1:9000".to_owned())
        );
        assert_eq!(
            parse_tui_command("/audio-input USB Audio CODEC").unwrap(),
            TuiCommand::SetupAudioInput("USB Audio CODEC".to_owned())
        );
        assert_eq!(
            parse_tui_command("/audio-output USB Audio CODEC").unwrap(),
            TuiCommand::SetupAudioOutput("USB Audio CODEC".to_owned())
        );
        assert_eq!(
            parse_tui_command("/audio-rate 48000").unwrap(),
            TuiCommand::SetupAudioRate(48_000)
        );
        assert_eq!(
            parse_tui_command("/audio-channels 1").unwrap(),
            TuiCommand::SetupAudioChannels(1)
        );
        assert_eq!(
            parse_tui_command("/radio-hamlib").unwrap(),
            TuiCommand::SetupRadioHamlib(DEFAULT_RIGCTLD_HOST.to_owned())
        );
        assert_eq!(
            parse_tui_command("/radio-off").unwrap(),
            TuiCommand::SetupRadioOff
        );
        assert_eq!(parse_tui_command("/start").unwrap(), TuiCommand::SetupStart);
        assert_eq!(
            parse_tui_command("/save-app out/app.json").unwrap(),
            TuiCommand::SaveApp(PathBuf::from("out/app.json"))
        );
        assert_eq!(
            parse_tui_command("/save-artifacts out/native.json").unwrap(),
            TuiCommand::SaveArtifacts(PathBuf::from("out/native.json"))
        );
        assert_eq!(
            parse_tui_command("/save-session out/session").unwrap(),
            TuiCommand::SaveSession(PathBuf::from("out/session"))
        );
        assert_eq!(
            parse_tui_command("/workspace weak-signal").unwrap(),
            TuiCommand::Workspace(WorkspaceId::WeakSignal)
        );
        assert!(parse_tui_command("/unknown").is_err());
    }

    #[test]
    fn tui_setup_starts_from_bare_config_and_applies_selection() {
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "JA1TST".to_owned(),
            backend: ChatTuiBackend::NativeLoopback,
            local_node: None,
            setup: Some(ChatTuiSetupConfig {
                backend: ChatTuiBackend::NativeLoopback,
                peer_call: None,
                mode: None,
                channel: ChannelConfig::default(),
            }),
        })
        .expect("app");

        assert!(app.setup.is_some());
        assert_eq!(app.focus, TuiPane::Setup);
        assert!(app.lines.iter().any(|line| line.contains("/station CALL")));
        app.handle_key(KeyCode::Down, KeyModifiers::empty())
            .expect("select backend");
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .expect("cycle backend");
        assert_eq!(
            app.setup.as_ref().map(|setup| setup.backend),
            Some(ChatTuiBackend::NativeWavLoopback)
        );
        app.apply_line("/station ja1tst").expect("station");
        app.apply_line("/backend fake").expect("backend");
        app.apply_line("/audio-input USB Audio CODEC")
            .expect("audio input");
        app.apply_line("/audio-output USB Audio CODEC")
            .expect("audio output");
        app.apply_line("/radio-hamlib 127.0.0.1:4532")
            .expect("radio hamlib");
        assert_eq!(
            app.setup
                .as_ref()
                .and_then(|setup| setup.hamlib_host.as_ref()),
            Some(&"127.0.0.1:4532".to_owned())
        );
        app.apply_line("/start").expect("start");

        assert!(app.setup.is_none());
        assert_eq!(app.backend_label, "fake");
        assert_eq!(app.focus, TuiPane::Composer);
        assert_eq!(app.runtime.audio_input.as_deref(), Some("USB Audio CODEC"));
        assert_eq!(app.runtime.audio_output.as_deref(), Some("USB Audio CODEC"));
        assert_eq!(app.runtime.radio_label(), "hamlib 127.0.0.1:4532");
        assert_eq!(app.transcript().station.call_sign, "JA1TST");
    }

    #[test]
    fn tui_draws_guided_setup_surface() {
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "JA1TST".to_owned(),
            backend: ChatTuiBackend::NativeLoopback,
            local_node: None,
            setup: Some(ChatTuiSetupConfig {
                backend: ChatTuiBackend::NativeLoopback,
                peer_call: None,
                mode: None,
                channel: ChannelConfig::default(),
            }),
        })
        .expect("app");

        let backend = ratatui::backend::TestBackend::new(120, 32);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, &app)).expect("draw");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("setup / radio"));
        assert!(rendered.contains("station JA1TST"));
        assert!(rendered.contains("backend native-loopback"));
        assert!(rendered.contains("safety DRY RUN"));
        assert!(rendered.contains("focus setup/radio"));

        app.apply_line("/station ve3tst").expect("station");
        assert!(app.status_text().starts_with("VE3TST |"));

        let backend = ratatui::backend::TestBackend::new(120, 32);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, &app)).expect("draw");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("VE3TST | backend"));
        assert!(rendered.contains("station VE3TST"));
    }

    #[test]
    fn fake_tui_session_updates_transcript_and_saves_log() {
        let dir = tempdir().expect("tempdir");
        let log_path = dir.path().join("chat.log");
        let app_path = dir.path().join("app-state.json");
        let receipt_dir = dir.path().join("receipts");
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "ja1tst".to_owned(),
            backend: ChatTuiBackend::Fake,
            local_node: None,
            setup: None,
        })
        .expect("app");

        app.apply_line("/connect ja1qso").expect("connect");
        app.apply_line("hello from tui").expect("send");
        app.apply_line("/rx ja1qso roger").expect("receive");
        app.apply_line("/beacon monitoring 14.105 USB")
            .expect("beacon");
        app.apply_line("/cq testing tui app model").expect("cq");
        app.apply_line("/mail ja1qso Subject | Body text")
            .expect("mail");
        app.apply_line(
            "/file-offer ja1qso sample.txt 42 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 note",
        )
        .expect("file offer");
        app.apply_line("/mail-read 3").expect("read mail");
        app.apply_line("/mail-reply 3 Re: Subject | Reply body")
            .expect("reply mail");
        app.apply_line("/file-inspect 4").expect("inspect file");
        app.apply_line(&format!("/file-accept 4 {}", receipt_dir.display()))
            .expect("accept file");
        app.apply_line("/app-status").expect("app status");
        app.apply_line(&format!("/save-log {}", log_path.display()))
            .expect("save log");
        app.apply_line(&format!("/save-app {}", app_path.display()))
            .expect("save app");

        let transcript = app.transcript();
        assert_eq!(transcript.station.call_sign, "JA1TST");
        assert_eq!(transcript.peer_call.as_deref(), Some("JA1QSO"));
        assert_eq!(transcript.messages.len(), 2);
        let log = fs::read_to_string(log_path).expect("read log");
        assert_eq!(log, "OUT JA1QSO hello from tui\nIN JA1QSO roger\n");
        let app_state = app.app_state();
        assert_eq!(app_state.beacons.len(), 1);
        assert_eq!(app_state.cq_calls.len(), 1);
        assert_eq!(app_state.mailbox[0].subject, "Subject");
        assert_eq!(app_state.mailbox[1].subject, "Re: Subject");
        assert_eq!(app_state.file_offers[0].filename, "sample.txt");
        assert!(receipt_dir.join("file-offer-004-sample.txt.json").exists());

        let saved_app: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(app_path).expect("read app state"))
                .expect("app json");
        assert_eq!(saved_app["kind"], "chat-app-state");
        assert_eq!(saved_app["beacons"].as_array().expect("beacons").len(), 1);
    }

    #[test]
    fn tui_keyboard_navigation_selects_mail_and_file_offers() {
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "ja1tst".to_owned(),
            backend: ChatTuiBackend::Fake,
            local_node: None,
            setup: None,
        })
        .expect("app");
        app.apply_line("/mail ja1qso First | First body")
            .expect("mail one");
        app.apply_line("/mail ja1qso Second | Second body")
            .expect("mail two");
        app.apply_line(
            "/file-offer ja1qso sample.txt 42 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 note",
        )
        .expect("file offer");

        app.focus = TuiPane::Mailbox;
        app.handle_key(KeyCode::Down, KeyModifiers::empty())
            .expect("down");
        assert_eq!(app.selected_mailbox_sequence(), Some(2));
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .expect("open mail");
        assert!(app.lines.iter().any(|line| line.contains("mail #2")));

        app.handle_key(KeyCode::Tab, KeyModifiers::empty())
            .expect("tab");
        assert_eq!(app.focus, TuiPane::FileOffers);
        assert_eq!(app.selected_file_offer_sequence(), Some(3));
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .expect("inspect file");
        assert!(app.lines.iter().any(|line| line.contains("file #3")));

        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty())
            .expect("slash");
        assert_eq!(app.focus, TuiPane::Composer);
        assert_eq!(app.composer_mode, ComposerMode::Command);
    }

    #[test]
    fn tui_draws_chat_app_surface() {
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "ja1tst".to_owned(),
            backend: ChatTuiBackend::Fake,
            local_node: None,
            setup: None,
        })
        .expect("app");
        app.apply_line("/connect ja1qso").expect("connect");
        app.apply_line("/beacon monitoring").expect("beacon");
        app.apply_line("/mail ja1qso Subject | Body").expect("mail");
        app.apply_line(
            "/file-offer ja1qso sample.txt 42 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 note",
        )
        .expect("file offer");

        let backend = ratatui::backend::TestBackend::new(120, 32);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, &app)).expect("draw");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("transcript"));
        assert!(rendered.contains("setup / radio"));
        assert!(rendered.contains("session ready"));
        assert!(rendered.contains("safety DRY RUN"));
        assert!(rendered.contains("beacon / cq monitor"));
        assert!(rendered.contains("mailbox"));
        assert!(rendered.contains("file offers"));
        assert!(rendered.contains("focus composer"));
        assert!(rendered.contains("input [chat]"));
    }

    #[test]
    fn tui_visual_layout_matrix_renders_core_surfaces() {
        let mut app = visual_app();
        let sizes = [
            (40, 12),
            (60, 16),
            (80, 24),
            (99, 24),
            (100, 24),
            (120, 32),
            (160, 48),
        ];
        let panes = [
            TuiPane::Setup,
            TuiPane::Transcript,
            TuiPane::Monitor,
            TuiPane::Mailbox,
            TuiPane::FileOffers,
            TuiPane::Composer,
        ];

        for pane in panes {
            app.focus = pane;
            for (width, height) in sizes {
                let lines = render_app_lines(&app, width, height);
                let rendered = lines.join("\n");
                assert_eq!(lines.len(), height as usize, "{width}x{height}");
                assert!(
                    lines.first().is_some_and(|line| line.contains("JA1TST")),
                    "missing station in status at {width}x{height} focus {}:\n{rendered}",
                    pane.label()
                );
                assert!(
                    rendered.contains("transcript"),
                    "missing transcript pane at {width}x{height} focus {}:\n{rendered}",
                    pane.label()
                );
                assert!(
                    rendered.contains("input [chat]"),
                    "missing input pane at {width}x{height} focus {}:\n{rendered}",
                    pane.label()
                );

                if width >= 100 && height >= 24 {
                    for label in [
                        "setup / radio",
                        "beacon / cq monitor",
                        "mailbox",
                        "file offers",
                    ] {
                        assert!(
                            rendered.contains(label),
                            "missing {label} at {width}x{height} focus {}:\n{rendered}",
                            pane.label()
                        );
                    }
                    assert!(
                        rendered.contains(&format!("focus {}", pane.label())),
                        "status did not track focus {} at {width}x{height}:\n{rendered}",
                        pane.label()
                    );
                }
            }
        }
    }

    #[test]
    fn tui_visual_setup_commands_update_pending_status_and_rows() {
        let mut app = setup_visual_app();

        for (width, height) in [(80, 24), (120, 32), (160, 48)] {
            app.setup_selected = 0;
            let rendered = render_app_text(&app, width, height);
            let first_row = rendered.lines().next().unwrap_or_default();
            assert!(
                first_row.contains("VE3TST"),
                "status did not show pending setup station at {width}x{height}:\n{rendered}"
            );
            assert!(
                first_row.contains("setup native-local-node"),
                "status did not show pending setup backend at {width}x{height}:\n{rendered}"
            );
            for label in [
                "station VE3TST",
                "backend local-node",
                "peer JA1QSO",
                "node listen 127.0.0.1:0",
            ] {
                assert!(
                    rendered.contains(label),
                    "missing setup row {label} at {width}x{height}:\n{rendered}"
                );
            }

            app.setup_selected = 6;
            let rendered = render_app_text(&app, width, height);
            assert!(
                rendered.contains("radio rig 127.0.0.1:4532"),
                "selected radio row was not visible at {width}x{height}:\n{rendered}"
            );

            app.setup_selected = 7;
            let rendered = render_app_text(&app, width, height);
            assert!(
                rendered.contains("start | safety DRY RUN"),
                "selected start row was not visible at {width}x{height}:\n{rendered}"
            );
        }
    }

    #[test]
    fn tui_visual_tiny_terminals_do_not_panic() {
        let mut app = visual_app();
        app.input = "x".repeat(70_000);

        for width in [16, 20, 30, 40] {
            for height in [4, 5, 6, 7, 8, 10, 12] {
                let lines = render_app_lines(&app, width, height);
                assert_eq!(lines.len(), height as usize, "{width}x{height}");
                assert!(
                    lines.iter().any(|line| !line.trim().is_empty()),
                    "blank render at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn tui_visual_keyboard_and_workspace_state_are_visible() {
        let mut app = visual_app();
        app.apply_line("/workspace weak-signal").expect("workspace");
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty())
            .expect("slash");
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
            .expect("h");

        let rendered = render_app_text(&app, 120, 32);
        assert!(rendered.contains("workspace weak-signal"));
        assert!(rendered.contains("input [command]"));
        assert!(rendered.contains("/h"));

        app.handle_key(KeyCode::Esc, KeyModifiers::empty())
            .expect("escape");
        let rendered = render_app_text(&app, 120, 32);
        assert!(rendered.contains("focus composer"));
        assert!(rendered.contains("input [chat]"));
    }

    #[test]
    fn native_loopback_tui_roundtrips_packets_and_saves_artifacts() {
        let dir = tempdir().expect("tempdir");
        let artifacts_path = dir.path().join("native-loopback.json");
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "ja1tst".to_owned(),
            backend: ChatTuiBackend::NativeLoopback,
            local_node: None,
            setup: None,
        })
        .expect("app");

        app.apply_line("/connect ja1qso").expect("connect");
        app.apply_line("hello native").expect("send");
        app.apply_line(&format!("/save-artifacts {}", artifacts_path.display()))
            .expect("save artifacts");

        let transcript = app.transcript();
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0].text, "hello native");
        assert_eq!(transcript.messages[1].text, "ack: hello native");

        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(artifacts_path).expect("read artifacts"))
                .expect("json");
        assert_eq!(report["backend"], "native-loopback");
        assert_eq!(report["packet_count"], 2);
        assert_eq!(report["packets"][0]["decode"]["ok"], true);
        assert_eq!(report["packets"][1]["decode"]["ok"], true);
        assert_eq!(
            report["packets"][0]["payload_text"],
            "MSG JA1TST hello native"
        );
        assert_eq!(
            report["packets"][1]["payload_text"],
            "MSG JA1QSO ack: hello native"
        );
    }

    #[test]
    fn native_wav_loopback_tui_writes_session_with_packet_wavs() {
        let dir = tempdir().expect("tempdir");
        let session_dir = dir.path().join("session");
        let mut app = ChatTuiApp::new(ChatTuiConfig {
            station_call: "ja1tst".to_owned(),
            backend: ChatTuiBackend::NativeWavLoopback,
            local_node: None,
            setup: None,
        })
        .expect("app");

        app.apply_line("/connect ja1qso").expect("connect");
        app.apply_line("hello wav").expect("send");
        app.apply_line(&format!("/save-session {}", session_dir.display()))
            .expect("save session");

        assert!(session_dir.join("transcript.json").exists());
        assert!(session_dir.join("events.jsonl").exists());
        assert!(session_dir.join("app-state.json").exists());
        assert!(session_dir.join("chat.log").exists());
        assert!(session_dir.join("artifacts.json").exists());
        assert!(session_dir.join("session.json").exists());
        let outbound_wav = session_dir
            .join("packets")
            .join("packet-001-outbound-JA1QSO.wav");
        let inbound_wav = session_dir
            .join("packets")
            .join("packet-002-inbound-JA1QSO.wav");
        assert!(outbound_wav.exists());
        assert!(inbound_wav.exists());

        let session: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(session_dir.join("session.json")).unwrap())
                .expect("session json");
        assert_eq!(session["backend"], "native-wav-loopback");
        assert_eq!(session["workspace"], "chat");
        assert_eq!(
            session["events"].as_str(),
            session_dir.join("events.jsonl").to_str()
        );
        assert_eq!(
            session["app_state"].as_str(),
            session_dir.join("app-state.json").to_str()
        );
        assert_eq!(session["artifacts"]["packet_count"], 2);
        assert_eq!(session["artifacts"]["packets"][0]["decode"]["ok"], true);
        assert_eq!(
            session["artifacts"]["packets"][0]["wav_path"].as_str(),
            outbound_wav.to_str()
        );
        assert_eq!(
            session["artifacts"]["packets"][1]["wav_path"].as_str(),
            inbound_wav.to_str()
        );
    }

    #[test]
    fn native_local_node_tui_exchanges_app_features() {
        let dir = tempdir().expect("tempdir");
        let ready_file = dir.path().join("listener.ready");
        let listener_ready_file = ready_file.clone();
        let listener_handle = thread::spawn(move || {
            ChatTuiApp::new(ChatTuiConfig {
                station_call: "ja1qso".to_owned(),
                backend: ChatTuiBackend::NativeLocalNode,
                local_node: Some(ChatTuiLocalNodeConfig {
                    peer_call: "ja1tst".to_owned(),
                    mode: LocalNodeMode::Listen {
                        bind: "127.0.0.1:0".to_owned(),
                        ready_file: Some(listener_ready_file),
                    },
                    channel: ChannelConfig::default(),
                }),
                setup: None,
            })
            .expect("listener app")
        });
        let address = wait_ready_file(&ready_file);
        let mut connector = ChatTuiApp::new(ChatTuiConfig {
            station_call: "ja1tst".to_owned(),
            backend: ChatTuiBackend::NativeLocalNode,
            local_node: Some(ChatTuiLocalNodeConfig {
                peer_call: "ja1qso".to_owned(),
                mode: LocalNodeMode::Connect { host: address },
                channel: ChannelConfig::default(),
            }),
            setup: None,
        })
        .expect("connector app");
        let mut listener = listener_handle.join().expect("listener thread");

        connector
            .apply_line("/beacon monitoring 14.105 USB")
            .expect("send beacon");
        wait_for_app_state(&mut listener, |state| state.beacons.len() == 1);
        assert_eq!(listener.app_state().beacons[0].from, "JA1TST");

        listener
            .apply_line("/cq replying over app packets")
            .expect("send cq");
        wait_for_app_state(&mut connector, |state| state.cq_calls.len() == 1);
        assert_eq!(connector.app_state().cq_calls[0].from, "JA1QSO");

        connector
            .apply_line("/mail ja1qso Test subject | Synthetic mailbox body")
            .expect("send mail");
        wait_for_app_state(&mut listener, |state| state.mailbox.len() == 1);
        assert_eq!(listener.app_state().mailbox[0].subject, "Test subject");

        listener
            .apply_line(
                "/file-offer ja1tst sample.txt 42 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 metadata only",
            )
            .expect("send file offer");
        wait_for_app_state(&mut connector, |state| state.file_offers.len() == 1);
        assert_eq!(connector.app_state().file_offers[0].from, "JA1QSO");
    }

    fn wait_ready_file(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(value) = fs::read_to_string(path) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_owned();
                }
            }
            assert!(
                Instant::now() <= deadline,
                "timed out waiting for ready file"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_app_state(
        app: &mut ChatTuiApp,
        predicate: impl Fn(&ChatAppState) -> bool,
    ) -> ChatAppState {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.poll_backend();
            let state = app.app_state();
            if predicate(&state) {
                return state;
            }
            assert!(
                Instant::now() <= deadline,
                "timed out waiting for app state"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

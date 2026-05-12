use crate::app_protocol::{
    AppDeliveryState, AppPacketKind, AppProtocolPacket, AppProtocolState,
    DEFAULT_FILE_CHUNK_DATA_BYTES, MAX_APP_PACKET_BYTES, ReassembledFile, decode_app_packet,
    encode_app_packet, reassemble_file_chunks,
};
use anyhow::{Context, Result, bail};
use chattybara_chat::{
    ChatAppEvent, ChatAppModel, ChatAppState, ChatBackend, ChatEvent, ChatMessage, ChatTranscript,
    FakeBackend, MessageDirection,
};
use orca_audio::AudioBuffer;
use orca_dsp::{ChannelConfig, simulate_channel_samples};
use orca_frames::{
    PacketCodecConfig, PacketDecodeReport, PacketEncodeReport, decode_packet_samples,
    encode_packet_payload,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

const AUDIO_FRAME_MAGIC: &[u8; 4] = b"CTBF";
const AUDIO_FRAME_VERSION: u8 = 1;
const MAX_TRANSPORT_PAYLOAD_BYTES: usize = MAX_APP_PACKET_BYTES;

#[derive(Debug, Clone)]
pub struct LocalPeerScriptConfig {
    pub station_a: String,
    pub station_b: String,
    pub out_dir: PathBuf,
    pub overwrite: bool,
    pub channel: ChannelConfig,
}

#[derive(Debug, Clone)]
pub struct LocalNodeScriptConfig {
    pub station: String,
    pub peer: String,
    pub out_dir: PathBuf,
    pub overwrite: bool,
    pub mode: LocalNodeMode,
    pub channel: ChannelConfig,
}

#[derive(Debug, Clone)]
pub struct LocalLiveNodeConfig {
    pub station: String,
    pub peer: String,
    pub mode: LocalNodeMode,
    pub channel: ChannelConfig,
}

#[derive(Debug, Clone)]
pub enum LocalNodeMode {
    Listen {
        bind: String,
        ready_file: Option<PathBuf>,
    },
    Connect {
        host: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalPeerScriptReport {
    pub kind: &'static str,
    pub backend: &'static str,
    pub ok: bool,
    pub channel: ChannelConfig,
    pub station_a: ChatTranscript,
    pub station_a_app: ChatAppState,
    pub station_b: ChatTranscript,
    pub station_b_app: ChatAppState,
    pub commands: Vec<LocalPeerCommandReport>,
    pub packets: Vec<LocalPeerPacketReport>,
    pub received_files: Vec<ReceivedFileReport>,
    pub paths: LocalPeerOutputPaths,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalNodeScriptReport {
    pub kind: &'static str,
    pub backend: &'static str,
    pub ok: bool,
    pub channel: ChannelConfig,
    pub station: ChatTranscript,
    pub app_state: ChatAppState,
    pub peer_call: String,
    pub commands: Vec<LocalNodeCommandReport>,
    pub packets: Vec<LocalNodePacketReport>,
    pub received_files: Vec<ReceivedFileReport>,
    pub paths: LocalNodeOutputPaths,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalPeerOutputPaths {
    pub out_dir: PathBuf,
    pub station_a_transcript: PathBuf,
    pub station_a_app: PathBuf,
    pub station_a_log: PathBuf,
    pub station_b_transcript: PathBuf,
    pub station_b_app: PathBuf,
    pub station_b_log: PathBuf,
    pub artifacts: PathBuf,
    pub session: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalNodeOutputPaths {
    pub out_dir: PathBuf,
    pub transcript: PathBuf,
    pub app_state: PathBuf,
    pub log: PathBuf,
    pub artifacts: PathBuf,
    pub session: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalPeerCommandReport {
    pub line_number: usize,
    pub line: String,
    pub side: Option<LocalPeerSide>,
    pub action: Option<String>,
    pub ok: bool,
    pub events: Vec<ChatEvent>,
    pub app_events: Vec<ChatAppEvent>,
    pub packet_sequence: Option<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalNodeCommandReport {
    pub line_number: usize,
    pub line: String,
    pub action: Option<String>,
    pub ok: bool,
    pub events: Vec<ChatEvent>,
    pub app_events: Vec<ChatAppEvent>,
    pub packet_sequence: Option<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalPeerPacketReport {
    pub sequence: usize,
    pub from: String,
    pub to: String,
    pub transport: &'static str,
    pub payload_text: String,
    pub sample_rate: u32,
    pub sample_count: usize,
    pub wav_path: Option<PathBuf>,
    pub encode: PacketEncodeReport,
    pub decode: PacketDecodeReport,
    #[serde(skip_serializing)]
    samples: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalNodePacketReport {
    pub sequence: usize,
    pub direction: MessageDirection,
    pub from: String,
    pub to: String,
    pub transport: &'static str,
    pub payload_text: String,
    pub sample_rate: u32,
    pub sample_count: usize,
    pub wav_path: Option<PathBuf>,
    pub encode: Option<PacketEncodeReport>,
    pub decode: PacketDecodeReport,
    #[serde(skip_serializing)]
    samples: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReceivedFileReport {
    pub station: String,
    pub from: String,
    pub to: String,
    pub filename: String,
    pub byte_count: u64,
    pub sha256: String,
    pub path: PathBuf,
    pub packet_count: usize,
}

pub(crate) struct LocalLiveNode {
    call_sign: String,
    peer_call: String,
    chat: FakeBackend,
    protocol: AppProtocolState,
    stream: TcpStream,
    config: PacketCodecConfig,
    channel: ChannelConfig,
    packets: Vec<LocalNodePacketReport>,
    inbound: Receiver<LocalLiveInbound>,
    reader_closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalLiveEvent {
    Chat(ChatEvent),
    App(LocalLiveAppEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalLiveAppEvent {
    Beacon {
        from: String,
        to: String,
        text: String,
    },
    Cq {
        from: String,
        to: String,
        text: String,
    },
    Mail {
        from: String,
        to: String,
        subject: String,
        body: String,
    },
    FileOffer {
        from: String,
        to: String,
        filename: String,
        byte_count: u64,
        sha256: String,
        note: Option<String>,
    },
}

struct LocalLiveInbound {
    frame: AudioFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalPeerSide {
    A,
    B,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalPeerCommand {
    side: LocalPeerSide,
    action: LocalPeerAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalPeerAction {
    Connect,
    Send(String),
    Beacon(String),
    Cq(String),
    Mail {
        subject: String,
        body: String,
    },
    FileOffer {
        filename: String,
        byte_count: u64,
        sha256: String,
        note: Option<String>,
    },
    FileSend {
        path: PathBuf,
        note: Option<String>,
    },
    Disconnect,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalNodeCommand {
    action: LocalNodeAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalNodeAction {
    Connect,
    ExpectConnect,
    Send(String),
    ExpectMessage(String),
    Beacon(String),
    ExpectBeacon(String),
    Cq(String),
    ExpectCq(String),
    Mail {
        subject: String,
        body: String,
    },
    ExpectMail {
        subject: String,
        body: String,
    },
    FileOffer {
        filename: String,
        byte_count: u64,
        sha256: String,
        note: Option<String>,
    },
    FileSend {
        path: PathBuf,
        note: Option<String>,
    },
    ExpectFileOffer {
        filename: String,
        byte_count: u64,
        sha256: String,
        note: Option<String>,
    },
    ExpectFileTransfer {
        filename: String,
        byte_count: u64,
        sha256: String,
        note: Option<String>,
    },
    Disconnect,
    ExpectDisconnect,
    Status,
}

struct LocalPeerSession {
    station_a: StationRuntime,
    station_b: StationRuntime,
    config: PacketCodecConfig,
    channel: ChannelConfig,
    packets: Vec<LocalPeerPacketReport>,
    received_dir: PathBuf,
    received_files: Vec<ReceivedFileReport>,
}

struct LocalNodeSession {
    call_sign: String,
    peer_call: String,
    chat: FakeBackend,
    app: ChatAppModel,
    protocol: AppProtocolState,
    stream: TcpStream,
    config: PacketCodecConfig,
    channel: ChannelConfig,
    packets: Vec<LocalNodePacketReport>,
    received_dir: PathBuf,
    received_files: Vec<ReceivedFileReport>,
}

struct StationRuntime {
    call_sign: String,
    chat: FakeBackend,
    app: ChatAppModel,
    protocol: AppProtocolState,
    stream: TcpStream,
}

struct AudioFrame {
    sample_rate: u32,
    samples: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransportPacket {
    Connect {
        from: String,
        to: String,
    },
    Message {
        from: String,
        to: String,
        text: String,
    },
    AppBeacon {
        from: String,
        to: String,
        text: String,
    },
    AppCq {
        from: String,
        to: String,
        text: String,
    },
    AppMail {
        from: String,
        to: String,
        subject: String,
        body: String,
    },
    AppFileOffer {
        from: String,
        to: String,
        filename: String,
        byte_count: u64,
        sha256: String,
        note: Option<String>,
    },
    AppFileChunk {
        from: String,
        to: String,
        file_id: String,
        filename: String,
        fragment_index: u32,
        fragment_total: u32,
    },
    AppFragment {
        from: String,
        to: String,
        message_id: String,
        fragment_index: u32,
        fragment_total: u32,
    },
    AppAck {
        from: String,
        to: String,
        receipt_for: String,
        delivery: AppDeliveryState,
    },
    Disconnect {
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum AppTransportEnvelope {
    Beacon {
        from: String,
        to: String,
        text: String,
    },
    Cq {
        from: String,
        to: String,
        text: String,
    },
    Mail {
        from: String,
        to: String,
        subject: String,
        body: String,
    },
    FileOffer {
        from: String,
        to: String,
        filename: String,
        byte_count: u64,
        sha256: String,
        note: Option<String>,
    },
}

impl From<AppTransportEnvelope> for TransportPacket {
    fn from(envelope: AppTransportEnvelope) -> Self {
        match envelope {
            AppTransportEnvelope::Beacon { from, to, text } => Self::AppBeacon { from, to, text },
            AppTransportEnvelope::Cq { from, to, text } => Self::AppCq { from, to, text },
            AppTransportEnvelope::Mail {
                from,
                to,
                subject,
                body,
            } => Self::AppMail {
                from,
                to,
                subject,
                body,
            },
            AppTransportEnvelope::FileOffer {
                from,
                to,
                filename,
                byte_count,
                sha256,
                note,
            } => Self::AppFileOffer {
                from,
                to,
                filename,
                byte_count,
                sha256,
                note,
            },
        }
    }
}

impl AppTransportEnvelope {
    #[cfg(test)]
    fn source_call(&self) -> &str {
        match self {
            Self::Beacon { from, .. }
            | Self::Cq { from, .. }
            | Self::Mail { from, .. }
            | Self::FileOffer { from, .. } => from,
        }
    }

    fn into_protocol_packet(self, protocol: &mut AppProtocolState) -> AppProtocolPacket {
        match self {
            Self::Beacon { to, text, .. } => protocol.beacon(&to, &text),
            Self::Cq { to, text, .. } => protocol.cq(&to, &text),
            Self::Mail {
                to, subject, body, ..
            } => protocol.mail(&to, &subject, &body),
            Self::FileOffer {
                to,
                filename,
                byte_count,
                sha256,
                note,
                ..
            } => protocol.file_offer(&to, &filename, byte_count, &sha256, note),
        }
    }
}

pub fn run_local_peer_script(
    config: LocalPeerScriptConfig,
    script: &str,
) -> Result<LocalPeerScriptReport> {
    prepare_output_dir(&config.out_dir, config.overwrite)?;
    let mut session = LocalPeerSession::new(
        &config.station_a,
        &config.station_b,
        config.channel,
        &config.out_dir,
    )?;
    let mut commands = Vec::new();
    let mut ok = true;

    for (index, line) in script.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let parsed = parse_local_peer_command(trimmed);
        let report = match parsed {
            Ok(command) => {
                let side = Some(command.side);
                let action = Some(command.action.label().to_owned());
                match session.apply(command) {
                    Ok(outcome) => LocalPeerCommandReport {
                        line_number: index + 1,
                        line: trimmed.to_owned(),
                        side,
                        action,
                        ok: true,
                        events: outcome.events,
                        app_events: outcome.app_events,
                        packet_sequence: outcome.packet_sequence,
                        error: None,
                    },
                    Err(error) => {
                        ok = false;
                        LocalPeerCommandReport {
                            line_number: index + 1,
                            line: trimmed.to_owned(),
                            side,
                            action,
                            ok: false,
                            events: Vec::new(),
                            app_events: Vec::new(),
                            packet_sequence: None,
                            error: Some(error.to_string()),
                        }
                    }
                }
            }
            Err(error) => {
                ok = false;
                LocalPeerCommandReport {
                    line_number: index + 1,
                    line: trimmed.to_owned(),
                    side: None,
                    action: None,
                    ok: false,
                    events: Vec::new(),
                    app_events: Vec::new(),
                    packet_sequence: None,
                    error: Some(error.to_string()),
                }
            }
        };
        commands.push(report);
    }

    session.write_outputs(&config.out_dir, commands, ok)
}

pub fn run_local_node_script(
    config: LocalNodeScriptConfig,
    script: &str,
) -> Result<LocalNodeScriptReport> {
    prepare_output_dir(&config.out_dir, config.overwrite)?;
    let mut session = LocalNodeSession::new(&config)?;
    let mut commands = Vec::new();
    let mut ok = true;

    for (index, line) in script.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let parsed = parse_local_node_command(trimmed);
        let report = match parsed {
            Ok(command) => {
                let action = Some(command.action.label().to_owned());
                match session.apply(command) {
                    Ok(outcome) => LocalNodeCommandReport {
                        line_number: index + 1,
                        line: trimmed.to_owned(),
                        action,
                        ok: true,
                        events: outcome.events,
                        app_events: outcome.app_events,
                        packet_sequence: outcome.packet_sequence,
                        error: None,
                    },
                    Err(error) => {
                        ok = false;
                        LocalNodeCommandReport {
                            line_number: index + 1,
                            line: trimmed.to_owned(),
                            action,
                            ok: false,
                            events: Vec::new(),
                            app_events: Vec::new(),
                            packet_sequence: None,
                            error: Some(error.to_string()),
                        }
                    }
                }
            }
            Err(error) => {
                ok = false;
                LocalNodeCommandReport {
                    line_number: index + 1,
                    line: trimmed.to_owned(),
                    action: None,
                    ok: false,
                    events: Vec::new(),
                    app_events: Vec::new(),
                    packet_sequence: None,
                    error: Some(error.to_string()),
                }
            }
        };
        commands.push(report);
    }

    session.write_outputs(&config.out_dir, commands, ok)
}

impl LocalLiveNode {
    pub(crate) fn new(config: LocalLiveNodeConfig) -> Result<Self> {
        let chat = FakeBackend::new(&config.station)?;
        let call_sign = chat.transcript().station.call_sign;
        let peer_call = FakeBackend::new(&config.peer)?
            .transcript()
            .station
            .call_sign;
        if call_sign == peer_call {
            bail!("station and peer must be different callsigns");
        }

        let stream = open_local_node_stream(&config.mode)?;
        stream
            .set_nodelay(true)
            .context("configuring local live node TCP stream")?;
        let read_stream = stream
            .try_clone()
            .context("cloning local live node TCP stream")?;
        let inbound = spawn_live_reader(read_stream);

        Ok(Self {
            protocol: AppProtocolState::new(&call_sign),
            call_sign,
            peer_call,
            chat,
            stream,
            config: PacketCodecConfig::default(),
            channel: config.channel,
            packets: Vec::new(),
            inbound,
            reader_closed: false,
        })
    }

    pub(crate) fn connect(&mut self, remote_call: &str) -> Result<Vec<ChatEvent>> {
        if normalize_live_call(remote_call)? != self.peer_call {
            bail!(
                "native local node peer is {}; cannot connect to {remote_call}",
                self.peer_call
            );
        }
        let event = self.chat.connect(&self.peer_call)?;
        let payload = format!("CONNECT {} {}", self.call_sign, self.peer_call);
        self.send_payload(&payload)?;
        Ok(vec![event])
    }

    pub(crate) fn send_text(&mut self, text: &str) -> Result<Vec<ChatEvent>> {
        let event = self.chat.send_text(text)?;
        let ChatEvent::Message {
            from,
            to,
            text: event_text,
            ..
        } = &event
        else {
            bail!("send did not produce a message event");
        };
        let payload = format!("MSG {from} {to} {event_text}");
        self.send_payload(&payload)?;
        Ok(vec![event])
    }

    pub(crate) fn receive_text(&mut self, from: &str, text: &str) -> Result<Vec<ChatEvent>> {
        Ok(vec![self.chat.receive_text(from, text)?])
    }

    pub(crate) fn send_beacon(&mut self, text: &str) -> Result<()> {
        let payload = app_protocol_payload(
            &mut self.protocol,
            AppTransportEnvelope::Beacon {
                from: self.call_sign.clone(),
                to: self.peer_call.clone(),
                text: text.to_owned(),
            },
        )?;
        self.send_payload(&payload)?;
        Ok(())
    }

    pub(crate) fn send_cq(&mut self, text: &str) -> Result<()> {
        let payload = app_protocol_payload(
            &mut self.protocol,
            AppTransportEnvelope::Cq {
                from: self.call_sign.clone(),
                to: self.peer_call.clone(),
                text: text.to_owned(),
            },
        )?;
        self.send_payload(&payload)?;
        Ok(())
    }

    pub(crate) fn send_mail(&mut self, subject: &str, body: &str) -> Result<()> {
        let payload = app_protocol_payload(
            &mut self.protocol,
            AppTransportEnvelope::Mail {
                from: self.call_sign.clone(),
                to: self.peer_call.clone(),
                subject: subject.to_owned(),
                body: body.to_owned(),
            },
        )?;
        self.send_payload(&payload)?;
        Ok(())
    }

    pub(crate) fn send_file_offer(
        &mut self,
        filename: &str,
        byte_count: u64,
        sha256: &str,
        note: Option<&str>,
    ) -> Result<()> {
        let payload = app_protocol_payload(
            &mut self.protocol,
            AppTransportEnvelope::FileOffer {
                from: self.call_sign.clone(),
                to: self.peer_call.clone(),
                filename: filename.to_owned(),
                byte_count,
                sha256: sha256.to_owned(),
                note: note
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
            },
        )?;
        self.send_payload(&payload)?;
        Ok(())
    }

    pub(crate) fn disconnect(&mut self) -> Result<Vec<ChatEvent>> {
        let payload = format!("DISCONNECT {} {}", self.call_sign, self.peer_call);
        self.send_payload(&payload)?;
        Ok(vec![self.chat.disconnect()?])
    }

    pub(crate) fn status(&self) -> ChatEvent {
        self.chat.status()
    }

    pub(crate) fn transcript(&self) -> ChatTranscript {
        self.chat.transcript()
    }

    pub(crate) fn poll_events(&mut self) -> Result<Vec<LocalLiveEvent>> {
        let mut events = Vec::new();
        loop {
            match self.inbound.try_recv() {
                Ok(inbound) => events.extend(self.receive_frame(inbound.frame)?),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.reader_closed {
                        self.reader_closed = true;
                    }
                    break;
                }
            }
        }
        Ok(events)
    }

    pub(crate) fn artifacts_report(
        &self,
        wav_paths: Option<&[Option<PathBuf>]>,
    ) -> serde_json::Value {
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
                    "from": packet.from,
                    "to": packet.to,
                    "transport": packet.transport,
                    "payload_text": packet.payload_text,
                    "sample_rate": packet.sample_rate,
                    "sample_count": packet.sample_count,
                    "wav_path": wav_path,
                    "encode": packet.encode,
                    "decode": packet.decode,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "kind": "tui-backend-artifacts",
            "backend": "native-local-node",
            "channel": self.channel,
            "packet_count": self.packets.len(),
            "transcript": self.chat.transcript(),
            "packets": packets,
        })
    }

    pub(crate) fn write_packet_wavs(&self, dir: &Path) -> Result<Vec<Option<PathBuf>>> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        self.packets
            .iter()
            .map(|packet| {
                let path = dir.join(node_packet_wav_filename(packet));
                AudioBuffer::new(packet.sample_rate, 1, packet.samples.clone())
                    .with_context(|| "building native local node packet WAV")?
                    .write_wav(&path)
                    .with_context(|| format!("writing {}", path.display()))?;
                Ok(Some(path))
            })
            .collect()
    }

    fn send_payload(&mut self, payload: &str) -> Result<LocalNodePacketReport> {
        ensure_transport_payload_size(payload)?;
        let signal = encode_packet_payload(payload.as_bytes(), self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        let samples = simulate_channel_samples(&signal.samples, self.channel);
        let decode = decode_packet_samples(&samples, self.config.sample_rate, self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !decode.ok || decode.payload_text.as_deref() != Some(payload) {
            bail!("native local node outbound packet failed self-decode");
        }
        write_audio_frame(&mut self.stream, self.config.sample_rate, &samples)?;
        let (from, to) = transport_route(payload)?;
        let packet = LocalNodePacketReport {
            sequence: self.packets.len() + 1,
            direction: MessageDirection::Outbound,
            from,
            to,
            transport: "localhost-tcp-audio-frame",
            payload_text: payload.to_owned(),
            sample_rate: self.config.sample_rate,
            sample_count: samples.len(),
            wav_path: None,
            encode: Some(signal.report),
            decode,
            samples,
        };
        self.packets.push(packet.clone());
        Ok(packet)
    }

    fn receive_frame(&mut self, frame: AudioFrame) -> Result<Vec<LocalLiveEvent>> {
        let decode = decode_packet_samples(&frame.samples, frame.sample_rate, self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !decode.ok {
            bail!("native local node inbound packet failed to decode");
        }
        let payload = decode
            .payload_text
            .clone()
            .context("native local node inbound packet did not decode as UTF-8 text")?;
        let transport = parse_transport_packet(&payload)?;
        let (from, to) = transport_route(&payload)?;
        let packet = LocalNodePacketReport {
            sequence: self.packets.len() + 1,
            direction: MessageDirection::Inbound,
            from: from.clone(),
            to: to.clone(),
            transport: "localhost-tcp-audio-frame",
            payload_text: payload,
            sample_rate: frame.sample_rate,
            sample_count: frame.samples.len(),
            wav_path: None,
            encode: None,
            decode,
            samples: frame.samples,
        };
        self.packets.push(packet);

        match transport {
            TransportPacket::Connect { from, to } => {
                ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
                Ok(vec![LocalLiveEvent::Chat(self.chat.connect(&from)?)])
            }
            TransportPacket::Message { from, to, text } => {
                ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
                Ok(vec![LocalLiveEvent::Chat(
                    self.chat.receive_text(&from, &text)?,
                )])
            }
            TransportPacket::AppBeacon { from, to, text } => {
                ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
                Ok(vec![LocalLiveEvent::App(LocalLiveAppEvent::Beacon {
                    from,
                    to,
                    text,
                })])
            }
            TransportPacket::AppCq { from, to, text } => {
                ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
                Ok(vec![LocalLiveEvent::App(LocalLiveAppEvent::Cq {
                    from,
                    to,
                    text,
                })])
            }
            TransportPacket::AppMail {
                from,
                to,
                subject,
                body,
            } => {
                ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
                Ok(vec![LocalLiveEvent::App(LocalLiveAppEvent::Mail {
                    from,
                    to,
                    subject,
                    body,
                })])
            }
            TransportPacket::AppFileOffer {
                from,
                to,
                filename,
                byte_count,
                sha256,
                note,
            } => {
                ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
                Ok(vec![LocalLiveEvent::App(LocalLiveAppEvent::FileOffer {
                    from,
                    to,
                    filename,
                    byte_count,
                    sha256,
                    note,
                })])
            }
            TransportPacket::AppFileChunk { from, to, .. }
            | TransportPacket::AppFragment { from, to, .. }
            | TransportPacket::AppAck { from, to, .. } => {
                ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
                Ok(Vec::new())
            }
            TransportPacket::Disconnect { from, to } => {
                ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
                Ok(vec![LocalLiveEvent::Chat(self.chat.disconnect()?)])
            }
        }
    }
}

impl LocalPeerSession {
    fn new(
        station_a: &str,
        station_b: &str,
        channel: ChannelConfig,
        out_dir: &Path,
    ) -> Result<Self> {
        let (stream_a, stream_b) = tcp_pair().context("creating local peer TCP pair")?;
        let station_a = StationRuntime::new(station_a, stream_a)?;
        let station_b = StationRuntime::new(station_b, stream_b)?;
        if station_a.call_sign == station_b.call_sign {
            bail!("station-a and station-b must be different callsigns");
        }
        Ok(Self {
            station_a,
            station_b,
            config: PacketCodecConfig::default(),
            channel,
            packets: Vec::new(),
            received_dir: out_dir.join("received"),
            received_files: Vec::new(),
        })
    }

    fn apply(&mut self, command: LocalPeerCommand) -> Result<LocalPeerCommandOutcome> {
        match command.action {
            LocalPeerAction::Connect => self.connect(command.side),
            LocalPeerAction::Send(text) => self.send_text(command.side, &text),
            LocalPeerAction::Beacon(text) => self.beacon(command.side, &text),
            LocalPeerAction::Cq(text) => self.cq(command.side, &text),
            LocalPeerAction::Mail { subject, body } => self.mail(command.side, &subject, &body),
            LocalPeerAction::FileOffer {
                filename,
                byte_count,
                sha256,
                note,
            } => self.file_offer(command.side, &filename, byte_count, &sha256, note),
            LocalPeerAction::FileSend { path, note } => self.file_send(command.side, &path, note),
            LocalPeerAction::Disconnect => self.disconnect(command.side),
            LocalPeerAction::Status => Ok(LocalPeerCommandOutcome {
                events: vec![self.station(command.side).chat.status()],
                app_events: vec![self.station(command.side).app.status()],
                packet_sequence: None,
            }),
        }
    }

    fn connect(&mut self, side: LocalPeerSide) -> Result<LocalPeerCommandOutcome> {
        let from = self.station(side).call_sign.clone();
        let to = self.station(side.other()).call_sign.clone();
        let local_event = self.station_mut(side).chat.connect(&to)?;
        let payload = format!("CONNECT {from} {to}");
        let packet = self.transmit(side, &payload)?;
        let transport =
            parse_transport_packet(packet.decode.payload_text.as_deref().unwrap_or_default())?;
        let TransportPacket::Connect {
            from: decoded_from,
            to: decoded_to,
        } = transport
        else {
            bail!("decoded packet was not a CONNECT frame");
        };
        ensure_route(&decoded_from, &decoded_to, &from, &to)?;
        let remote_event = self.station_mut(side.other()).chat.connect(&decoded_from)?;
        Ok(LocalPeerCommandOutcome {
            events: vec![local_event, remote_event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn send_text(&mut self, side: LocalPeerSide, text: &str) -> Result<LocalPeerCommandOutcome> {
        let local_event = self.station_mut(side).chat.send_text(text)?;
        let ChatEvent::Message {
            from,
            to,
            text: event_text,
            ..
        } = &local_event
        else {
            bail!("send did not produce a message event");
        };
        let payload = format!("MSG {from} {to} {event_text}");
        let packet = self.transmit(side, &payload)?;
        let transport =
            parse_transport_packet(packet.decode.payload_text.as_deref().unwrap_or_default())?;
        let TransportPacket::Message {
            from: decoded_from,
            to: decoded_to,
            text: decoded_text,
        } = transport
        else {
            bail!("decoded packet was not a MSG frame");
        };
        ensure_route(&decoded_from, &decoded_to, from, to)?;
        let remote_event = self
            .station_mut(side.other())
            .chat
            .receive_text(&decoded_from, &decoded_text)?;
        Ok(LocalPeerCommandOutcome {
            events: vec![local_event, remote_event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn beacon(&mut self, side: LocalPeerSide, text: &str) -> Result<LocalPeerCommandOutcome> {
        let from = self.station(side).call_sign.clone();
        let to = self.station(side.other()).call_sign.clone();
        let payload = app_protocol_payload(
            &mut self.station_mut(side).protocol,
            AppTransportEnvelope::Beacon {
                from: from.clone(),
                to: to.clone(),
                text: text.to_owned(),
            },
        )?;
        let local_event = self.station_mut(side).app.beacon(text)?;
        let packet = self.transmit(side, &payload)?;
        let transport =
            parse_transport_packet(packet.decode.payload_text.as_deref().unwrap_or_default())?;
        let TransportPacket::AppBeacon {
            from: decoded_from,
            to: decoded_to,
            text: decoded_text,
        } = transport
        else {
            bail!("decoded packet was not an APP-BEACON frame");
        };
        ensure_route(&decoded_from, &decoded_to, &from, &to)?;
        let remote_event = self
            .station_mut(side.other())
            .app
            .observe_beacon(&decoded_from, &decoded_text)?;
        Ok(LocalPeerCommandOutcome {
            events: Vec::new(),
            app_events: vec![local_event, remote_event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn cq(&mut self, side: LocalPeerSide, text: &str) -> Result<LocalPeerCommandOutcome> {
        let from = self.station(side).call_sign.clone();
        let to = self.station(side.other()).call_sign.clone();
        let payload = app_protocol_payload(
            &mut self.station_mut(side).protocol,
            AppTransportEnvelope::Cq {
                from: from.clone(),
                to: to.clone(),
                text: text.to_owned(),
            },
        )?;
        let local_event = self.station_mut(side).app.cq(text)?;
        let packet = self.transmit(side, &payload)?;
        let transport =
            parse_transport_packet(packet.decode.payload_text.as_deref().unwrap_or_default())?;
        let TransportPacket::AppCq {
            from: decoded_from,
            to: decoded_to,
            text: decoded_text,
        } = transport
        else {
            bail!("decoded packet was not an APP-CQ frame");
        };
        ensure_route(&decoded_from, &decoded_to, &from, &to)?;
        let remote_event = self
            .station_mut(side.other())
            .app
            .observe_cq(&decoded_from, &decoded_text)?;
        Ok(LocalPeerCommandOutcome {
            events: Vec::new(),
            app_events: vec![local_event, remote_event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn mail(
        &mut self,
        side: LocalPeerSide,
        subject: &str,
        body: &str,
    ) -> Result<LocalPeerCommandOutcome> {
        let from = self.station(side).call_sign.clone();
        let to = self.station(side.other()).call_sign.clone();
        let payload = app_protocol_payload(
            &mut self.station_mut(side).protocol,
            AppTransportEnvelope::Mail {
                from: from.clone(),
                to: to.clone(),
                subject: subject.to_owned(),
                body: body.to_owned(),
            },
        )?;
        let local_event = self
            .station_mut(side)
            .app
            .mailbox_message(&to, subject, body)?;
        let packet = self.transmit(side, &payload)?;
        let transport =
            parse_transport_packet(packet.decode.payload_text.as_deref().unwrap_or_default())?;
        let TransportPacket::AppMail {
            from: decoded_from,
            to: decoded_to,
            subject: decoded_subject,
            body: decoded_body,
        } = transport
        else {
            bail!("decoded packet was not an APP-MAIL frame");
        };
        ensure_route(&decoded_from, &decoded_to, &from, &to)?;
        let remote_event = self.station_mut(side.other()).app.receive_mailbox_message(
            &decoded_from,
            &decoded_to,
            &decoded_subject,
            &decoded_body,
        )?;
        Ok(LocalPeerCommandOutcome {
            events: Vec::new(),
            app_events: vec![local_event, remote_event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn file_offer(
        &mut self,
        side: LocalPeerSide,
        filename: &str,
        byte_count: u64,
        sha256: &str,
        note: Option<String>,
    ) -> Result<LocalPeerCommandOutcome> {
        let from = self.station(side).call_sign.clone();
        let to = self.station(side.other()).call_sign.clone();
        let payload = app_protocol_payload(
            &mut self.station_mut(side).protocol,
            AppTransportEnvelope::FileOffer {
                from: from.clone(),
                to: to.clone(),
                filename: filename.to_owned(),
                byte_count,
                sha256: sha256.to_owned(),
                note: note
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
            },
        )?;
        let local_event = self.station_mut(side).app.file_offer(
            &to,
            filename,
            byte_count,
            sha256,
            note.clone(),
        )?;
        let packet = self.transmit(side, &payload)?;
        let transport =
            parse_transport_packet(packet.decode.payload_text.as_deref().unwrap_or_default())?;
        let TransportPacket::AppFileOffer {
            from: decoded_from,
            to: decoded_to,
            filename: decoded_filename,
            byte_count: decoded_byte_count,
            sha256: decoded_sha256,
            note: decoded_note,
        } = transport
        else {
            bail!("decoded packet was not an APP-FILE frame");
        };
        ensure_route(&decoded_from, &decoded_to, &from, &to)?;
        let remote_event = self.station_mut(side.other()).app.receive_file_offer(
            &decoded_from,
            &decoded_to,
            &decoded_filename,
            decoded_byte_count,
            &decoded_sha256,
            decoded_note,
        )?;
        Ok(LocalPeerCommandOutcome {
            events: Vec::new(),
            app_events: vec![local_event, remote_event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn file_send(
        &mut self,
        side: LocalPeerSide,
        path: &Path,
        note: Option<String>,
    ) -> Result<LocalPeerCommandOutcome> {
        let from = self.station(side).call_sign.clone();
        let to = self.station(side.other()).call_sign.clone();
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if bytes.is_empty() {
            bail!("cannot send empty file {}", path.display());
        }
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .context("file-send path must have a UTF-8 filename")?
            .to_owned();
        let protocol_packets = self.station_mut(side).protocol.file_transfer_packets(
            &to,
            &filename,
            &bytes,
            note.clone(),
            DEFAULT_FILE_CHUNK_DATA_BYTES,
        )?;
        let offer = protocol_packets
            .first()
            .context("file transfer did not produce a file offer")?;
        let (offer_filename, offer_byte_count, offer_sha256, offer_note) =
            offer.require_file_offer()?;
        let local_event = self.station_mut(side).app.file_offer(
            &to,
            &offer_filename,
            offer_byte_count,
            &offer_sha256,
            offer_note.clone(),
        )?;

        let mut decoded_packets = Vec::with_capacity(protocol_packets.len());
        let mut first_packet_sequence = None;
        for protocol_packet in protocol_packets {
            let payload = encode_app_packet(&protocol_packet)?;
            let packet = self.transmit(side, &payload)?;
            first_packet_sequence.get_or_insert(packet.sequence);
            let decoded_payload = packet
                .decode
                .payload_text
                .as_deref()
                .context("file-send packet did not decode as text")?;
            let decoded_packet =
                decode_app_packet(decoded_payload)?.context("file-send packet was not CBAPP/1")?;
            decoded_packets.push(decoded_packet);
        }

        let decoded_offer = decoded_packets
            .iter()
            .find(|packet| packet.kind == AppPacketKind::FileOffer)
            .cloned()
            .context("received file transfer did not contain a file offer")?;
        let TransportPacket::AppFileOffer {
            from: decoded_from,
            to: decoded_to,
            filename: decoded_filename,
            byte_count: decoded_byte_count,
            sha256: decoded_sha256,
            note: decoded_note,
        } = app_protocol_packet_to_transport(decoded_offer)?
        else {
            bail!("decoded file transfer offer was not an APP-FILE frame");
        };
        ensure_route(&decoded_from, &decoded_to, &from, &to)?;
        let remote_event = self.station_mut(side.other()).app.receive_file_offer(
            &decoded_from,
            &decoded_to,
            &decoded_filename,
            decoded_byte_count,
            &decoded_sha256,
            decoded_note,
        )?;
        let reassembled = reassemble_file_chunks(&decoded_packets)?;
        self.save_received_file(
            side.other(),
            &decoded_from,
            &decoded_to,
            reassembled,
            decoded_packets.len(),
        )?;

        Ok(LocalPeerCommandOutcome {
            events: Vec::new(),
            app_events: vec![local_event, remote_event],
            packet_sequence: first_packet_sequence,
        })
    }

    fn disconnect(&mut self, side: LocalPeerSide) -> Result<LocalPeerCommandOutcome> {
        let from = self.station(side).call_sign.clone();
        let to = self.station(side.other()).call_sign.clone();
        let payload = format!("DISCONNECT {from} {to}");
        let packet = self.transmit(side, &payload)?;
        let transport =
            parse_transport_packet(packet.decode.payload_text.as_deref().unwrap_or_default())?;
        let TransportPacket::Disconnect {
            from: decoded_from,
            to: decoded_to,
        } = transport
        else {
            bail!("decoded packet was not a DISCONNECT frame");
        };
        ensure_route(&decoded_from, &decoded_to, &from, &to)?;
        let local_event = self.station_mut(side).chat.disconnect()?;
        let remote_event = self.station_mut(side.other()).chat.disconnect()?;
        Ok(LocalPeerCommandOutcome {
            events: vec![local_event, remote_event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn transmit(&mut self, side: LocalPeerSide, payload: &str) -> Result<LocalPeerPacketReport> {
        ensure_transport_payload_size(payload)?;
        let from = self.station(side).call_sign.clone();
        let to = self.station(side.other()).call_sign.clone();
        let signal = encode_packet_payload(payload.as_bytes(), self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        let samples = simulate_channel_samples(&signal.samples, self.channel);

        match side {
            LocalPeerSide::A => {
                let writer = spawn_audio_frame_writer(
                    self.station_a
                        .stream
                        .try_clone()
                        .context("cloning local peer station A writer")?,
                    self.config.sample_rate,
                    samples.clone(),
                );
                let frame = read_audio_frame(&mut self.station_b.stream);
                join_audio_frame_writer(writer)?;
                let frame = frame?;
                self.record_packet(from, to, payload, signal.report, frame)
            }
            LocalPeerSide::B => {
                let writer = spawn_audio_frame_writer(
                    self.station_b
                        .stream
                        .try_clone()
                        .context("cloning local peer station B writer")?,
                    self.config.sample_rate,
                    samples.clone(),
                );
                let frame = read_audio_frame(&mut self.station_a.stream);
                join_audio_frame_writer(writer)?;
                let frame = frame?;
                self.record_packet(from, to, payload, signal.report, frame)
            }
        }
    }

    fn record_packet(
        &mut self,
        from: String,
        to: String,
        payload: &str,
        encode: PacketEncodeReport,
        frame: AudioFrame,
    ) -> Result<LocalPeerPacketReport> {
        let decode = decode_packet_samples(&frame.samples, frame.sample_rate, self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !decode.ok {
            bail!("local peer packet failed to decode");
        }
        if decode.payload_text.as_deref() != Some(payload) {
            bail!("local peer packet payload mismatch after decode");
        }

        let packet = LocalPeerPacketReport {
            sequence: self.packets.len() + 1,
            from,
            to,
            transport: "localhost-tcp-audio-frame",
            payload_text: payload.to_owned(),
            sample_rate: frame.sample_rate,
            sample_count: frame.samples.len(),
            wav_path: None,
            encode,
            decode,
            samples: frame.samples,
        };
        self.packets.push(packet.clone());
        Ok(packet)
    }

    fn save_received_file(
        &mut self,
        receiver: LocalPeerSide,
        from: &str,
        to: &str,
        file: ReassembledFile,
        packet_count: usize,
    ) -> Result<ReceivedFileReport> {
        let station = self.station(receiver).call_sign.clone();
        let filename = safe_received_filename(&file.filename);
        let station_dir = self.received_dir.join(&station);
        fs::create_dir_all(&station_dir)
            .with_context(|| format!("creating {}", station_dir.display()))?;
        let path = station_dir.join(&filename);
        fs::write(&path, &file.bytes).with_context(|| format!("writing {}", path.display()))?;
        let report = ReceivedFileReport {
            station,
            from: from.to_owned(),
            to: to.to_owned(),
            filename,
            byte_count: file.bytes.len() as u64,
            sha256: file.sha256,
            path,
            packet_count,
        };
        self.received_files.push(report.clone());
        Ok(report)
    }

    fn write_outputs(
        mut self,
        out_dir: &Path,
        commands: Vec<LocalPeerCommandReport>,
        ok: bool,
    ) -> Result<LocalPeerScriptReport> {
        let station_a_dir = out_dir.join("station-a");
        let station_b_dir = out_dir.join("station-b");
        let packets_dir = out_dir.join("packets");
        fs::create_dir_all(&station_a_dir)
            .with_context(|| format!("creating {}", station_a_dir.display()))?;
        fs::create_dir_all(&station_b_dir)
            .with_context(|| format!("creating {}", station_b_dir.display()))?;
        fs::create_dir_all(&packets_dir)
            .with_context(|| format!("creating {}", packets_dir.display()))?;

        for packet in &mut self.packets {
            let path = packets_dir.join(packet_wav_filename(packet));
            AudioBuffer::new(packet.sample_rate, 1, packet.samples.clone())
                .with_context(|| "building local peer packet WAV")?
                .write_wav(&path)
                .with_context(|| format!("writing {}", path.display()))?;
            packet.wav_path = Some(path);
        }

        let station_a = self.station_a.chat.transcript();
        let station_a_app = self.station_a.app.state();
        let station_b = self.station_b.chat.transcript();
        let station_b_app = self.station_b.app.state();
        let station_a_transcript = station_a_dir.join("transcript.json");
        let station_a_app_path = station_a_dir.join("app-state.json");
        let station_a_log = station_a_dir.join("chat.log");
        let station_b_transcript = station_b_dir.join("transcript.json");
        let station_b_app_path = station_b_dir.join("app-state.json");
        let station_b_log = station_b_dir.join("chat.log");
        let artifacts = out_dir.join("artifacts.json");
        let session = out_dir.join("session.json");

        fs::write(
            &station_a_transcript,
            serde_json::to_string_pretty(&station_a)?,
        )
        .with_context(|| format!("writing {}", station_a_transcript.display()))?;
        fs::write(
            &station_a_app_path,
            serde_json::to_string_pretty(&station_a_app)?,
        )
        .with_context(|| format!("writing {}", station_a_app_path.display()))?;
        fs::write(&station_a_log, normalized_log(&station_a))
            .with_context(|| format!("writing {}", station_a_log.display()))?;
        fs::write(
            &station_b_transcript,
            serde_json::to_string_pretty(&station_b)?,
        )
        .with_context(|| format!("writing {}", station_b_transcript.display()))?;
        fs::write(
            &station_b_app_path,
            serde_json::to_string_pretty(&station_b_app)?,
        )
        .with_context(|| format!("writing {}", station_b_app_path.display()))?;
        fs::write(&station_b_log, normalized_log(&station_b))
            .with_context(|| format!("writing {}", station_b_log.display()))?;
        fs::write(&artifacts, serde_json::to_string_pretty(&self.packets)?)
            .with_context(|| format!("writing {}", artifacts.display()))?;

        let report = LocalPeerScriptReport {
            kind: "local-peer-script-report",
            backend: "native-local-peer",
            ok,
            channel: self.channel,
            station_a,
            station_a_app,
            station_b,
            station_b_app,
            commands,
            packets: self.packets,
            received_files: self.received_files,
            paths: LocalPeerOutputPaths {
                out_dir: out_dir.to_owned(),
                station_a_transcript,
                station_a_app: station_a_app_path,
                station_a_log,
                station_b_transcript,
                station_b_app: station_b_app_path,
                station_b_log,
                artifacts,
                session: session.clone(),
            },
        };
        fs::write(&session, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("writing {}", session.display()))?;
        Ok(report)
    }

    fn station(&self, side: LocalPeerSide) -> &StationRuntime {
        match side {
            LocalPeerSide::A => &self.station_a,
            LocalPeerSide::B => &self.station_b,
        }
    }

    fn station_mut(&mut self, side: LocalPeerSide) -> &mut StationRuntime {
        match side {
            LocalPeerSide::A => &mut self.station_a,
            LocalPeerSide::B => &mut self.station_b,
        }
    }
}

struct LocalPeerCommandOutcome {
    events: Vec<ChatEvent>,
    app_events: Vec<ChatAppEvent>,
    packet_sequence: Option<usize>,
}

impl LocalNodeSession {
    fn new(config: &LocalNodeScriptConfig) -> Result<Self> {
        let chat = FakeBackend::new(&config.station)?;
        let call_sign = chat.transcript().station.call_sign;
        let app = ChatAppModel::new(&call_sign)?;
        let peer_call = FakeBackend::new(&config.peer)?
            .transcript()
            .station
            .call_sign;
        if call_sign == peer_call {
            bail!("station and peer must be different callsigns");
        }
        let stream = open_local_node_stream(&config.mode)?;
        stream
            .set_nodelay(true)
            .context("configuring local node TCP stream")?;
        Ok(Self {
            protocol: AppProtocolState::new(&call_sign),
            call_sign,
            peer_call,
            chat,
            app,
            stream,
            config: PacketCodecConfig::default(),
            channel: config.channel,
            packets: Vec::new(),
            received_dir: config.out_dir.join("received"),
            received_files: Vec::new(),
        })
    }

    fn apply(&mut self, command: LocalNodeCommand) -> Result<LocalNodeCommandOutcome> {
        match command.action {
            LocalNodeAction::Connect => self.connect(),
            LocalNodeAction::ExpectConnect => self.expect_connect(),
            LocalNodeAction::Send(text) => self.send_text(&text),
            LocalNodeAction::ExpectMessage(text) => self.expect_message(&text),
            LocalNodeAction::Beacon(text) => self.beacon(&text),
            LocalNodeAction::ExpectBeacon(text) => self.expect_beacon(&text),
            LocalNodeAction::Cq(text) => self.cq(&text),
            LocalNodeAction::ExpectCq(text) => self.expect_cq(&text),
            LocalNodeAction::Mail { subject, body } => self.mail(&subject, &body),
            LocalNodeAction::ExpectMail { subject, body } => self.expect_mail(&subject, &body),
            LocalNodeAction::FileOffer {
                filename,
                byte_count,
                sha256,
                note,
            } => self.file_offer(&filename, byte_count, &sha256, note),
            LocalNodeAction::FileSend { path, note } => self.file_send(&path, note),
            LocalNodeAction::ExpectFileOffer {
                filename,
                byte_count,
                sha256,
                note,
            } => self.expect_file_offer(&filename, byte_count, &sha256, note),
            LocalNodeAction::ExpectFileTransfer {
                filename,
                byte_count,
                sha256,
                note,
            } => self.expect_file_transfer(&filename, byte_count, &sha256, note),
            LocalNodeAction::Disconnect => self.disconnect(),
            LocalNodeAction::ExpectDisconnect => self.expect_disconnect(),
            LocalNodeAction::Status => Ok(LocalNodeCommandOutcome {
                events: vec![self.chat.status()],
                app_events: vec![self.app.status()],
                packet_sequence: None,
            }),
        }
    }

    fn connect(&mut self) -> Result<LocalNodeCommandOutcome> {
        let event = self.chat.connect(&self.peer_call)?;
        let payload = format!("CONNECT {} {}", self.call_sign, self.peer_call);
        let packet = self.send_payload(&payload)?;
        Ok(LocalNodeCommandOutcome {
            events: vec![event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn expect_connect(&mut self) -> Result<LocalNodeCommandOutcome> {
        let packet = self.receive_payload()?;
        let transport = parse_transport_packet(&packet.payload_text)?;
        let TransportPacket::Connect { from, to } = transport else {
            bail!("expected CONNECT frame, got {}", packet.payload_text);
        };
        ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
        let event = self.chat.connect(&from)?;
        Ok(LocalNodeCommandOutcome {
            events: vec![event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn send_text(&mut self, text: &str) -> Result<LocalNodeCommandOutcome> {
        let event = self.chat.send_text(text)?;
        let ChatEvent::Message {
            from,
            to,
            text: event_text,
            ..
        } = &event
        else {
            bail!("send did not produce a message event");
        };
        let payload = format!("MSG {from} {to} {event_text}");
        let packet = self.send_payload(&payload)?;
        Ok(LocalNodeCommandOutcome {
            events: vec![event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn expect_message(&mut self, expected_text: &str) -> Result<LocalNodeCommandOutcome> {
        let packet = self.receive_payload()?;
        let transport = parse_transport_packet(&packet.payload_text)?;
        let TransportPacket::Message { from, to, text } = transport else {
            bail!("expected MSG frame, got {}", packet.payload_text);
        };
        ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
        if text != expected_text {
            bail!("expected message text {expected_text:?}, got {text:?}");
        }
        let event = self.chat.receive_text(&from, &text)?;
        Ok(LocalNodeCommandOutcome {
            events: vec![event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn beacon(&mut self, text: &str) -> Result<LocalNodeCommandOutcome> {
        let payload = app_protocol_payload(
            &mut self.protocol,
            AppTransportEnvelope::Beacon {
                from: self.call_sign.clone(),
                to: self.peer_call.clone(),
                text: text.to_owned(),
            },
        )?;
        let event = self.app.beacon(text)?;
        let packet = self.send_payload(&payload)?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn expect_beacon(&mut self, expected_text: &str) -> Result<LocalNodeCommandOutcome> {
        let packet = self.receive_payload()?;
        let transport = parse_transport_packet(&packet.payload_text)?;
        let TransportPacket::AppBeacon { from, to, text } = transport else {
            bail!("expected APP-BEACON frame, got {}", packet.payload_text);
        };
        ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
        if text != expected_text {
            bail!("expected beacon text {expected_text:?}, got {text:?}");
        }
        let event = self.app.observe_beacon(&from, &text)?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn cq(&mut self, text: &str) -> Result<LocalNodeCommandOutcome> {
        let payload = app_protocol_payload(
            &mut self.protocol,
            AppTransportEnvelope::Cq {
                from: self.call_sign.clone(),
                to: self.peer_call.clone(),
                text: text.to_owned(),
            },
        )?;
        let event = self.app.cq(text)?;
        let packet = self.send_payload(&payload)?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn expect_cq(&mut self, expected_text: &str) -> Result<LocalNodeCommandOutcome> {
        let packet = self.receive_payload()?;
        let transport = parse_transport_packet(&packet.payload_text)?;
        let TransportPacket::AppCq { from, to, text } = transport else {
            bail!("expected APP-CQ frame, got {}", packet.payload_text);
        };
        ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
        if text != expected_text {
            bail!("expected CQ text {expected_text:?}, got {text:?}");
        }
        let event = self.app.observe_cq(&from, &text)?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn mail(&mut self, subject: &str, body: &str) -> Result<LocalNodeCommandOutcome> {
        let payload = app_protocol_payload(
            &mut self.protocol,
            AppTransportEnvelope::Mail {
                from: self.call_sign.clone(),
                to: self.peer_call.clone(),
                subject: subject.to_owned(),
                body: body.to_owned(),
            },
        )?;
        let event = self.app.mailbox_message(&self.peer_call, subject, body)?;
        let packet = self.send_payload(&payload)?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn expect_mail(
        &mut self,
        expected_subject: &str,
        expected_body: &str,
    ) -> Result<LocalNodeCommandOutcome> {
        let packet = self.receive_payload()?;
        let transport = parse_transport_packet(&packet.payload_text)?;
        let TransportPacket::AppMail {
            from,
            to,
            subject,
            body,
        } = transport
        else {
            bail!("expected APP-MAIL frame, got {}", packet.payload_text);
        };
        ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
        if subject != expected_subject || body != expected_body {
            bail!(
                "expected mail {:?} | {:?}, got {:?} | {:?}",
                expected_subject,
                expected_body,
                subject,
                body
            );
        }
        let event = self
            .app
            .receive_mailbox_message(&from, &to, &subject, &body)?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn file_offer(
        &mut self,
        filename: &str,
        byte_count: u64,
        sha256: &str,
        note: Option<String>,
    ) -> Result<LocalNodeCommandOutcome> {
        let payload = app_protocol_payload(
            &mut self.protocol,
            AppTransportEnvelope::FileOffer {
                from: self.call_sign.clone(),
                to: self.peer_call.clone(),
                filename: filename.to_owned(),
                byte_count,
                sha256: sha256.to_owned(),
                note: note
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
            },
        )?;
        let event =
            self.app
                .file_offer(&self.peer_call, filename, byte_count, sha256, note.clone())?;
        let packet = self.send_payload(&payload)?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn file_send(&mut self, path: &Path, note: Option<String>) -> Result<LocalNodeCommandOutcome> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if bytes.is_empty() {
            bail!("cannot send empty file {}", path.display());
        }
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .context("file-send path must have a UTF-8 filename")?
            .to_owned();
        let protocol_packets = self.protocol.file_transfer_packets(
            &self.peer_call,
            &filename,
            &bytes,
            note.clone(),
            DEFAULT_FILE_CHUNK_DATA_BYTES,
        )?;
        let offer = protocol_packets
            .first()
            .context("file transfer did not produce a file offer")?;
        let (offer_filename, offer_byte_count, offer_sha256, offer_note) =
            offer.require_file_offer()?;
        let event = self.app.file_offer(
            &self.peer_call,
            &offer_filename,
            offer_byte_count,
            &offer_sha256,
            offer_note,
        )?;
        let mut first_packet_sequence = None;
        for protocol_packet in protocol_packets {
            let payload = encode_app_packet(&protocol_packet)?;
            let packet = self.send_payload(&payload)?;
            first_packet_sequence.get_or_insert(packet.sequence);
        }
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: first_packet_sequence,
        })
    }

    fn expect_file_offer(
        &mut self,
        expected_filename: &str,
        expected_byte_count: u64,
        expected_sha256: &str,
        expected_note: Option<String>,
    ) -> Result<LocalNodeCommandOutcome> {
        let packet = self.receive_payload()?;
        let transport = parse_transport_packet(&packet.payload_text)?;
        let TransportPacket::AppFileOffer {
            from,
            to,
            filename,
            byte_count,
            sha256,
            note,
        } = transport
        else {
            bail!("expected APP-FILE frame, got {}", packet.payload_text);
        };
        ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
        if filename != expected_filename
            || byte_count != expected_byte_count
            || sha256 != expected_sha256
            || note != expected_note
        {
            bail!(
                "expected file offer {} {} {} {:?}, got {} {} {} {:?}",
                expected_filename,
                expected_byte_count,
                expected_sha256,
                expected_note,
                filename,
                byte_count,
                sha256,
                note
            );
        }
        let event = self
            .app
            .receive_file_offer(&from, &to, &filename, byte_count, &sha256, note)?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(packet.sequence),
        })
    }

    fn expect_file_transfer(
        &mut self,
        expected_filename: &str,
        expected_byte_count: u64,
        expected_sha256: &str,
        expected_note: Option<String>,
    ) -> Result<LocalNodeCommandOutcome> {
        let offer_packet = self.receive_payload()?;
        let offer_protocol_packet = decode_app_packet(&offer_packet.payload_text)?
            .context("expected CBAPP/1 file transfer offer")?;
        let TransportPacket::AppFileOffer {
            from,
            to,
            filename,
            byte_count,
            sha256,
            note,
        } = app_protocol_packet_to_transport(offer_protocol_packet.clone())?
        else {
            bail!(
                "expected APP-FILE transfer offer, got {}",
                offer_packet.payload_text
            );
        };
        ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
        if filename != expected_filename
            || byte_count != expected_byte_count
            || sha256 != expected_sha256
            || note != expected_note
        {
            bail!(
                "expected file transfer offer {} {} {} {:?}, got {} {} {} {:?}",
                expected_filename,
                expected_byte_count,
                expected_sha256,
                expected_note,
                filename,
                byte_count,
                sha256,
                note
            );
        }
        let event = self
            .app
            .receive_file_offer(&from, &to, &filename, byte_count, &sha256, note)?;
        let mut protocol_packets = vec![offer_protocol_packet];
        let mut expected_total = None;
        let mut received_chunks = 0_u32;
        while expected_total.is_none_or(|total| received_chunks < total) {
            let packet = self.receive_payload()?;
            let protocol_packet = decode_app_packet(&packet.payload_text)?
                .context("expected CBAPP/1 file transfer chunk")?;
            if protocol_packet.kind != AppPacketKind::FileChunk {
                bail!("expected APP file chunk, got {}", packet.payload_text);
            }
            ensure_route(
                &protocol_packet.from,
                &protocol_packet.to,
                &self.peer_call,
                &self.call_sign,
            )?;
            let total = protocol_packet
                .fragment_total
                .context("file transfer chunk is missing fragment_total")?;
            if let Some(expected_total) = expected_total {
                if expected_total != total {
                    bail!("file transfer chunk total changed from {expected_total} to {total}");
                }
            } else {
                expected_total = Some(total);
            }
            received_chunks += 1;
            protocol_packets.push(protocol_packet);
        }
        let reassembled = reassemble_file_chunks(&protocol_packets)?;
        if reassembled.filename != expected_filename
            || reassembled.bytes.len() as u64 != expected_byte_count
            || reassembled.sha256 != expected_sha256
        {
            bail!(
                "reassembled file transfer did not match expected file {} {} {}",
                expected_filename,
                expected_byte_count,
                expected_sha256
            );
        }
        self.save_received_file(&from, &to, reassembled, protocol_packets.len())?;
        Ok(LocalNodeCommandOutcome {
            events: Vec::new(),
            app_events: vec![event],
            packet_sequence: Some(offer_packet.sequence),
        })
    }

    fn disconnect(&mut self) -> Result<LocalNodeCommandOutcome> {
        let payload = format!("DISCONNECT {} {}", self.call_sign, self.peer_call);
        let packet = self.send_payload(&payload)?;
        let event = self.chat.disconnect()?;
        Ok(LocalNodeCommandOutcome {
            events: vec![event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn expect_disconnect(&mut self) -> Result<LocalNodeCommandOutcome> {
        let packet = self.receive_payload()?;
        let transport = parse_transport_packet(&packet.payload_text)?;
        let TransportPacket::Disconnect { from, to } = transport else {
            bail!("expected DISCONNECT frame, got {}", packet.payload_text);
        };
        ensure_route(&from, &to, &self.peer_call, &self.call_sign)?;
        let event = self.chat.disconnect()?;
        Ok(LocalNodeCommandOutcome {
            events: vec![event],
            app_events: Vec::new(),
            packet_sequence: Some(packet.sequence),
        })
    }

    fn send_payload(&mut self, payload: &str) -> Result<LocalNodePacketReport> {
        ensure_transport_payload_size(payload)?;
        let signal = encode_packet_payload(payload.as_bytes(), self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        let samples = simulate_channel_samples(&signal.samples, self.channel);
        let decode = decode_packet_samples(&samples, self.config.sample_rate, self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !decode.ok || decode.payload_text.as_deref() != Some(payload) {
            bail!("local node outbound packet failed self-decode");
        }
        write_audio_frame(&mut self.stream, self.config.sample_rate, &samples)?;
        let (from, to) = transport_route(payload)?;
        let packet = LocalNodePacketReport {
            sequence: self.packets.len() + 1,
            direction: MessageDirection::Outbound,
            from,
            to,
            transport: "localhost-tcp-audio-frame",
            payload_text: payload.to_owned(),
            sample_rate: self.config.sample_rate,
            sample_count: samples.len(),
            wav_path: None,
            encode: Some(signal.report),
            decode,
            samples,
        };
        self.packets.push(packet.clone());
        Ok(packet)
    }

    fn receive_payload(&mut self) -> Result<LocalNodePacketReport> {
        let frame = read_audio_frame(&mut self.stream)?;
        let decode = decode_packet_samples(&frame.samples, frame.sample_rate, self.config)
            .map_err(|error| anyhow::anyhow!(error))?;
        if !decode.ok {
            bail!("local node inbound packet failed to decode");
        }
        let payload = decode
            .payload_text
            .clone()
            .context("local node inbound packet did not decode as UTF-8 text")?;
        let (from, to) = transport_route(&payload)?;
        let packet = LocalNodePacketReport {
            sequence: self.packets.len() + 1,
            direction: MessageDirection::Inbound,
            from,
            to,
            transport: "localhost-tcp-audio-frame",
            payload_text: payload,
            sample_rate: frame.sample_rate,
            sample_count: frame.samples.len(),
            wav_path: None,
            encode: None,
            decode,
            samples: frame.samples,
        };
        self.packets.push(packet.clone());
        Ok(packet)
    }

    fn save_received_file(
        &mut self,
        from: &str,
        to: &str,
        file: ReassembledFile,
        packet_count: usize,
    ) -> Result<ReceivedFileReport> {
        let filename = safe_received_filename(&file.filename);
        let station_dir = self.received_dir.join(&self.call_sign);
        fs::create_dir_all(&station_dir)
            .with_context(|| format!("creating {}", station_dir.display()))?;
        let path = station_dir.join(&filename);
        fs::write(&path, &file.bytes).with_context(|| format!("writing {}", path.display()))?;
        let report = ReceivedFileReport {
            station: self.call_sign.clone(),
            from: from.to_owned(),
            to: to.to_owned(),
            filename,
            byte_count: file.bytes.len() as u64,
            sha256: file.sha256,
            path,
            packet_count,
        };
        self.received_files.push(report.clone());
        Ok(report)
    }

    fn write_outputs(
        mut self,
        out_dir: &Path,
        commands: Vec<LocalNodeCommandReport>,
        ok: bool,
    ) -> Result<LocalNodeScriptReport> {
        let packets_dir = out_dir.join("packets");
        fs::create_dir_all(&packets_dir)
            .with_context(|| format!("creating {}", packets_dir.display()))?;
        for packet in &mut self.packets {
            let path = packets_dir.join(node_packet_wav_filename(packet));
            AudioBuffer::new(packet.sample_rate, 1, packet.samples.clone())
                .with_context(|| "building local node packet WAV")?
                .write_wav(&path)
                .with_context(|| format!("writing {}", path.display()))?;
            packet.wav_path = Some(path);
        }

        let transcript = self.chat.transcript();
        let app_state = self.app.state();
        let transcript_path = out_dir.join("transcript.json");
        let app_state_path = out_dir.join("app-state.json");
        let log_path = out_dir.join("chat.log");
        let artifacts_path = out_dir.join("artifacts.json");
        let session_path = out_dir.join("session.json");
        fs::write(&transcript_path, serde_json::to_string_pretty(&transcript)?)
            .with_context(|| format!("writing {}", transcript_path.display()))?;
        fs::write(&app_state_path, serde_json::to_string_pretty(&app_state)?)
            .with_context(|| format!("writing {}", app_state_path.display()))?;
        fs::write(&log_path, normalized_log(&transcript))
            .with_context(|| format!("writing {}", log_path.display()))?;
        fs::write(
            &artifacts_path,
            serde_json::to_string_pretty(&self.packets)?,
        )
        .with_context(|| format!("writing {}", artifacts_path.display()))?;

        let report = LocalNodeScriptReport {
            kind: "local-node-script-report",
            backend: "native-local-node",
            ok,
            channel: self.channel,
            station: transcript,
            app_state,
            peer_call: self.peer_call,
            commands,
            packets: self.packets,
            received_files: self.received_files,
            paths: LocalNodeOutputPaths {
                out_dir: out_dir.to_owned(),
                transcript: transcript_path,
                app_state: app_state_path,
                log: log_path,
                artifacts: artifacts_path,
                session: session_path.clone(),
            },
        };
        fs::write(&session_path, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("writing {}", session_path.display()))?;
        Ok(report)
    }
}

struct LocalNodeCommandOutcome {
    events: Vec<ChatEvent>,
    app_events: Vec<ChatAppEvent>,
    packet_sequence: Option<usize>,
}

fn open_local_node_stream(mode: &LocalNodeMode) -> Result<TcpStream> {
    match mode {
        LocalNodeMode::Listen { bind, ready_file } => {
            let listener = TcpListener::bind(bind)
                .with_context(|| format!("binding local node listener {bind}"))?;
            let address = listener
                .local_addr()
                .context("reading local node listener address")?;
            if let Some(path) = ready_file {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                fs::write(path, address.to_string())
                    .with_context(|| format!("writing {}", path.display()))?;
            }
            let (stream, _) = listener
                .accept()
                .context("accepting local node connection")?;
            Ok(stream)
        }
        LocalNodeMode::Connect { host } => {
            TcpStream::connect(host).with_context(|| format!("connecting local node peer {host}"))
        }
    }
}

fn spawn_live_reader(mut stream: TcpStream) -> Receiver<LocalLiveInbound> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(frame) = read_audio_frame(&mut stream) {
            if sender.send(LocalLiveInbound { frame }).is_err() {
                break;
            }
        }
    });
    receiver
}

impl StationRuntime {
    fn new(call_sign: &str, stream: TcpStream) -> Result<Self> {
        let chat = FakeBackend::new(call_sign)?;
        let call_sign = chat.transcript().station.call_sign;
        let app = ChatAppModel::new(&call_sign)?;
        Ok(Self {
            protocol: AppProtocolState::new(&call_sign),
            call_sign,
            chat,
            app,
            stream,
        })
    }
}

impl LocalPeerSide {
    fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

impl LocalPeerAction {
    fn label(&self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Send(_) => "send",
            Self::Beacon(_) => "beacon",
            Self::Cq(_) => "cq",
            Self::Mail { .. } => "mail",
            Self::FileOffer { .. } => "file-offer",
            Self::FileSend { .. } => "file-send",
            Self::Disconnect => "disconnect",
            Self::Status => "status",
        }
    }
}

impl LocalNodeAction {
    fn label(&self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::ExpectConnect => "expect-connect",
            Self::Send(_) => "send",
            Self::ExpectMessage(_) => "expect-msg",
            Self::Beacon(_) => "beacon",
            Self::ExpectBeacon(_) => "expect-beacon",
            Self::Cq(_) => "cq",
            Self::ExpectCq(_) => "expect-cq",
            Self::Mail { .. } => "mail",
            Self::ExpectMail { .. } => "expect-mail",
            Self::FileOffer { .. } => "file-offer",
            Self::FileSend { .. } => "file-send",
            Self::ExpectFileOffer { .. } => "expect-file-offer",
            Self::ExpectFileTransfer { .. } => "expect-file-send",
            Self::Disconnect => "disconnect",
            Self::ExpectDisconnect => "expect-disconnect",
            Self::Status => "status",
        }
    }
}

fn tcp_pair() -> Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind("127.0.0.1:0").context("binding localhost listener")?;
    let address = listener.local_addr().context("reading listener address")?;
    let client = TcpStream::connect(address).context("connecting local peer client")?;
    let (server, _) = listener.accept().context("accepting local peer server")?;
    client
        .set_nodelay(true)
        .context("configuring local peer client")?;
    server
        .set_nodelay(true)
        .context("configuring local peer server")?;
    Ok((client, server))
}

fn write_audio_frame(stream: &mut TcpStream, sample_rate: u32, samples: &[f32]) -> Result<()> {
    let sample_count = u32::try_from(samples.len()).context("audio frame too large")?;
    stream
        .write_all(AUDIO_FRAME_MAGIC)
        .context("writing audio frame magic")?;
    stream
        .write_all(&[AUDIO_FRAME_VERSION])
        .context("writing audio frame version")?;
    stream
        .write_all(&sample_rate.to_le_bytes())
        .context("writing audio frame sample rate")?;
    stream
        .write_all(&sample_count.to_le_bytes())
        .context("writing audio frame sample count")?;
    for sample in samples {
        stream
            .write_all(&sample.to_le_bytes())
            .context("writing audio frame sample")?;
    }
    stream.flush().context("flushing audio frame")
}

fn spawn_audio_frame_writer(
    mut stream: TcpStream,
    sample_rate: u32,
    samples: Vec<f32>,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || write_audio_frame(&mut stream, sample_rate, &samples))
}

fn join_audio_frame_writer(handle: thread::JoinHandle<Result<()>>) -> Result<()> {
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("local peer audio writer thread panicked"))?
}

fn read_audio_frame(stream: &mut TcpStream) -> Result<AudioFrame> {
    let mut magic = [0_u8; 4];
    stream
        .read_exact(&mut magic)
        .context("reading audio frame magic")?;
    if &magic != AUDIO_FRAME_MAGIC {
        bail!("invalid audio frame magic");
    }

    let mut version = [0_u8; 1];
    stream
        .read_exact(&mut version)
        .context("reading audio frame version")?;
    if version[0] != AUDIO_FRAME_VERSION {
        bail!("unsupported audio frame version {}", version[0]);
    }

    let sample_rate = read_u32(stream).context("reading audio frame sample rate")?;
    let sample_count = read_u32(stream).context("reading audio frame sample count")? as usize;
    let byte_count = sample_count
        .checked_mul(std::mem::size_of::<f32>())
        .context("audio frame byte count overflow")?;
    let mut bytes = vec![0_u8; byte_count];
    stream
        .read_exact(&mut bytes)
        .context("reading audio frame samples")?;
    let samples = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    Ok(AudioFrame {
        sample_rate,
        samples,
    })
}

fn read_u32(stream: &mut TcpStream) -> Result<u32> {
    let mut bytes = [0_u8; 4];
    stream.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn parse_local_peer_command(line: &str) -> Result<LocalPeerCommand> {
    let mut parts = line.splitn(3, char::is_whitespace);
    let side = parse_side(parts.next().unwrap_or_default())?;
    let verb = parts.next().unwrap_or_default().to_ascii_uppercase();
    let rest = parts.next().unwrap_or_default().trim();
    let action = match verb.as_str() {
        "CONNECT" => LocalPeerAction::Connect,
        "SEND" => {
            if rest.is_empty() {
                bail!("missing SEND text");
            }
            LocalPeerAction::Send(rest.to_owned())
        }
        "BEACON" | "BCN" => {
            if rest.is_empty() {
                bail!("missing BEACON text");
            }
            LocalPeerAction::Beacon(rest.to_owned())
        }
        "CQ" => {
            if rest.is_empty() {
                bail!("missing CQ text");
            }
            LocalPeerAction::Cq(rest.to_owned())
        }
        "MAIL" | "MAILBOX" | "VMAIL" => {
            let (subject, body) = split_mailbox(rest, &verb)?;
            LocalPeerAction::Mail { subject, body }
        }
        "FILE-OFFER" | "FILE_OFFER" | "FILE" => {
            let (filename, byte_count, sha256, note) = split_file_offer(rest, &verb)?;
            LocalPeerAction::FileOffer {
                filename,
                byte_count,
                sha256,
                note,
            }
        }
        "FILE-SEND" | "FILE_SEND" | "SEND-FILE" | "SEND_FILE" => {
            let (path, note) = split_file_send(rest, &verb)?;
            LocalPeerAction::FileSend { path, note }
        }
        "DISCONNECT" => LocalPeerAction::Disconnect,
        "STATUS" => LocalPeerAction::Status,
        "" => bail!("missing local peer command verb"),
        _ => bail!("unknown local peer command verb {verb}"),
    };
    Ok(LocalPeerCommand { side, action })
}

fn parse_local_node_command(line: &str) -> Result<LocalNodeCommand> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next().unwrap_or_default().to_ascii_uppercase();
    let rest = parts.next().unwrap_or_default().trim();
    let action = match verb.as_str() {
        "CONNECT" => LocalNodeAction::Connect,
        "EXPECT-CONNECT" | "EXPECT_CONNECT" => LocalNodeAction::ExpectConnect,
        "SEND" => {
            if rest.is_empty() {
                bail!("missing SEND text");
            }
            LocalNodeAction::Send(rest.to_owned())
        }
        "EXPECT-MSG" | "EXPECT_MSG" | "EXPECT-MESSAGE" | "EXPECT_MESSAGE" => {
            if rest.is_empty() {
                bail!("missing EXPECT-MSG text");
            }
            LocalNodeAction::ExpectMessage(rest.to_owned())
        }
        "BEACON" | "BCN" => {
            if rest.is_empty() {
                bail!("missing BEACON text");
            }
            LocalNodeAction::Beacon(rest.to_owned())
        }
        "EXPECT-BEACON" | "EXPECT_BEACON" | "EXPECT-BCN" | "EXPECT_BCN" => {
            if rest.is_empty() {
                bail!("missing EXPECT-BEACON text");
            }
            LocalNodeAction::ExpectBeacon(rest.to_owned())
        }
        "CQ" => {
            if rest.is_empty() {
                bail!("missing CQ text");
            }
            LocalNodeAction::Cq(rest.to_owned())
        }
        "EXPECT-CQ" | "EXPECT_CQ" => {
            if rest.is_empty() {
                bail!("missing EXPECT-CQ text");
            }
            LocalNodeAction::ExpectCq(rest.to_owned())
        }
        "MAIL" | "MAILBOX" | "VMAIL" => {
            let (subject, body) = split_mailbox(rest, &verb)?;
            LocalNodeAction::Mail { subject, body }
        }
        "EXPECT-MAIL" | "EXPECT_MAIL" | "EXPECT-MAILBOX" | "EXPECT_MAILBOX" => {
            let (subject, body) = split_mailbox(rest, &verb)?;
            LocalNodeAction::ExpectMail { subject, body }
        }
        "FILE-OFFER" | "FILE_OFFER" | "FILE" => {
            let (filename, byte_count, sha256, note) = split_file_offer(rest, &verb)?;
            LocalNodeAction::FileOffer {
                filename,
                byte_count,
                sha256,
                note,
            }
        }
        "FILE-SEND" | "FILE_SEND" | "SEND-FILE" | "SEND_FILE" => {
            let (path, note) = split_file_send(rest, &verb)?;
            LocalNodeAction::FileSend { path, note }
        }
        "EXPECT-FILE-OFFER" | "EXPECT_FILE_OFFER" | "EXPECT-FILE" | "EXPECT_FILE" => {
            let (filename, byte_count, sha256, note) = split_file_offer(rest, &verb)?;
            LocalNodeAction::ExpectFileOffer {
                filename,
                byte_count,
                sha256,
                note,
            }
        }
        "EXPECT-FILE-SEND"
        | "EXPECT_FILE_SEND"
        | "EXPECT-FILE-TRANSFER"
        | "EXPECT_FILE_TRANSFER" => {
            let (filename, byte_count, sha256, note) = split_file_offer(rest, &verb)?;
            LocalNodeAction::ExpectFileTransfer {
                filename,
                byte_count,
                sha256,
                note,
            }
        }
        "DISCONNECT" => LocalNodeAction::Disconnect,
        "EXPECT-DISCONNECT" | "EXPECT_DISCONNECT" => LocalNodeAction::ExpectDisconnect,
        "STATUS" => LocalNodeAction::Status,
        "" => bail!("missing local node command verb"),
        _ => bail!("unknown local node command verb {verb}"),
    };
    Ok(LocalNodeCommand { action })
}

fn parse_side(side: &str) -> Result<LocalPeerSide> {
    match side.to_ascii_uppercase().as_str() {
        "A" | "STATION-A" | "STATION_A" => Ok(LocalPeerSide::A),
        "B" | "STATION-B" | "STATION_B" => Ok(LocalPeerSide::B),
        "" => bail!("missing local peer side"),
        _ => bail!("unknown local peer side {side:?}; expected A or B"),
    }
}

fn split_mailbox(rest: &str, verb: &str) -> Result<(String, String)> {
    let Some((subject, body)) = rest.split_once('|') else {
        bail!("missing {verb} subject | body");
    };
    let subject = subject.trim();
    let body = body.trim();
    if subject.is_empty() || body.is_empty() {
        bail!("missing {verb} subject | body");
    }
    Ok((subject.to_owned(), body.to_owned()))
}

fn split_file_offer(rest: &str, verb: &str) -> Result<(String, u64, String, Option<String>)> {
    let mut parts = rest.splitn(4, char::is_whitespace);
    let filename = parts.next().unwrap_or_default().trim();
    let byte_count = parts.next().unwrap_or_default().trim();
    let sha256 = parts.next().unwrap_or_default().trim();
    let note = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if filename.is_empty() || byte_count.is_empty() || sha256.is_empty() {
        bail!("missing {verb} filename byte-count sha256 [note]");
    }
    let byte_count = byte_count
        .parse::<u64>()
        .with_context(|| format!("invalid byte count for {verb}: {byte_count}"))?;
    Ok((filename.to_owned(), byte_count, sha256.to_owned(), note))
}

fn split_file_send(rest: &str, verb: &str) -> Result<(PathBuf, Option<String>)> {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let path = parts.next().unwrap_or_default().trim();
    if path.is_empty() {
        bail!("missing file path for {verb}");
    }
    let note = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok((PathBuf::from(path), note))
}

fn normalize_live_call(call: &str) -> Result<String> {
    Ok(FakeBackend::new(call)?.transcript().station.call_sign)
}

fn safe_received_filename(filename: &str) -> String {
    Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("received.bin")
        .to_owned()
}

#[cfg(test)]
fn app_envelope_payload(envelope: AppTransportEnvelope) -> Result<String> {
    let mut protocol = AppProtocolState::new(envelope.source_call());
    app_protocol_payload(&mut protocol, envelope)
}

fn app_protocol_payload(
    protocol: &mut AppProtocolState,
    envelope: AppTransportEnvelope,
) -> Result<String> {
    let packet = envelope.into_protocol_packet(protocol);
    encode_app_packet(&packet)
}

fn ensure_transport_payload_size(payload: &str) -> Result<()> {
    let len = payload.len();
    if len > MAX_TRANSPORT_PAYLOAD_BYTES {
        bail!(
            "transport payload is {len} bytes; maximum one-packet payload is {MAX_TRANSPORT_PAYLOAD_BYTES} bytes"
        );
    }
    Ok(())
}

fn parse_app_envelope(payload: &str) -> Result<Option<TransportPacket>> {
    let Some(packet) = decode_app_packet(payload)? else {
        return Ok(None);
    };
    Ok(Some(app_protocol_packet_to_transport(packet)?))
}

fn app_protocol_packet_to_transport(packet: AppProtocolPacket) -> Result<TransportPacket> {
    match packet.kind {
        AppPacketKind::Beacon => {
            let text = packet.require_text("text")?;
            Ok(TransportPacket::AppBeacon {
                from: packet.from,
                to: packet.to,
                text,
            })
        }
        AppPacketKind::Cq => {
            let text = packet.require_text("text")?;
            Ok(TransportPacket::AppCq {
                from: packet.from,
                to: packet.to,
                text,
            })
        }
        AppPacketKind::Mail => {
            let (subject, body) = packet.require_subject_body()?;
            Ok(TransportPacket::AppMail {
                from: packet.from,
                to: packet.to,
                subject,
                body,
            })
        }
        AppPacketKind::FileOffer => {
            let (filename, byte_count, sha256, note) = packet.require_file_offer()?;
            Ok(TransportPacket::AppFileOffer {
                from: packet.from,
                to: packet.to,
                filename,
                byte_count,
                sha256,
                note,
            })
        }
        AppPacketKind::FileChunk => {
            let file_id = packet
                .file_id
                .context("CBAPP/1 file chunk is missing file_id")?;
            let filename = packet
                .filename
                .context("CBAPP/1 file chunk is missing filename")?;
            let fragment_index = packet
                .fragment_index
                .context("CBAPP/1 file chunk is missing fragment_index")?;
            let fragment_total = packet
                .fragment_total
                .context("CBAPP/1 file chunk is missing fragment_total")?;
            Ok(TransportPacket::AppFileChunk {
                from: packet.from,
                to: packet.to,
                file_id,
                filename,
                fragment_index,
                fragment_total,
            })
        }
        AppPacketKind::Fragment => {
            let message_id = packet
                .message_id
                .context("CBAPP/1 fragment is missing message_id")?;
            let fragment_index = packet
                .fragment_index
                .context("CBAPP/1 fragment is missing fragment_index")?;
            let fragment_total = packet
                .fragment_total
                .context("CBAPP/1 fragment is missing fragment_total")?;
            Ok(TransportPacket::AppFragment {
                from: packet.from,
                to: packet.to,
                message_id,
                fragment_index,
                fragment_total,
            })
        }
        AppPacketKind::Ack | AppPacketKind::Receipt => {
            let receipt_for = packet
                .ack_for()
                .context("CBAPP/1 ack is missing receipt_for")?
                .to_owned();
            let delivery = packet.delivery.unwrap_or(AppDeliveryState::Acknowledged);
            Ok(TransportPacket::AppAck {
                from: packet.from,
                to: packet.to,
                receipt_for,
                delivery,
            })
        }
    }
}

fn parse_transport_packet(payload: &str) -> Result<TransportPacket> {
    if let Some(packet) = parse_app_envelope(payload)? {
        return Ok(packet);
    }

    let mut parts = payload.splitn(4, char::is_whitespace);
    let verb = parts.next().unwrap_or_default();
    let from = parts.next().unwrap_or_default().trim();
    let to = parts.next().unwrap_or_default().trim();
    if from.is_empty() || to.is_empty() {
        bail!("decoded transport packet is missing callsigns");
    }
    match verb {
        "CONNECT" => Ok(TransportPacket::Connect {
            from: from.to_owned(),
            to: to.to_owned(),
        }),
        "MSG" => {
            let text = parts.next().unwrap_or_default().trim();
            if text.is_empty() {
                bail!("decoded MSG packet is missing text");
            }
            Ok(TransportPacket::Message {
                from: from.to_owned(),
                to: to.to_owned(),
                text: text.to_owned(),
            })
        }
        "APP-BEACON" => {
            let text = parts.next().unwrap_or_default().trim();
            if text.is_empty() {
                bail!("decoded APP-BEACON packet is missing text");
            }
            Ok(TransportPacket::AppBeacon {
                from: from.to_owned(),
                to: to.to_owned(),
                text: text.to_owned(),
            })
        }
        "APP-CQ" => {
            let text = parts.next().unwrap_or_default().trim();
            if text.is_empty() {
                bail!("decoded APP-CQ packet is missing text");
            }
            Ok(TransportPacket::AppCq {
                from: from.to_owned(),
                to: to.to_owned(),
                text: text.to_owned(),
            })
        }
        "APP-MAIL" => {
            let rest = parts.next().unwrap_or_default().trim();
            let (subject, body) = split_mailbox(rest, "APP-MAIL")?;
            Ok(TransportPacket::AppMail {
                from: from.to_owned(),
                to: to.to_owned(),
                subject,
                body,
            })
        }
        "APP-FILE" => {
            let rest = parts.next().unwrap_or_default().trim();
            let (filename, byte_count, sha256, note) = split_file_offer(rest, "APP-FILE")?;
            Ok(TransportPacket::AppFileOffer {
                from: from.to_owned(),
                to: to.to_owned(),
                filename,
                byte_count,
                sha256,
                note,
            })
        }
        "DISCONNECT" => Ok(TransportPacket::Disconnect {
            from: from.to_owned(),
            to: to.to_owned(),
        }),
        _ => bail!("unknown decoded transport packet verb {verb:?}"),
    }
}

fn transport_route(payload: &str) -> Result<(String, String)> {
    match parse_transport_packet(payload)? {
        TransportPacket::Connect { from, to }
        | TransportPacket::Message { from, to, .. }
        | TransportPacket::AppBeacon { from, to, .. }
        | TransportPacket::AppCq { from, to, .. }
        | TransportPacket::AppMail { from, to, .. }
        | TransportPacket::AppFileOffer { from, to, .. }
        | TransportPacket::AppFileChunk { from, to, .. }
        | TransportPacket::AppFragment { from, to, .. }
        | TransportPacket::AppAck { from, to, .. }
        | TransportPacket::Disconnect { from, to } => Ok((from, to)),
    }
}

fn ensure_route(
    actual_from: &str,
    actual_to: &str,
    expected_from: &str,
    expected_to: &str,
) -> Result<()> {
    if actual_from != expected_from || actual_to != expected_to {
        bail!(
            "decoded route mismatch: expected {expected_from}->{expected_to}, got {actual_from}->{actual_to}"
        );
    }
    Ok(())
}

fn prepare_output_dir(out_dir: &Path, overwrite: bool) -> Result<()> {
    if out_dir.exists() && out_dir.read_dir()?.next().is_some() && !overwrite {
        bail!(
            "output directory {} is not empty; pass --overwrite to replace generated files",
            out_dir.display()
        );
    }
    fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))
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

fn packet_wav_filename(packet: &LocalPeerPacketReport) -> String {
    format!(
        "packet-{sequence:03}-{from}-to-{to}.wav",
        sequence = packet.sequence,
        from = sanitize_filename(&packet.from),
        to = sanitize_filename(&packet.to)
    )
}

fn node_packet_wav_filename(packet: &LocalNodePacketReport) -> String {
    format!(
        "packet-{sequence:03}-{direction}-{from}-to-{to}.wav",
        sequence = packet.sequence,
        direction = direction_label(packet.direction),
        from = sanitize_filename(&packet.from),
        to = sanitize_filename(&packet.to)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    #[test]
    fn local_peer_script_exchanges_packetized_messages() {
        let dir = tempdir().expect("tempdir");
        let script = "A CONNECT\nA SEND hello peer\nB SEND roger local\nA DISCONNECT\n";
        let report = run_local_peer_script(
            LocalPeerScriptConfig {
                station_a: "ja1tst".to_owned(),
                station_b: "ja1qso".to_owned(),
                out_dir: dir.path().join("session"),
                overwrite: false,
                channel: ChannelConfig::default(),
            },
            script,
        )
        .expect("local peer script");

        assert!(report.ok);
        assert_eq!(report.station_a.station.call_sign, "JA1TST");
        assert_eq!(report.station_b.station.call_sign, "JA1QSO");
        assert_eq!(report.station_a.messages.len(), 2);
        assert_eq!(report.station_b.messages.len(), 2);
        assert_eq!(report.packets.len(), 4);
        assert_eq!(
            report.packets[1].payload_text,
            "MSG JA1TST JA1QSO hello peer"
        );
        assert_eq!(
            report.packets[2].payload_text,
            "MSG JA1QSO JA1TST roger local"
        );
        assert!(report.packets.iter().all(|packet| packet.decode.ok));
        assert!(report.paths.station_a_log.exists());
        assert!(report.paths.station_b_log.exists());
        assert!(
            report
                .packets
                .iter()
                .all(|packet| packet.wav_path.is_some())
        );
    }

    #[test]
    fn local_peer_script_exchanges_packetized_app_features() {
        let dir = tempdir().expect("tempdir");
        let script = concat!(
            "A BEACON monitoring 14.105 USB\n",
            "B CQ testing app packets\n",
            "A MAIL Test subject | Synthetic mailbox body\n",
            "B FILE-OFFER sample.txt 42 ",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 ",
            "metadata only\n",
        );
        let report = run_local_peer_script(
            LocalPeerScriptConfig {
                station_a: "ja1tst".to_owned(),
                station_b: "ja1qso".to_owned(),
                out_dir: dir.path().join("session"),
                overwrite: false,
                channel: ChannelConfig::default(),
            },
            script,
        )
        .expect("local peer app script");

        assert!(report.ok);
        assert_eq!(report.station_a.messages.len(), 0);
        assert_eq!(report.station_b.messages.len(), 0);
        assert_eq!(report.station_a_app.beacons.len(), 1);
        assert_eq!(report.station_b_app.beacons.len(), 1);
        assert_eq!(report.station_a_app.cq_calls[0].from, "JA1QSO");
        assert_eq!(report.station_b_app.cq_calls[0].from, "JA1QSO");
        assert_eq!(report.station_a_app.mailbox[0].from, "JA1TST");
        assert_eq!(report.station_b_app.mailbox[0].to, "JA1QSO");
        assert_eq!(report.station_a_app.file_offers[0].to, "JA1TST");
        assert_eq!(report.station_b_app.file_offers[0].from, "JA1QSO");
        assert_eq!(report.commands[0].app_events.len(), 2);
        assert_eq!(report.commands[2].action.as_deref(), Some("mail"));
        assert_eq!(report.packets.len(), 4);
        assert!(report.packets[0].payload_text.starts_with("CBAPP/1\n"));
        assert!(
            report.packets[0]
                .payload_text
                .contains("\"kind\":\"beacon\"")
        );
        assert!(report.packets[2].payload_text.starts_with("CBAPP/1\n"));
        assert!(report.packets[2].payload_text.contains("\"kind\":\"mail\""));
        assert!(
            report.packets[2]
                .payload_text
                .contains("\"subject\":\"Test subject\"")
        );
        assert!(report.paths.station_a_app.exists());
        assert!(report.paths.station_b_app.exists());
    }

    #[test]
    fn local_peer_script_sends_real_file_chunks_and_saves_received_file() {
        let dir = tempdir().expect("tempdir");
        let input = dir.path().join("payload.txt");
        let bytes = b"real file transfer payload split across several clean-room CBAPP chunks";
        fs::write(&input, bytes).expect("write input");
        let script = format!("A FILE-SEND {} test file transfer\n", input.display());
        let report = run_local_peer_script(
            LocalPeerScriptConfig {
                station_a: "ja1tst".to_owned(),
                station_b: "ja1qso".to_owned(),
                out_dir: dir.path().join("session"),
                overwrite: false,
                channel: ChannelConfig::default(),
            },
            &script,
        )
        .expect("local peer file-send script");

        assert!(report.ok);
        assert_eq!(report.commands[0].action.as_deref(), Some("file-send"));
        assert_eq!(report.commands[0].app_events.len(), 2);
        assert!(report.packets.len() > 2);
        assert_eq!(report.station_a_app.file_offers[0].filename, "payload.txt");
        assert_eq!(report.station_b_app.file_offers[0].filename, "payload.txt");
        assert_eq!(report.received_files.len(), 1);
        assert_eq!(report.received_files[0].station, "JA1QSO");
        assert_eq!(report.received_files[0].from, "JA1TST");
        assert_eq!(report.received_files[0].to, "JA1QSO");
        assert_eq!(report.received_files[0].filename, "payload.txt");
        assert_eq!(
            fs::read(&report.received_files[0].path).expect("read received"),
            bytes
        );
    }

    #[test]
    fn app_transport_envelope_roundtrips_json() {
        let payload = app_envelope_payload(AppTransportEnvelope::Mail {
            from: "JA1TST".to_owned(),
            to: "JA1QSO".to_owned(),
            subject: "Subject".to_owned(),
            body: "Body text".to_owned(),
        })
        .expect("encode envelope");

        assert!(payload.starts_with("CBAPP/1\n"));
        assert!(payload.contains("\"kind\":\"mail\""));
        assert!(payload.contains("\"from\":\"JA1TST\""));

        let packet = parse_transport_packet(&payload).expect("parse envelope");
        assert_eq!(
            packet,
            TransportPacket::AppMail {
                from: "JA1TST".to_owned(),
                to: "JA1QSO".to_owned(),
                subject: "Subject".to_owned(),
                body: "Body text".to_owned(),
            }
        );
    }

    #[test]
    fn legacy_app_transport_packets_still_parse() {
        assert_eq!(
            parse_transport_packet("APP-BEACON JA1TST JA1QSO monitoring").unwrap(),
            TransportPacket::AppBeacon {
                from: "JA1TST".to_owned(),
                to: "JA1QSO".to_owned(),
                text: "monitoring".to_owned(),
            }
        );
        assert_eq!(
            parse_transport_packet(
                "APP-FILE JA1QSO JA1TST sample.txt 42 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 metadata only"
            )
            .unwrap(),
            TransportPacket::AppFileOffer {
                from: "JA1QSO".to_owned(),
                to: "JA1TST".to_owned(),
                filename: "sample.txt".to_owned(),
                byte_count: 42,
                sha256:
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                        .to_owned(),
                note: Some("metadata only".to_owned()),
            }
        );
    }

    #[test]
    fn local_peer_script_rejects_oversized_transport_payload() {
        let dir = tempdir().expect("tempdir");
        let script = format!("A BEACON {}\n", "x".repeat(MAX_TRANSPORT_PAYLOAD_BYTES));
        let report = run_local_peer_script(
            LocalPeerScriptConfig {
                station_a: "ja1tst".to_owned(),
                station_b: "ja1qso".to_owned(),
                out_dir: dir.path().join("session"),
                overwrite: false,
                channel: ChannelConfig::default(),
            },
            &script,
        )
        .expect("oversized app script report");

        assert!(!report.ok);
        assert_eq!(report.packets.len(), 0);
        assert_eq!(report.station_a_app.beacons.len(), 0);
        let error = report.commands[0].error.as_deref().expect("error");
        assert!(error.contains("maximum one-packet payload"));
    }

    #[test]
    fn local_live_nodes_exchange_interactive_events() {
        let dir = tempdir().expect("tempdir");
        let ready_file = dir.path().join("listener.ready");
        let listener_ready_file = ready_file.clone();
        let listener_handle = thread::spawn(move || {
            LocalLiveNode::new(LocalLiveNodeConfig {
                station: "ja1qso".to_owned(),
                peer: "ja1tst".to_owned(),
                mode: LocalNodeMode::Listen {
                    bind: "127.0.0.1:0".to_owned(),
                    ready_file: Some(listener_ready_file),
                },
                channel: ChannelConfig::default(),
            })
            .expect("listener")
        });
        let address = wait_ready_file(&ready_file);
        let mut connector = LocalLiveNode::new(LocalLiveNodeConfig {
            station: "ja1tst".to_owned(),
            peer: "ja1qso".to_owned(),
            mode: LocalNodeMode::Connect { host: address },
            channel: ChannelConfig::default(),
        })
        .expect("connector");
        let mut listener = listener_handle.join().expect("listener thread");

        let events = connector.connect("ja1qso").expect("connect");
        assert!(matches!(events[0], ChatEvent::Connected { .. }));
        let events = wait_live_chat_events(&mut listener);
        assert!(matches!(events[0], ChatEvent::Connected { .. }));

        connector.send_text("hello live").expect("send connector");
        let events = wait_live_chat_events(&mut listener);
        assert!(matches!(
            &events[0],
            ChatEvent::Message { text, .. } if text == "hello live"
        ));

        listener.send_text("roger live").expect("send listener");
        let events = wait_live_chat_events(&mut connector);
        assert!(matches!(
            &events[0],
            ChatEvent::Message { text, .. } if text == "roger live"
        ));

        connector.disconnect().expect("disconnect connector");
        let events = wait_live_chat_events(&mut listener);
        assert!(matches!(events[0], ChatEvent::Disconnected { .. }));
        assert_eq!(connector.transcript().messages.len(), 2);
        assert_eq!(listener.transcript().messages.len(), 2);
        assert_eq!(connector.packets.len(), 4);
        assert_eq!(listener.packets.len(), 4);
    }

    fn wait_ready_file(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
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

    fn wait_live_chat_events(node: &mut LocalLiveNode) -> Vec<ChatEvent> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let events = node
                .poll_events()
                .expect("poll live node")
                .into_iter()
                .filter_map(|event| match event {
                    LocalLiveEvent::Chat(event) => Some(event),
                    LocalLiveEvent::App(_) => None,
                })
                .collect::<Vec<_>>();
            if !events.is_empty() {
                return events;
            }
            assert!(
                Instant::now() <= deadline,
                "timed out waiting for live events"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

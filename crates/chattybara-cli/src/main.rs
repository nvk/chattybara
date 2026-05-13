mod app_protocol;
mod audio_devices;
mod hamlib;
mod ic705;
mod live_audio;
mod local_peer;
mod radio_profile;
mod tui;

use anyhow::{Context, Result, bail};
use app_protocol::{
    AppDeliveryState, AppProtocolState, DEFAULT_FILE_CHUNK_DATA_BYTES, DEFAULT_FRAGMENT_DATA_BYTES,
    SimulatedAppLinkConfig, encode_app_packet, reassemble_file_chunks, reassemble_fragments,
    simulate_reliable_delivery,
};
use audio_devices::{AudioDeviceRequest, enumerate_audio_devices};
use chattybara_chat::{
    compare_fake_script_to_simple_log, compare_peer_logs, parse_simple_log, run_app_script,
    run_fake_script,
};
use chattybara_station::{
    ModeId, StationAction, StationSafetyState, action_guard_report, built_in_modes,
    fake_events_for_mode, mode_by_label, read_event_log, replay_summary, write_event_log,
};
use chattybara_winlink::{
    B2fProposal, CredentialSource, DEFAULT_CMS_HOST, DEFAULT_CMS_PORT, DEFAULT_TELNET_TIMEOUT_MS,
    MailFolder, TelnetCmsConfig, WinlinkAccount, WinlinkAttachment, WinlinkStore,
    WinlinkTransportKind, default_store_path, fake_sync, guarded_dry_run_sync_report,
    normalize_call, telnet_cms_check, telnet_cms_receive_sync, transport_plan_report,
    winlink_password_from_env,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use hamlib::{
    DEFAULT_RIGCTLD_HOST, HamlibConfig, HamlibPttState, hamlib_get_frequency, hamlib_get_mode,
    hamlib_set_ptt, hamlib_status,
};
use ic705::{
    Ic705CivOperation, Ic705CivSerialConfig, build_ic705_civ_frame_report, default_ic705_profile,
    ic705_profile_toml, load_ic705_profile, run_ic705_civ_serial, validate_ic705_profile,
};
use live_audio::{LiveAudioModemConfig, run_live_audio_modem};
use local_peer::{
    LocalNodeMode, LocalNodeScriptConfig, LocalPeerScriptConfig, run_local_node_script,
    run_local_peer_script,
};
use orca_audio::{AudioBuffer, LoopbackConfig, virtual_loopback};
use orca_corpus::{
    file_sha256_hex, load_manifest, validate_manifest, validate_observation_manifest,
};
use orca_dsp::{
    AnalysisConfig, ChannelConfig, SyntheticSignal, SyntheticWavConfig, analyze_wav, bandpass_fft,
    build_analysis_trace, estimate_tone, simulate_channel, soft_decision_trace, track_frequency,
    write_synthetic_wav,
};
use orca_frames::{
    CandidateClass, PacketCodecConfig, PacketDecodeReport, classify_trace, decode_packet_samples,
    encode_packet_payload, run_receive_pipeline,
};
use orca_host::ModemShell;
use radio_profile::{
    RadioProfileTemplate, default_radio_profile, load_radio_profile, radio_profile_toml,
    validate_radio_profile,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tui::{
    ChatTuiBackend, ChatTuiConfig, ChatTuiLocalNodeConfig, ChatTuiSetupConfig, run_chat_tui,
};

const DEFAULT_SAMPLE_STATION: &str = "JA1TST";
const LOCAL_SETTINGS_ENV: &str = "CHATTYBARA_SETTINGS";

#[derive(Debug, Parser)]
#[command(name = "chattybara")]
#[command(about = "terminal radio chat client with the clean-room orca modem stack")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Audio(AudioArgs),
    Chat(ChatArgs),
    Corpus(CorpusArgs),
    Decode(DecodeArgs),
    Dsp(DspArgs),
    Frames(FramesArgs),
    Fixture(FixtureArgs),
    Host(HostArgs),
    Inspect(InspectArgs),
    Lab(LabArgs),
    Modem(ModemArgs),
    Rig(RigArgs),
    Station(StationArgs),
    Simulate(SimulateArgs),
    Winlink(WinlinkArgs),
}

#[derive(Debug, Args)]
struct ChatArgs {
    #[command(subcommand)]
    command: ChatCommand,
}

#[derive(Debug, Subcommand)]
enum ChatCommand {
    AppScript(ChatAppScriptArgs),
    ComparePeerLogs(ChatComparePeerLogsArgs),
    CompareSessionLogs(ChatCompareSessionLogsArgs),
    CompareScriptLog(ChatCompareScriptLogArgs),
    FakeScript(ChatFakeScriptArgs),
    LocalNodeScript(ChatLocalNodeScriptArgs),
    LocalPeerScript(ChatLocalPeerScriptArgs),
    ParseLog(ChatParseLogArgs),
    Tui(ChatTuiArgs),
}

#[derive(Debug, Args)]
struct ChatAppScriptArgs {
    script: PathBuf,
    #[arg(long)]
    station: String,
}

#[derive(Debug, Args)]
struct ChatComparePeerLogsArgs {
    station_a_log: PathBuf,
    station_b_log: PathBuf,
    #[arg(long = "station-a")]
    station_a: String,
    #[arg(long = "station-b")]
    station_b: String,
}

#[derive(Debug, Args)]
struct ChatCompareSessionLogsArgs {
    station_a_path: PathBuf,
    station_b_path: Option<PathBuf>,
    #[arg(long = "station-a")]
    station_a: String,
    #[arg(long = "station-b")]
    station_b: String,
}

#[derive(Debug, Args)]
struct ChatCompareScriptLogArgs {
    script: PathBuf,
    log: PathBuf,
    #[arg(long)]
    station: String,
}

#[derive(Debug, Args)]
struct ChatFakeScriptArgs {
    script: PathBuf,
    #[arg(long)]
    station: String,
}

#[derive(Debug, Args)]
struct ChatLocalPeerScriptArgs {
    script: PathBuf,
    #[arg(long = "station-a")]
    station_a: String,
    #[arg(long = "station-b")]
    station_b: String,
    #[arg(long = "out-dir")]
    out_dir: PathBuf,
    #[arg(long)]
    overwrite: bool,
    #[arg(long, default_value_t = 1.0)]
    gain: f32,
    #[arg(long = "snr-db")]
    snr_db: Option<f32>,
    #[arg(long = "drift-ppm")]
    sample_rate_drift_ppm: Option<f32>,
}

#[derive(Debug, Args)]
struct ChatLocalNodeScriptArgs {
    script: PathBuf,
    #[arg(long)]
    station: String,
    #[arg(long)]
    peer: String,
    #[arg(long, conflicts_with = "connect", value_name = "HOST:PORT")]
    listen: Option<String>,
    #[arg(long, conflicts_with = "listen", value_name = "HOST:PORT")]
    connect: Option<String>,
    #[arg(long = "ready-file")]
    ready_file: Option<PathBuf>,
    #[arg(long = "out-dir")]
    out_dir: PathBuf,
    #[arg(long)]
    overwrite: bool,
    #[arg(long, default_value_t = 1.0)]
    gain: f32,
    #[arg(long = "snr-db")]
    snr_db: Option<f32>,
    #[arg(long = "drift-ppm")]
    sample_rate_drift_ppm: Option<f32>,
}

#[derive(Debug, Args)]
struct ChatParseLogArgs {
    log: PathBuf,
    #[arg(long)]
    station: String,
}

#[derive(Debug, Args)]
struct ChatTuiArgs {
    #[arg(
        long,
        help = "Station call sign. Overrides the local setting from `chattybara station config --station CALL`."
    )]
    station: Option<String>,
    #[arg(
        long,
        value_name = "BACKEND",
        help = "Preselect chat backend: fake, native-loopback, native-wav-loopback, or native-local-node. Omit it to choose in the TUI."
    )]
    backend: Option<String>,
    #[arg(long)]
    peer: Option<String>,
    #[arg(long, conflicts_with = "connect", value_name = "HOST:PORT")]
    listen: Option<String>,
    #[arg(long, conflicts_with = "listen", value_name = "HOST:PORT")]
    connect: Option<String>,
    #[arg(long = "ready-file")]
    ready_file: Option<PathBuf>,
    #[arg(long, default_value_t = 1.0)]
    gain: f32,
    #[arg(long = "snr-db")]
    snr_db: Option<f32>,
    #[arg(long = "drift-ppm")]
    sample_rate_drift_ppm: Option<f32>,
    #[arg(
        long = "setup-preview",
        help = "Print the resolved TUI setup defaults and exit without opening the terminal."
    )]
    setup_preview: bool,
    #[arg(hide = true, trailing_var_arg = true)]
    backend_tail: Vec<String>,
}

#[derive(Debug, Args)]
struct StationArgs {
    #[command(subcommand)]
    command: StationCommand,
}

#[derive(Debug, Subcommand)]
enum StationCommand {
    Config(StationConfigArgs),
    Modes(StationModesArgs),
    FakeEvents(StationFakeEventsArgs),
    Replay(StationReplayArgs),
    Guard(StationGuardArgs),
    External(StationExternalArgs),
}

#[derive(Debug, Args)]
struct StationConfigArgs {
    #[arg(long, help = "Save the local station call sign for future commands.")]
    station: Option<String>,
    #[arg(long, help = "Override the settings file path.")]
    path: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct StationModesArgs {}

#[derive(Debug, Args)]
struct StationFakeEventsArgs {
    #[arg(long, default_value = "orca-chat")]
    mode: String,
    #[arg(long, default_value = "JA1TST")]
    station: String,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long = "session-dir")]
    session_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct StationReplayArgs {
    events: PathBuf,
}

#[derive(Debug, Args)]
struct StationGuardArgs {
    #[arg(long, value_enum)]
    action: StationGuardAction,
    #[arg(long = "arm-tx")]
    arm_tx: bool,
    #[arg(long)]
    live: bool,
    #[arg(long = "enable-reporting")]
    enable_reporting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StationGuardAction {
    SendMessage,
    Reply,
    QueueMail,
    ReportSpot,
    LogQso,
    AbortTransmit,
}

#[derive(Debug, Args)]
struct StationExternalArgs {
    #[arg(long, value_enum)]
    adapter: StationExternalAdapter,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    enable_tx: bool,
    #[arg(long = "enable-reporting")]
    enable_reporting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StationExternalAdapter {
    Js8call,
    Wsjtx,
    Fldigi,
    Pskreporter,
}

#[derive(Debug, Args)]
struct WinlinkArgs {
    #[command(subcommand)]
    command: WinlinkCommand,
}

#[derive(Debug, Subcommand)]
enum WinlinkCommand {
    Account(WinlinkAccountArgs),
    Compose(WinlinkComposeArgs),
    Inbox(WinlinkMailboxArgs),
    Outbox(WinlinkMailboxArgs),
    Read(WinlinkReadArgs),
    Sync(WinlinkSyncArgs),
    Telnet(WinlinkTelnetArgs),
    Transport(WinlinkTransportArgs),
}

#[derive(Debug, Args)]
struct WinlinkAccountArgs {
    #[command(subcommand)]
    command: WinlinkAccountCommand,
}

#[derive(Debug, Subcommand)]
enum WinlinkAccountCommand {
    Setup(WinlinkAccountSetupArgs),
    Status(WinlinkMailboxArgs),
}

#[derive(Debug, Args)]
struct WinlinkAccountSetupArgs {
    #[arg(long)]
    station: Option<String>,
    #[arg(long)]
    store: Option<PathBuf>,
    #[arg(long = "password-source", value_enum, default_value = "none")]
    password_source: WinlinkCredentialSourceArg,
}

#[derive(Debug, Args)]
struct WinlinkComposeArgs {
    #[arg(long)]
    station: Option<String>,
    #[arg(long)]
    store: Option<PathBuf>,
    #[arg(long, required = true)]
    to: Vec<String>,
    #[arg(long)]
    subject: String,
    #[arg(long)]
    body: String,
    #[arg(long = "attach")]
    attachments: Vec<PathBuf>,
}

#[derive(Debug, Args)]
struct WinlinkMailboxArgs {
    #[arg(long)]
    station: Option<String>,
    #[arg(long)]
    store: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct WinlinkReadArgs {
    message_id: String,
    #[arg(long)]
    station: Option<String>,
    #[arg(long)]
    store: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct WinlinkSyncArgs {
    #[arg(long)]
    station: Option<String>,
    #[arg(long)]
    store: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "fake")]
    transport: WinlinkTransportArg,
    #[arg(long)]
    live: bool,
    #[arg(long = "allow-send")]
    allow_send: bool,
    #[arg(long, default_value = DEFAULT_CMS_HOST)]
    host: String,
    #[arg(long, default_value_t = DEFAULT_CMS_PORT)]
    port: u16,
    #[arg(long = "timeout-ms", default_value_t = DEFAULT_TELNET_TIMEOUT_MS)]
    timeout_ms: u64,
}

#[derive(Debug, Args)]
struct WinlinkTelnetArgs {
    #[arg(long)]
    station: Option<String>,
    #[arg(long, default_value = DEFAULT_CMS_HOST)]
    host: String,
    #[arg(long, default_value_t = DEFAULT_CMS_PORT)]
    port: u16,
    #[arg(long = "timeout-ms", default_value_t = DEFAULT_TELNET_TIMEOUT_MS)]
    timeout_ms: u64,
    #[arg(long)]
    live: bool,
    #[arg(long)]
    check: bool,
}

#[derive(Debug, Args)]
struct WinlinkTransportArgs {
    #[arg(long)]
    station: Option<String>,
    #[arg(long, value_enum)]
    transport: WinlinkTransportArg,
    #[arg(long)]
    live: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum WinlinkTransportArg {
    Fake,
    #[value(alias = "telnet-cms", alias = "cms")]
    Telnet,
    #[value(alias = "vara-hf", alias = "vara-fm")]
    Vara,
    Orca,
}

impl From<WinlinkTransportArg> for WinlinkTransportKind {
    fn from(value: WinlinkTransportArg) -> Self {
        match value {
            WinlinkTransportArg::Fake => Self::Fake,
            WinlinkTransportArg::Telnet => Self::Telnet,
            WinlinkTransportArg::Vara => Self::Vara,
            WinlinkTransportArg::Orca => Self::Orca,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum WinlinkCredentialSourceArg {
    None,
    Env,
    Keychain,
}

impl From<WinlinkCredentialSourceArg> for CredentialSource {
    fn from(value: WinlinkCredentialSourceArg) -> Self {
        match value {
            WinlinkCredentialSourceArg::None => Self::None,
            WinlinkCredentialSourceArg::Env => Self::Env,
            WinlinkCredentialSourceArg::Keychain => Self::Keychain,
        }
    }
}

#[derive(Debug, Args)]
struct RigArgs {
    #[command(subcommand)]
    command: RigCommand,
}

#[derive(Debug, Subcommand)]
enum RigCommand {
    Hamlib(HamlibArgs),
    Ic705(Ic705Args),
    Profile(RadioProfileArgs),
    Validate(RadioValidateArgs),
}

#[derive(Debug, Args)]
struct HamlibArgs {
    #[command(subcommand)]
    command: HamlibCommand,
}

#[derive(Debug, Subcommand)]
enum HamlibCommand {
    Status(HamlibCommonArgs),
    GetFrequency(HamlibCommonArgs),
    GetMode(HamlibCommonArgs),
    PttRx(HamlibCommonArgs),
    PttTx(HamlibPttTxArgs),
}

#[derive(Debug, Args, Clone)]
struct HamlibCommonArgs {
    #[arg(long, default_value = DEFAULT_RIGCTLD_HOST)]
    host: String,
    #[arg(long = "timeout-ms", default_value_t = 1000)]
    timeout_ms: u64,
}

#[derive(Debug, Args)]
struct HamlibPttTxArgs {
    #[command(flatten)]
    common: HamlibCommonArgs,
    #[arg(long = "allow-transmit")]
    allow_transmit: bool,
}

#[derive(Debug, Args)]
struct RadioProfileArgs {
    #[arg(long, default_value = "generic-hamlib-radio")]
    model: String,
    #[arg(long = "input-device")]
    input_device: Option<String>,
    #[arg(long = "output-device")]
    output_device: Option<String>,
    #[arg(long = "sample-rate", default_value_t = 48000)]
    sample_rate: u32,
    #[arg(long, default_value_t = 1)]
    channels: u16,
    #[arg(long = "hamlib-host", default_value = DEFAULT_RIGCTLD_HOST)]
    hamlib_host: String,
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct RadioValidateArgs {
    profile: PathBuf,
}

#[derive(Debug, Args)]
struct Ic705Args {
    #[command(subcommand)]
    command: Ic705Command,
}

#[derive(Debug, Subcommand)]
enum Ic705Command {
    Profile(Ic705ProfileArgs),
    Validate(Ic705ValidateArgs),
    Civ(Ic705CivArgs),
    CivSerial(Ic705CivSerialArgs),
}

#[derive(Debug, Args)]
struct Ic705ProfileArgs {
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct Ic705ValidateArgs {
    profile: PathBuf,
}

#[derive(Debug, Args)]
struct Ic705CivArgs {
    #[arg(long, value_enum)]
    operation: Ic705CivCliOperation,
    #[arg(long = "radio-address", default_value = "A4")]
    radio_address: String,
    #[arg(long = "controller-address", default_value = "E0")]
    controller_address: String,
}

#[derive(Debug, Args)]
struct Ic705CivSerialArgs {
    #[arg(long, value_enum)]
    operation: Ic705CivCliOperation,
    #[arg(long = "radio-address", default_value = "A4")]
    radio_address: String,
    #[arg(long = "controller-address", default_value = "E0")]
    controller_address: String,
    #[arg(long)]
    port: Option<String>,
    #[arg(long = "baud-rate", default_value_t = 19200)]
    baud_rate: u32,
    #[arg(long = "timeout-ms", default_value_t = 500)]
    timeout_ms: u64,
    #[arg(long)]
    live: bool,
    #[arg(long = "allow-transmit")]
    allow_transmit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Ic705CivCliOperation {
    ReadFrequency,
    ReadMode,
    PttRx,
    PttTx,
}

#[derive(Debug, Args)]
struct DspArgs {
    #[command(subcommand)]
    command: DspCommand,
}

#[derive(Debug, Subcommand)]
enum DspCommand {
    Bandpass(DspBandpassArgs),
    Tone(DspToneArgs),
    Track(DspTrackArgs),
    Soft(DspSoftArgs),
}

#[derive(Debug, Args)]
struct DspBandpassArgs {
    #[arg(long)]
    low: f32,
    #[arg(long)]
    high: f32,
    input: PathBuf,
    output: PathBuf,
}

#[derive(Debug, Args)]
struct DspToneArgs {
    wav: PathBuf,
}

#[derive(Debug, Args)]
struct DspTrackArgs {
    wav: PathBuf,
}

#[derive(Debug, Args)]
struct DspSoftArgs {
    #[arg(long)]
    mark: f32,
    #[arg(long)]
    space: f32,
    #[arg(long = "samples-per-symbol")]
    samples_per_symbol: usize,
    wav: PathBuf,
}

#[derive(Debug, Args)]
struct AudioArgs {
    #[command(subcommand)]
    command: AudioCommand,
}

#[derive(Debug, Subcommand)]
enum AudioCommand {
    Chunks(AudioChunksArgs),
    Devices(AudioDevicesArgs),
    Loopback(AudioLoopbackArgs),
}

#[derive(Debug, Args)]
struct AudioDevicesArgs {
    #[arg(long = "include-supported")]
    include_supported: bool,
    #[arg(long = "sample-rate", default_value_t = 8000)]
    sample_rate: u32,
    #[arg(long, default_value_t = 1)]
    channels: u16,
}

#[derive(Debug, Args)]
struct AudioChunksArgs {
    #[arg(long, default_value_t = 256)]
    frames: usize,
    wav: PathBuf,
}

#[derive(Debug, Args)]
struct AudioLoopbackArgs {
    #[arg(long = "latency-frames", default_value_t = 0)]
    latency_frames: usize,
    #[arg(long, default_value_t = 1.0)]
    gain: f32,
    input: PathBuf,
    output: PathBuf,
}

#[derive(Debug, Args)]
struct CorpusArgs {
    #[command(subcommand)]
    command: CorpusCommand,
}

#[derive(Debug, Subcommand)]
enum CorpusCommand {
    Audit(AuditArgs),
    Validate(ValidateArgs),
    Verify(VerifyArgs),
    Inspect(CorpusInspectArgs),
    Decode(CorpusDecodeArgs),
    Observation(ObservationArgs),
}

#[derive(Debug, Args)]
struct ValidateArgs {
    manifest: PathBuf,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    manifest: PathBuf,
}

#[derive(Debug, Args)]
struct AuditArgs {
    #[arg(default_value = ".")]
    root: PathBuf,
}

#[derive(Debug, Args)]
struct CorpusInspectArgs {
    manifest: PathBuf,
}

#[derive(Debug, Args)]
struct CorpusDecodeArgs {
    manifest: PathBuf,
    #[arg(long, default_value = "out/traces")]
    out_dir: PathBuf,
}

#[derive(Debug, Args)]
struct ObservationArgs {
    #[command(subcommand)]
    command: ObservationCommand,
}

#[derive(Debug, Subcommand)]
enum ObservationCommand {
    Validate(ObservationValidateArgs),
}

#[derive(Debug, Args)]
struct ObservationValidateArgs {
    manifest: PathBuf,
}

#[derive(Debug, Args)]
struct DecodeArgs {
    wav: PathBuf,
    #[arg(long)]
    trace: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct FramesArgs {
    #[command(subcommand)]
    command: FramesCommand,
}

#[derive(Debug, Subcommand)]
enum FramesCommand {
    Classify(ClassifyArgs),
    Pipeline(PipelineArgs),
}

#[derive(Debug, Args)]
struct ClassifyArgs {
    wav: PathBuf,
}

#[derive(Debug, Args)]
struct PipelineArgs {
    wav: PathBuf,
}

#[derive(Debug, Args)]
struct FixtureArgs {
    #[command(subcommand)]
    command: FixtureCommand,
}

#[derive(Debug, Subcommand)]
enum FixtureCommand {
    Suite(SuiteArgs),
    Synth(SynthArgs),
}

#[derive(Debug, Args)]
struct SuiteArgs {
    out_dir: PathBuf,
    #[arg(long, default_value_t = 8000)]
    sample_rate: u32,
    #[arg(long)]
    overwrite: bool,
}

#[derive(Debug, Args)]
struct SynthArgs {
    output: PathBuf,
    #[arg(long, value_enum, default_value_t = SynthKind::ToneBurst)]
    kind: SynthKind,
    #[arg(long, default_value_t = 8000)]
    sample_rate: u32,
    #[arg(long, default_value_t = 1.0)]
    duration: f32,
    #[arg(long, default_value_t = 0.5)]
    amplitude: f32,
    #[arg(long, default_value_t = 1000.0)]
    frequency: f32,
    #[arg(long, default_value_t = 0.25)]
    start: f32,
    #[arg(long = "burst-duration", default_value_t = 0.375)]
    burst_duration: f32,
    #[arg(long = "start-frequency", default_value_t = 300.0)]
    start_frequency: f32,
    #[arg(long = "end-frequency", default_value_t = 1800.0)]
    end_frequency: f32,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SynthKind {
    Silence,
    ToneBurst,
    Sweep,
}

#[derive(Debug, Args)]
struct InspectArgs {
    wav: PathBuf,
}

#[derive(Debug, Args)]
struct SimulateArgs {
    #[command(subcommand)]
    command: SimulateCommand,
}

#[derive(Debug, Subcommand)]
enum SimulateCommand {
    AppLink(AppLinkArgs),
    Channel(ChannelArgs),
}

#[derive(Debug, Args)]
struct AppLinkArgs {
    #[arg(long, default_value = "JA1TST")]
    station: String,
    #[arg(long, default_value = "JA1QSO")]
    peer: String,
    #[arg(long = "payload-bytes", default_value_t = 180)]
    payload_bytes: usize,
    #[arg(long = "max-retries", default_value_t = 2)]
    max_retries: usize,
    #[arg(long = "timeout-ticks", default_value_t = 3)]
    timeout_ticks: u64,
    #[arg(long)]
    drop_first_attempt: bool,
    #[arg(long)]
    drop_all_attempts: bool,
    #[arg(long)]
    duplicate_deliveries: bool,
}

#[derive(Debug, Args)]
struct ChannelArgs {
    #[arg(long, default_value_t = 1.0)]
    gain: f32,
    #[arg(long)]
    snr: Option<f32>,
    #[arg(long = "sample-rate-drift-ppm")]
    sample_rate_drift_ppm: Option<f32>,
    input: PathBuf,
    output: PathBuf,
}

#[derive(Debug, Args)]
struct HostArgs {
    #[command(subcommand)]
    command: HostCommand,
}

#[derive(Debug, Subcommand)]
enum HostCommand {
    Eval(HostEvalArgs),
    Script(HostScriptArgs),
}

#[derive(Debug, Args)]
struct HostEvalArgs {
    line: Vec<String>,
}

#[derive(Debug, Args)]
struct HostScriptArgs {
    path: PathBuf,
}

#[derive(Debug, Args)]
struct ModemArgs {
    #[command(subcommand)]
    command: ModemCommand,
}

#[derive(Debug, Subcommand)]
enum ModemCommand {
    LiveAudio(ModemLiveAudioArgs),
    Decode(ModemDecodeArgs),
    Encode(ModemEncodeArgs),
    Roundtrip(ModemRoundtripArgs),
    Samples(ModemSamplesArgs),
    Sweep(ModemSweepArgs),
}

#[derive(Debug, Args)]
struct ModemOptions {
    #[arg(long, default_value_t = 8000)]
    sample_rate: u32,
    #[arg(long = "symbol-rate", default_value_t = 100.0)]
    symbol_rate: f32,
    #[arg(long, default_value_t = 1200.0)]
    mark: f32,
    #[arg(long, default_value_t = 1800.0)]
    space: f32,
    #[arg(long, default_value_t = 0.55)]
    amplitude: f32,
}

#[derive(Debug, Args)]
struct ModemEncodeArgs {
    payload: String,
    output: PathBuf,
    #[command(flatten)]
    options: ModemOptions,
}

#[derive(Debug, Args)]
struct ModemDecodeArgs {
    input: PathBuf,
    #[command(flatten)]
    options: ModemOptions,
}

#[derive(Debug, Args)]
struct ModemRoundtripArgs {
    payload: String,
    #[arg(long)]
    out: Option<PathBuf>,
    #[command(flatten)]
    options: ModemOptions,
}

#[derive(Debug, Args)]
struct ModemSamplesArgs {
    payload: String,
    out_dir: PathBuf,
    #[arg(long = "rx-latency-frames", default_value_t = 80)]
    rx_latency_frames: usize,
    #[arg(long, default_value_t = 0.8)]
    gain: f32,
    #[arg(long = "snr-db", default_value_t = 30.0)]
    snr_db: f32,
    #[arg(long = "drift-ppm", default_value_t = 100.0)]
    drift_ppm: f32,
    #[arg(long)]
    overwrite: bool,
    #[command(flatten)]
    options: ModemOptions,
}

#[derive(Debug, Args)]
struct ModemSweepArgs {
    payload: String,
    out_dir: PathBuf,
    #[arg(long, default_value_t = 0.8)]
    gain: f32,
    #[arg(long = "snr-db")]
    snr_db: Vec<f32>,
    #[arg(long = "drift-ppm")]
    drift_ppm: Vec<f32>,
    #[arg(long)]
    overwrite: bool,
    #[command(flatten)]
    options: ModemOptions,
}

#[derive(Debug, Args)]
struct ModemLiveAudioArgs {
    payload: String,
    #[arg(long = "input-device")]
    input_device: Option<String>,
    #[arg(long = "output-device")]
    output_device: Option<String>,
    #[arg(long = "tx-gain", default_value_t = 0.20)]
    tx_gain: f32,
    #[arg(long = "rx-seconds", default_value_t = 5.0)]
    rx_seconds: f32,
    #[arg(long, default_value_t = 1)]
    channels: u16,
    #[arg(long)]
    live: bool,
    #[arg(long = "allow-transmit-audio")]
    allow_transmit_audio: bool,
    #[arg(long = "key-ptt")]
    key_ptt: bool,
    #[arg(long = "hamlib-host")]
    hamlib_host: Option<String>,
    #[arg(long = "hamlib-timeout-ms", default_value_t = 1000)]
    hamlib_timeout_ms: u64,
    #[command(flatten)]
    options: ModemOptions,
}

#[derive(Debug, Args)]
struct LabArgs {
    #[command(subcommand)]
    command: LabCommand,
}

#[derive(Debug, Subcommand)]
enum LabCommand {
    Compare(LabCompareArgs),
    Run(LabRunArgs),
    Snapshot(LabSnapshotArgs),
}

#[derive(Debug, Args)]
struct LabRunArgs {
    out_dir: PathBuf,
    #[arg(long, default_value_t = 8000)]
    sample_rate: u32,
    #[arg(long)]
    overwrite: bool,
}

#[derive(Debug, Args)]
struct LabSnapshotArgs {
    report: PathBuf,
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct LabCompareArgs {
    expected: PathBuf,
    actual: PathBuf,
    #[arg(long = "tone-tolerance-hz", default_value_t = 1.0)]
    tone_tolerance_hz: f64,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chattybara=info".into()),
        )
        .without_time()
        .init();

    match Cli::parse().command {
        Command::Audio(args) => run_audio(args),
        Command::Chat(args) => run_chat(args),
        Command::Corpus(args) => run_corpus(args),
        Command::Decode(args) => run_decode(args),
        Command::Dsp(args) => run_dsp(args),
        Command::Frames(args) => run_frames(args),
        Command::Fixture(args) => run_fixture(args),
        Command::Host(args) => run_host(args),
        Command::Inspect(args) => run_inspect(args),
        Command::Lab(args) => run_lab(args),
        Command::Modem(args) => run_modem(args),
        Command::Rig(args) => run_rig(args),
        Command::Station(args) => run_station(args),
        Command::Simulate(args) => run_simulate(args),
        Command::Winlink(args) => run_winlink(args),
    }
}

fn run_station(args: StationArgs) -> Result<()> {
    match args.command {
        StationCommand::Config(args) => {
            let path = args.path.unwrap_or_else(default_local_settings_path);
            let mut settings = load_local_settings_from(&path)?;
            let updated = if let Some(station) = args.station {
                settings.station = Some(normalize_call(&station)?);
                save_local_settings_to(&path, &settings)?;
                true
            } else {
                false
            };
            let report = json!({
                "kind": "station-local-settings-report",
                "ok": true,
                "updated": updated,
                "path": path,
                "settings": settings,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        StationCommand::Modes(_args) => {
            let modes = built_in_modes();
            let report = json!({
                "kind": "station-mode-registry",
                "ok": true,
                "mode_count": modes.len(),
                "modes": modes,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        StationCommand::FakeEvents(args) => {
            let mode = mode_by_label(&args.mode)?;
            let records = fake_events_for_mode(mode, &args.station)?;
            let summary = replay_summary(&records);
            if let Some(path) = &args.out {
                write_event_log(path, &records)?;
            }
            if let Some(dir) = &args.session_dir {
                fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
                write_event_log(&dir.join("events.jsonl"), &records)?;
                write_json_file(
                    &dir.join("support.json"),
                    &json!({
                        "kind": "station-session-support",
                        "product": "chattybara",
                        "mode": mode.label(),
                        "station": args.station,
                        "receive_only": built_in_modes()
                            .into_iter()
                            .find(|descriptor| descriptor.id == mode)
                            .map(|descriptor| descriptor.capabilities.rx_only)
                            .unwrap_or(false),
                    }),
                )?;
            }
            let report = json!({
                "kind": "station-fake-events-report",
                "ok": true,
                "mode": mode.label(),
                "out": args.out,
                "session_dir": args.session_dir,
                "summary": summary,
                "records": records,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        StationCommand::Replay(args) => {
            let records = read_event_log(&args.events)?;
            let report = json!({
                "kind": "station-replay-report",
                "ok": true,
                "events": args.events,
                "summary": replay_summary(&records),
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        StationCommand::Guard(args) => {
            let safety = StationSafetyState {
                live: args.live,
                tx_armed: args.arm_tx,
                ptt_keyed: false,
                reporting_enabled: args.enable_reporting,
            };
            let action = station_guard_action(args.action);
            let report = action_guard_report(action, safety);
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("station action guard rejected action");
            }
            Ok(())
        }
        StationCommand::External(args) => {
            let (mode, default_host, default_port) = match args.adapter {
                StationExternalAdapter::Js8call => (ModeId::Js8callExternal, "127.0.0.1", 2242),
                StationExternalAdapter::Wsjtx => (ModeId::WsjtxExternal, "127.0.0.1", 2237),
                StationExternalAdapter::Fldigi => (ModeId::FldigiExternal, "127.0.0.1", 7362),
                StationExternalAdapter::Pskreporter => {
                    (ModeId::PskReporter, "retrieve.pskreporter.info", 443)
                }
            };
            let tx_enabled = args.enable_tx;
            let reporting_enabled = args.enable_reporting;
            let report = json!({
                "kind": "station-external-adapter-scaffold",
                "ok": true,
                "mode": mode.label(),
                "endpoint": {
                    "host": args.host.unwrap_or_else(|| default_host.to_owned()),
                    "port": args.port.unwrap_or(default_port),
                },
                "receive_only": !tx_enabled && !reporting_enabled,
                "tx_enabled": tx_enabled,
                "reporting_enabled": reporting_enabled,
                "safety": {
                    "default": "DRY RUN",
                    "requires_explicit_arming": true,
                    "opens_network_connection": false,
                    "note": "scaffold only; live external app connections are implemented behind later opt-in adapters"
                }
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

fn run_chat(args: ChatArgs) -> Result<()> {
    match args.command {
        ChatCommand::AppScript(args) => {
            let script = fs::read_to_string(&args.script)
                .with_context(|| format!("reading {}", args.script.display()))?;
            let report = run_app_script(&args.station, &script)
                .with_context(|| format!("running chat app script {}", args.script.display()))?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("chat app script failed");
            }
            Ok(())
        }
        ChatCommand::ComparePeerLogs(args) => {
            let station_a_log = fs::read_to_string(&args.station_a_log)
                .with_context(|| format!("reading {}", args.station_a_log.display()))?;
            let station_b_log = fs::read_to_string(&args.station_b_log)
                .with_context(|| format!("reading {}", args.station_b_log.display()))?;
            let report = compare_peer_logs(
                &args.station_a,
                &station_a_log,
                &args.station_b,
                &station_b_log,
            )
            .with_context(|| {
                format!(
                    "comparing peer chat logs {} and {}",
                    args.station_a_log.display(),
                    args.station_b_log.display()
                )
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("chat peer log comparison failed");
            }
            Ok(())
        }
        ChatCommand::CompareSessionLogs(args) => {
            let (station_a_log_path, station_b_log_path) =
                resolve_session_log_paths(&args.station_a_path, args.station_b_path.as_deref())?;
            let station_a_log = fs::read_to_string(&station_a_log_path)
                .with_context(|| format!("reading {}", station_a_log_path.display()))?;
            let station_b_log = fs::read_to_string(&station_b_log_path)
                .with_context(|| format!("reading {}", station_b_log_path.display()))?;
            let report = compare_peer_logs(
                &args.station_a,
                &station_a_log,
                &args.station_b,
                &station_b_log,
            )
            .with_context(|| {
                format!(
                    "comparing session chat logs {} and {}",
                    station_a_log_path.display(),
                    station_b_log_path.display()
                )
            })?;
            let output = json!({
                "kind": "chat-session-log-comparison-report",
                "ok": report.ok,
                "station_a_log": station_a_log_path,
                "station_b_log": station_b_log_path,
                "peer_log_comparison": report,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            if !output["ok"].as_bool().unwrap_or(false) {
                bail!("chat session log comparison failed");
            }
            Ok(())
        }
        ChatCommand::CompareScriptLog(args) => {
            let script = fs::read_to_string(&args.script)
                .with_context(|| format!("reading {}", args.script.display()))?;
            let log = fs::read_to_string(&args.log)
                .with_context(|| format!("reading {}", args.log.display()))?;
            let report = compare_fake_script_to_simple_log(&args.station, &script, &log)
                .with_context(|| {
                    format!(
                        "comparing chat script {} to log {}",
                        args.script.display(),
                        args.log.display()
                    )
                })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("chat script/log comparison failed");
            }
            Ok(())
        }
        ChatCommand::FakeScript(args) => {
            let script = fs::read_to_string(&args.script)
                .with_context(|| format!("reading {}", args.script.display()))?;
            let report = run_fake_script(&args.station, &script)
                .with_context(|| format!("running chat script {}", args.script.display()))?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("chat fake script failed");
            }
            Ok(())
        }
        ChatCommand::LocalNodeScript(args) => {
            let script = fs::read_to_string(&args.script)
                .with_context(|| format!("reading {}", args.script.display()))?;
            let mode = match (args.listen, args.connect) {
                (Some(bind), None) => LocalNodeMode::Listen {
                    bind,
                    ready_file: args.ready_file,
                },
                (None, Some(host)) => {
                    if args.ready_file.is_some() {
                        bail!("--ready-file can only be used with --listen");
                    }
                    LocalNodeMode::Connect { host }
                }
                (None, None) => bail!("local node script requires --listen or --connect"),
                (Some(_), Some(_)) => bail!("use either --listen or --connect, not both"),
            };
            let report = run_local_node_script(
                LocalNodeScriptConfig {
                    station: args.station,
                    peer: args.peer,
                    out_dir: args.out_dir,
                    overwrite: args.overwrite,
                    mode,
                    channel: ChannelConfig {
                        gain: args.gain,
                        snr_db: args.snr_db,
                        sample_rate_drift_ppm: args.sample_rate_drift_ppm,
                    },
                },
                &script,
            )
            .with_context(|| format!("running local node script {}", args.script.display()))?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("local node chat script failed");
            }
            Ok(())
        }
        ChatCommand::LocalPeerScript(args) => {
            let script = fs::read_to_string(&args.script)
                .with_context(|| format!("reading {}", args.script.display()))?;
            let report = run_local_peer_script(
                LocalPeerScriptConfig {
                    station_a: args.station_a,
                    station_b: args.station_b,
                    out_dir: args.out_dir,
                    overwrite: args.overwrite,
                    channel: ChannelConfig {
                        gain: args.gain,
                        snr_db: args.snr_db,
                        sample_rate_drift_ppm: args.sample_rate_drift_ppm,
                    },
                },
                &script,
            )
            .with_context(|| format!("running local peer script {}", args.script.display()))?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("local peer chat script failed");
            }
            Ok(())
        }
        ChatCommand::ParseLog(args) => {
            let log = fs::read_to_string(&args.log)
                .with_context(|| format!("reading {}", args.log.display()))?;
            let report = parse_simple_log(&args.station, &log)
                .with_context(|| format!("parsing chat log {}", args.log.display()))?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("chat log parse failed");
            }
            Ok(())
        }
        ChatCommand::Tui(args) => {
            let station = resolve_station(args.station.as_deref())?;
            let channel = ChannelConfig {
                gain: args.gain,
                snr_db: args.snr_db,
                sample_rate_drift_ppm: args.sample_rate_drift_ppm,
            };
            let setup_mode = build_optional_tui_local_node_mode(
                args.listen.clone(),
                args.connect.clone(),
                args.ready_file.clone(),
            )?;
            let backend = resolve_chat_tui_backend(
                args.backend.as_deref(),
                &args.backend_tail,
                args.peer.is_some() || setup_mode.is_some(),
            )?;
            let setup = if args.backend.is_none() {
                Some(ChatTuiSetupConfig {
                    backend,
                    peer_call: args.peer.clone(),
                    mode: setup_mode.clone(),
                    channel,
                })
            } else {
                None
            };
            if args.setup_preview {
                let preview = chat_tui_setup_preview(&station, backend, setup.as_ref());
                println!("{}", serde_json::to_string_pretty(&preview)?);
                return Ok(());
            }
            let local_node = if setup.is_some() {
                None
            } else {
                build_tui_local_node_config(
                    backend,
                    args.peer,
                    args.listen,
                    args.connect,
                    args.ready_file,
                    channel,
                )?
            };
            run_chat_tui(ChatTuiConfig {
                station_call: station,
                backend,
                local_node,
                setup,
            })
        }
    }
}

fn run_winlink(args: WinlinkArgs) -> Result<()> {
    match args.command {
        WinlinkCommand::Account(args) => match args.command {
            WinlinkAccountCommand::Setup(args) => {
                let station = resolve_station(args.station.as_deref())?;
                let store_path = resolve_winlink_store_path(args.store, &station)?;
                let mut store = WinlinkStore::load_or_new(&store_path, &station)
                    .with_context(|| format!("loading {}", store_path.display()))?;
                let account = WinlinkAccount::new(&station, args.password_source.into())?;
                store.set_account(account.clone());
                store
                    .save(&store_path)
                    .with_context(|| format!("saving {}", store_path.display()))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "kind": "winlink-account-setup-report",
                        "ok": true,
                        "station": account.station,
                        "address": account.address,
                        "password_source": account.password_source.label(),
                        "store": store_path,
                    }))?
                );
                Ok(())
            }
            WinlinkAccountCommand::Status(args) => {
                let station = resolve_station(args.station.as_deref())?;
                let store_path = resolve_winlink_store_path(args.store, &station)?;
                let store = WinlinkStore::load_or_new(&store_path, &station)
                    .with_context(|| format!("loading {}", store_path.display()))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "kind": "winlink-account-status-report",
                        "ok": true,
                        "station": store.station,
                        "store": store_path,
                        "account": store.account,
                        "message_count": store.messages.len(),
                        "inbox_count": store.messages_in(MailFolder::Inbox).len(),
                        "outbox_count": store.messages_in(MailFolder::Outbox).len(),
                        "sent_count": store.messages_in(MailFolder::Sent).len(),
                    }))?
                );
                Ok(())
            }
        },
        WinlinkCommand::Compose(args) => {
            let station = resolve_station(args.station.as_deref())?;
            let store_path = resolve_winlink_store_path(args.store, &station)?;
            let mut store = WinlinkStore::load_or_new(&store_path, &station)
                .with_context(|| format!("loading {}", store_path.display()))?;
            let attachments = args
                .attachments
                .iter()
                .map(WinlinkAttachment::from_path)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let id = store.queue_message(args.to, args.subject, args.body, attachments)?;
            let proposal =
                B2fProposal::from_message(store.find_message(&id).expect("queued message"));
            store
                .save(&store_path)
                .with_context(|| format!("saving {}", store_path.display()))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "kind": "winlink-compose-report",
                    "ok": true,
                    "station": store.station,
                    "store": store_path,
                    "message_id": id,
                    "folder": MailFolder::Outbox.label(),
                    "b2f_proposal": proposal,
                }))?
            );
            Ok(())
        }
        WinlinkCommand::Inbox(args) => run_winlink_mailbox(args, MailFolder::Inbox),
        WinlinkCommand::Outbox(args) => run_winlink_mailbox(args, MailFolder::Outbox),
        WinlinkCommand::Read(args) => {
            let station = resolve_station(args.station.as_deref())?;
            let store_path = resolve_winlink_store_path(args.store, &station)?;
            let store = WinlinkStore::load_or_new(&store_path, &station)
                .with_context(|| format!("loading {}", store_path.display()))?;
            let message = store.find_message(&args.message_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "kind": "winlink-message-report",
                    "ok": true,
                    "station": store.station,
                    "store": store_path,
                    "message": message,
                    "b2f_proposal": B2fProposal::from_message(message),
                }))?
            );
            Ok(())
        }
        WinlinkCommand::Sync(args) => {
            let station = resolve_station(args.station.as_deref())?;
            let store_path = resolve_winlink_store_path(args.store, &station)?;
            let mut store = WinlinkStore::load_or_new(&store_path, &station)
                .with_context(|| format!("loading {}", store_path.display()))?;
            let transport = WinlinkTransportKind::from(args.transport);
            let report = if transport == WinlinkTransportKind::Fake {
                let report = fake_sync(&mut store, Some(store_path.clone()))?;
                store
                    .save(&store_path)
                    .with_context(|| format!("saving {}", store_path.display()))?;
                report
            } else if transport == WinlinkTransportKind::Telnet && args.live {
                let password = winlink_password_from_env();
                let report = telnet_cms_receive_sync(
                    &mut store,
                    Some(store_path.clone()),
                    TelnetCmsConfig {
                        station: station.clone(),
                        host: args.host,
                        port: args.port,
                        timeout_ms: args.timeout_ms,
                        live: true,
                    },
                    password.as_deref(),
                    args.allow_send,
                )?;
                store
                    .save(&store_path)
                    .with_context(|| format!("saving {}", store_path.display()))?;
                report
            } else {
                guarded_dry_run_sync_report(
                    &store,
                    Some(store_path.clone()),
                    transport,
                    args.live,
                    args.allow_send,
                )?
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        WinlinkCommand::Telnet(args) => {
            let station = resolve_station(args.station.as_deref())?;
            let report = telnet_cms_check(TelnetCmsConfig {
                station,
                host: args.host,
                port: args.port,
                timeout_ms: args.timeout_ms,
                live: args.live,
            })?;
            let output = json!({
                "kind": "winlink-telnet-check-report",
                "ok": report.ok,
                "check_requested": args.check,
                "transport_status": report,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            Ok(())
        }
        WinlinkCommand::Transport(args) => {
            let station = resolve_station(args.station.as_deref())?;
            let report = transport_plan_report(
                station,
                WinlinkTransportKind::from(args.transport),
                args.live,
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

fn run_winlink_mailbox(args: WinlinkMailboxArgs, folder: MailFolder) -> Result<()> {
    let station = resolve_station(args.station.as_deref())?;
    let store_path = resolve_winlink_store_path(args.store, &station)?;
    let store = WinlinkStore::load_or_new(&store_path, &station)
        .with_context(|| format!("loading {}", store_path.display()))?;
    let messages = store
        .messages_in(folder)
        .into_iter()
        .map(|message| {
            json!({
                "id": message.id,
                "from": message.from,
                "to": message.to,
                "subject": message.subject,
                "state": message.state,
                "attachment_count": message.attachments.len(),
                "transport": message.transport,
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "kind": "winlink-mailbox-report",
            "ok": true,
            "station": store.station,
            "store": store_path,
            "folder": folder.label(),
            "message_count": messages.len(),
            "messages": messages,
        }))?
    );
    Ok(())
}

fn resolve_winlink_store_path(path: Option<PathBuf>, station: &str) -> Result<PathBuf> {
    path.map(Ok)
        .unwrap_or_else(|| Ok(default_store_path(station)?))
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct LocalSettings {
    station: Option<String>,
}

fn resolve_station(explicit: Option<&str>) -> Result<String> {
    if let Some(station) = explicit.filter(|value| !value.trim().is_empty()) {
        return Ok(normalize_call(station)?);
    }
    if let Some(station) = load_local_settings()?.station {
        return Ok(normalize_call(&station)?);
    }
    Ok(DEFAULT_SAMPLE_STATION.to_owned())
}

fn load_local_settings() -> Result<LocalSettings> {
    load_local_settings_from(&default_local_settings_path())
}

fn load_local_settings_from(path: &Path) -> Result<LocalSettings> {
    if !path.exists() {
        return Ok(LocalSettings::default());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn save_local_settings_to(path: &Path, settings: &LocalSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, toml::to_string_pretty(settings)?)
        .with_context(|| format!("writing {}", path.display()))
}

fn default_local_settings_path() -> PathBuf {
    std::env::var_os(LOCAL_SETTINGS_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .map(|base| base.join("chattybara").join("settings.toml"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                PathBuf::from(home)
                    .join(".config")
                    .join("chattybara")
                    .join("settings.toml")
            })
        })
        .unwrap_or_else(|| PathBuf::from(".chattybara-settings.toml"))
}

fn run_rig(args: RigArgs) -> Result<()> {
    match args.command {
        RigCommand::Hamlib(args) => match args.command {
            HamlibCommand::Status(args) => {
                let config = hamlib_config(args);
                let report = hamlib_status(&config);
                println!("{}", serde_json::to_string_pretty(&report)?);
                if !report.ok {
                    bail!("Hamlib status failed");
                }
                Ok(())
            }
            HamlibCommand::GetFrequency(args) => {
                let report = hamlib_get_frequency(&hamlib_config(args))?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            HamlibCommand::GetMode(args) => {
                let report = hamlib_get_mode(&hamlib_config(args))?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            HamlibCommand::PttRx(args) => {
                let report = hamlib_set_ptt(&hamlib_config(args), HamlibPttState::Rx)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            HamlibCommand::PttTx(args) => {
                if !args.allow_transmit {
                    bail!("Hamlib PTT TX requires --allow-transmit");
                }
                let report = hamlib_set_ptt(&hamlib_config(args.common), HamlibPttState::Tx)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
        },
        RigCommand::Ic705(args) => match args.command {
            Ic705Command::Profile(args) => {
                let profile = default_ic705_profile();
                let toml = ic705_profile_toml(&profile)?;
                if let Some(path) = args.out {
                    ensure_parent_dir(&path)?;
                    fs::write(&path, toml)
                        .with_context(|| format!("writing {}", path.display()))?;
                    let report = json!({
                        "kind": "ic705-profile-write-report",
                        "ok": true,
                        "path": path,
                        "profile": profile,
                    });
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{toml}");
                }
                Ok(())
            }
            Ic705Command::Validate(args) => {
                let profile = load_ic705_profile(&args.profile)?;
                let report = validate_ic705_profile(profile);
                println!("{}", serde_json::to_string_pretty(&report)?);
                if !report.ok {
                    bail!("IC-705 profile validation failed");
                }
                Ok(())
            }
            Ic705Command::Civ(args) => {
                let report = build_ic705_civ_frame_report(
                    args.operation.into(),
                    &args.radio_address,
                    &args.controller_address,
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
            Ic705Command::CivSerial(args) => {
                let report = run_ic705_civ_serial(Ic705CivSerialConfig {
                    operation: args.operation.into(),
                    radio_address: args.radio_address,
                    controller_address: args.controller_address,
                    port: args.port,
                    baud_rate: args.baud_rate,
                    timeout_ms: args.timeout_ms,
                    live: args.live,
                    allow_transmit: args.allow_transmit,
                })?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            }
        },
        RigCommand::Profile(args) => {
            let profile = default_radio_profile(RadioProfileTemplate {
                model: args.model,
                input_device: args.input_device,
                output_device: args.output_device,
                sample_rate: args.sample_rate,
                channels: args.channels,
                hamlib_host: args.hamlib_host,
            });
            let toml = radio_profile_toml(&profile)?;
            if let Some(path) = args.out {
                ensure_parent_dir(&path)?;
                fs::write(&path, toml).with_context(|| format!("writing {}", path.display()))?;
                let report = json!({
                    "kind": "radio-profile-write-report",
                    "ok": true,
                    "path": path,
                    "profile": profile,
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{toml}");
            }
            Ok(())
        }
        RigCommand::Validate(args) => {
            let profile = load_radio_profile(&args.profile)?;
            let report = validate_radio_profile(profile);
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("radio profile validation failed");
            }
            Ok(())
        }
    }
}

fn hamlib_config(args: HamlibCommonArgs) -> HamlibConfig {
    HamlibConfig {
        host: args.host,
        timeout_ms: args.timeout_ms,
    }
}

fn station_guard_action(action: StationGuardAction) -> StationAction {
    match action {
        StationGuardAction::SendMessage => StationAction::SendMessage {
            to: "JA1QSO".to_owned(),
            text: "guarded test message".to_owned(),
        },
        StationGuardAction::Reply => StationAction::Reply {
            target_id: "decode-001".to_owned(),
            text: "guarded reply".to_owned(),
        },
        StationGuardAction::QueueMail => StationAction::QueueMail {
            to: "JA1QSO".to_owned(),
            subject: "guarded mail".to_owned(),
            body: "guarded body".to_owned(),
        },
        StationGuardAction::ReportSpot => StationAction::ReportSpot {
            call_sign: "JA1QSO".to_owned(),
            frequency_hz: 14_074_000,
            mode: "FT8".to_owned(),
        },
        StationGuardAction::LogQso => StationAction::LogQso {
            call_sign: "JA1QSO".to_owned(),
            mode: "FT8".to_owned(),
        },
        StationGuardAction::AbortTransmit => StationAction::AbortTransmit,
    }
}

impl From<Ic705CivCliOperation> for Ic705CivOperation {
    fn from(operation: Ic705CivCliOperation) -> Self {
        match operation {
            Ic705CivCliOperation::ReadFrequency => Self::ReadFrequency,
            Ic705CivCliOperation::ReadMode => Self::ReadMode,
            Ic705CivCliOperation::PttRx => Self::PttRx,
            Ic705CivCliOperation::PttTx => Self::PttTx,
        }
    }
}

fn resolve_chat_tui_backend(
    raw: Option<&str>,
    tail: &[String],
    local_node_hint: bool,
) -> Result<ChatTuiBackend> {
    match raw {
        Some(raw) => parse_chat_tui_backend(raw, tail),
        None => {
            if !tail.is_empty() {
                bail!("unexpected extra chat TUI argument(s): {}", tail.join(" "));
            }
            if local_node_hint {
                Ok(ChatTuiBackend::NativeLocalNode)
            } else {
                Ok(ChatTuiBackend::NativeLoopback)
            }
        }
    }
}

fn parse_chat_tui_backend(raw: &str, tail: &[String]) -> Result<ChatTuiBackend> {
    if !tail.is_empty() {
        let joined = std::iter::once(raw)
            .chain(tail.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let compact = joined.replace(' ', "");
        if matches!(
            compact.as_str(),
            "native-loopback" | "native-wav-loopback" | "native-local-node" | "fake"
        ) {
            bail!("unknown chat TUI backend {joined:?}; remove the space and use {compact:?}");
        }
        bail!("unexpected extra chat TUI argument(s): {}", tail.join(" "));
    }

    ChatTuiBackend::from_label(raw).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown chat TUI backend {raw:?}; expected fake, native-loopback, native-wav-loopback, or native-local-node"
        )
    })
}

fn build_optional_tui_local_node_mode(
    listen: Option<String>,
    connect: Option<String>,
    ready_file: Option<PathBuf>,
) -> Result<Option<LocalNodeMode>> {
    match (listen, connect) {
        (Some(bind), None) => Ok(Some(LocalNodeMode::Listen { bind, ready_file })),
        (None, Some(host)) => {
            if ready_file.is_some() {
                bail!("--ready-file can only be used with --listen");
            }
            Ok(Some(LocalNodeMode::Connect { host }))
        }
        (None, None) => {
            if ready_file.is_some() {
                bail!("--ready-file requires --listen");
            }
            Ok(None)
        }
        (Some(_), Some(_)) => bail!("use either --listen or --connect, not both"),
    }
}

fn chat_tui_setup_preview(
    station: &str,
    backend: ChatTuiBackend,
    setup: Option<&ChatTuiSetupConfig>,
) -> Value {
    let station = normalize_preview_call(station);
    let peer = setup
        .and_then(|value| value.peer_call.as_deref())
        .map(normalize_preview_call);
    let selected_backend = setup.map(|value| value.backend).unwrap_or(backend);
    let local_node = setup.and_then(|value| {
        value.mode.as_ref().map(|mode| match mode {
            LocalNodeMode::Listen { bind, ready_file } => json!({
                "mode": "listen",
                "bind": bind,
                "ready_file": ready_file,
            }),
            LocalNodeMode::Connect { host } => json!({
                "mode": "connect",
                "host": host,
            }),
        })
    });
    json!({
        "kind": "chat-tui-setup-preview",
        "command": "chattybara chat tui",
        "product": "chattybara",
        "modem_engine": "orca",
        "station": station,
        "starts_in_setup": setup.is_some(),
        "running_backend": if setup.is_some() {
            ChatTuiBackend::NativeLoopback.label()
        } else {
            backend.label()
        },
        "selected_backend": selected_backend.label(),
        "peer": peer,
        "local_node": local_node,
        "channel": setup.map(|value| value.channel),
        "audio": {
            "input_device": "default input",
            "output_device": "default output",
            "sample_rate": 48000,
            "channels": 1,
        },
        "radio": {
            "control": "off",
            "hamlib_host": null,
        },
        "setup_commands": [
            "/station CALL",
            "/backend fake|native-loopback|native-wav-loopback|native-local-node",
            "/peer CALL",
            "/listen [HOST:PORT]",
            "/connect-node HOST:PORT",
            "/audio-input DEVICE",
            "/audio-output DEVICE",
            "/audio-rate HZ",
            "/radio-hamlib [HOST:PORT]",
            "/start",
        ],
    })
}

fn normalize_preview_call(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn build_tui_local_node_config(
    backend: ChatTuiBackend,
    peer: Option<String>,
    listen: Option<String>,
    connect: Option<String>,
    ready_file: Option<PathBuf>,
    channel: ChannelConfig,
) -> Result<Option<ChatTuiLocalNodeConfig>> {
    if backend != ChatTuiBackend::NativeLocalNode {
        if peer.is_some() || listen.is_some() || connect.is_some() || ready_file.is_some() {
            bail!(
                "--peer, --listen, --connect, and --ready-file require --backend native-local-node"
            );
        }
        return Ok(None);
    }

    let peer_call =
        peer.ok_or_else(|| anyhow::anyhow!("--backend native-local-node requires --peer"))?;
    let mode = match (listen, connect) {
        (Some(bind), None) => LocalNodeMode::Listen { bind, ready_file },
        (None, Some(host)) => {
            if ready_file.is_some() {
                bail!("--ready-file can only be used with --listen");
            }
            LocalNodeMode::Connect { host }
        }
        (None, None) => bail!("--backend native-local-node requires --listen or --connect"),
        (Some(_), Some(_)) => bail!("use either --listen or --connect, not both"),
    };
    Ok(Some(ChatTuiLocalNodeConfig {
        peer_call,
        mode,
        channel,
    }))
}

fn resolve_session_log_paths(
    station_a_path: &Path,
    station_b_path: Option<&Path>,
) -> Result<(PathBuf, PathBuf)> {
    if let Some(station_b_path) = station_b_path {
        return Ok((
            resolve_one_session_log_path(station_a_path)
                .with_context(|| format!("resolving {}", station_a_path.display()))?,
            resolve_one_session_log_path(station_b_path)
                .with_context(|| format!("resolving {}", station_b_path.display()))?,
        ));
    }

    let station_a_log = station_a_path.join("station-a").join("chat.log");
    let station_b_log = station_a_path.join("station-b").join("chat.log");
    if station_a_log.exists() && station_b_log.exists() {
        return Ok((station_a_log, station_b_log));
    }
    bail!(
        "{} is not a combined local-peer session directory; pass two session dirs or log files",
        station_a_path.display()
    )
}

fn resolve_one_session_log_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_owned());
    }
    let log_path = path.join("chat.log");
    if log_path.exists() {
        return Ok(log_path);
    }
    bail!(
        "{} is not a chat log file or session directory",
        path.display()
    )
}

fn run_audio(args: AudioArgs) -> Result<()> {
    match args.command {
        AudioCommand::Chunks(args) => {
            let buffer = AudioBuffer::from_wav(&args.wav)
                .with_context(|| format!("reading {}", args.wav.display()))?;
            let report = buffer
                .chunk_report(args.frames)
                .with_context(|| "building chunk report")?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        AudioCommand::Devices(args) => {
            let report = enumerate_audio_devices(AudioDeviceRequest {
                sample_rate: args.sample_rate,
                channels: args.channels,
                include_supported: args.include_supported,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        AudioCommand::Loopback(args) => {
            ensure_parent_dir(&args.output)?;
            let input = AudioBuffer::from_wav(&args.input)
                .with_context(|| format!("reading {}", args.input.display()))?;
            let output = virtual_loopback(
                &input,
                LoopbackConfig {
                    latency_frames: args.latency_frames,
                    gain: args.gain,
                },
            )
            .with_context(|| "running virtual loopback")?;
            output
                .write_wav(&args.output)
                .with_context(|| format!("writing {}", args.output.display()))?;
            let report = json!({
                "input_frames": input.frame_count(),
                "output_frames": output.frame_count(),
                "latency_frames": args.latency_frames,
                "gain": args.gain,
                "output": args.output,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

fn run_corpus(args: CorpusArgs) -> Result<()> {
    match args.command {
        CorpusCommand::Audit(args) => {
            let report = audit_corpus(&args.root)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report["ok"].as_bool().unwrap_or(false) {
                bail!("corpus audit failed");
            }
            Ok(())
        }
        CorpusCommand::Validate(args) => {
            let manifest = resolve_manifest_path(&args.manifest);
            let report = validate_manifest(&manifest)
                .with_context(|| format!("validating {}", manifest.display()))?;

            println!(
                "validated {} fixture(s) from {}",
                report.fixtures.len(),
                report.manifest_path.display()
            );
            for fixture in report.fixtures {
                println!(
                    "- {}: {} Hz, {} channel(s), {} bit, {} sample(s), sha256 {}",
                    fixture.id,
                    fixture.sample_rate,
                    fixture.channels,
                    fixture.bits_per_sample,
                    fixture.total_samples,
                    fixture.sha256
                );
            }
            Ok(())
        }
        CorpusCommand::Verify(args) => {
            let report = verify_corpus_manifest(&args.manifest)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report["ok"].as_bool().unwrap_or(false) {
                bail!("corpus verification failed");
            }
            Ok(())
        }
        CorpusCommand::Inspect(args) => {
            let manifest = resolve_manifest_path(&args.manifest);
            let report = validate_manifest(&manifest)
                .with_context(|| format!("validating {}", manifest.display()))?;
            let analyses = report
                .fixtures
                .iter()
                .map(|fixture| {
                    let analysis = analyze_wav(&fixture.audio_path, AnalysisConfig::default())
                        .with_context(|| format!("analyzing fixture {}", fixture.id))?;
                    Ok(json!({
                        "id": fixture.id,
                        "audio": fixture.audio_path,
                        "analysis": analysis,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;

            println!("{}", serde_json::to_string_pretty(&analyses)?);
            Ok(())
        }
        CorpusCommand::Decode(args) => {
            let manifest = resolve_manifest_path(&args.manifest);
            let report = validate_manifest(&manifest)
                .with_context(|| format!("validating {}", manifest.display()))?;
            fs::create_dir_all(&args.out_dir)
                .with_context(|| format!("creating {}", args.out_dir.display()))?;

            for fixture in report.fixtures {
                let trace = build_analysis_trace(&fixture.audio_path, AnalysisConfig::default())
                    .with_context(|| format!("decoding fixture {}", fixture.id))?;
                let trace_path = args
                    .out_dir
                    .join(format!("{}.trace.json", sanitize_filename(&fixture.id)));
                write_json_file(&trace_path, &trace)?;
                println!("wrote {} -> {}", fixture.id, trace_path.display());
            }
            Ok(())
        }
        CorpusCommand::Observation(args) => match args.command {
            ObservationCommand::Validate(args) => {
                let report = validate_observation_manifest(&args.manifest)
                    .with_context(|| format!("validating {}", args.manifest.display()))?;
                let files = report
                    .files
                    .iter()
                    .map(|file| {
                        json!({
                            "path": file.path,
                            "role": file.role,
                            "media_type": file.media_type,
                            "sha256": file.sha256,
                        })
                    })
                    .collect::<Vec<_>>();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "kind": "observation-validation-report",
                        "manifest_path": report.manifest_path,
                        "observation_id": report.observation_id,
                        "files": files,
                    }))?
                );
                Ok(())
            }
        },
    }
}

fn run_dsp(args: DspArgs) -> Result<()> {
    match args.command {
        DspCommand::Bandpass(args) => {
            ensure_parent_dir(&args.output)?;
            let buffer = AudioBuffer::from_wav(&args.input)
                .with_context(|| format!("reading {}", args.input.display()))?;
            let trace = bandpass_fft(
                &buffer.mono_mixdown(),
                buffer.sample_rate,
                args.low,
                args.high,
            );
            let filtered =
                AudioBuffer::new(buffer.sample_rate, 1, trace.samples).with_context(|| {
                    format!("building filtered audio for {}", args.output.display())
                })?;
            filtered
                .write_wav(&args.output)
                .with_context(|| format!("writing {}", args.output.display()))?;
            let analysis = analyze_wav(&args.output, AnalysisConfig::default())
                .with_context(|| "analyzing filtered WAV")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "kind": "bandpass-report",
                    "input": args.input,
                    "output": args.output,
                    "low_frequency_hz": args.low,
                    "high_frequency_hz": args.high,
                    "sample_count": filtered.samples.len(),
                    "rms_after": analysis.stats.rms,
                    "peak_frequency_hz": analysis.spectral_summary.peak_frequency_hz,
                }))?
            );
            Ok(())
        }
        DspCommand::Tone(args) => {
            let buffer = AudioBuffer::from_wav(&args.wav)
                .with_context(|| format!("reading {}", args.wav.display()))?;
            let report = estimate_tone(
                &buffer.mono_mixdown(),
                buffer.sample_rate,
                AnalysisConfig::default(),
            );
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        DspCommand::Track(args) => {
            let buffer = AudioBuffer::from_wav(&args.wav)
                .with_context(|| format!("reading {}", args.wav.display()))?;
            let report = track_frequency(
                &buffer.mono_mixdown(),
                buffer.sample_rate,
                AnalysisConfig::default(),
            );
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        DspCommand::Soft(args) => {
            let buffer = AudioBuffer::from_wav(&args.wav)
                .with_context(|| format!("reading {}", args.wav.display()))?;
            let report = soft_decision_trace(
                &buffer.mono_mixdown(),
                buffer.sample_rate,
                args.mark,
                args.space,
                args.samples_per_symbol,
            );
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

fn run_decode(args: DecodeArgs) -> Result<()> {
    let trace = build_analysis_trace(&args.wav, AnalysisConfig::default())
        .with_context(|| "decoding WAV")?;

    if let Some(trace_path) = args.trace {
        write_json_file(&trace_path, &trace)?;
        println!("wrote analysis trace to {}", trace_path.display());
    } else {
        println!("{}", serde_json::to_string_pretty(&trace)?);
    }

    Ok(())
}

fn run_frames(args: FramesArgs) -> Result<()> {
    match args.command {
        FramesCommand::Classify(args) => {
            let trace = build_analysis_trace(&args.wav, AnalysisConfig::default())
                .with_context(|| "decoding WAV")?;
            let report = classify_trace(&trace);
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        FramesCommand::Pipeline(args) => {
            let trace = build_analysis_trace(&args.wav, AnalysisConfig::default())
                .with_context(|| "decoding WAV")?;
            let report = run_receive_pipeline(&trace);
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

fn run_fixture(args: FixtureArgs) -> Result<()> {
    match args.command {
        FixtureCommand::Suite(args) => run_fixture_suite(args),
        FixtureCommand::Synth(args) => {
            ensure_parent_dir(&args.output)?;
            let config = SyntheticWavConfig {
                sample_rate: args.sample_rate,
                duration_seconds: args.duration,
                amplitude: args.amplitude,
                signal: synth_signal(&args),
            };
            write_synthetic_wav(&args.output, config)
                .with_context(|| format!("writing synthetic fixture {}", args.output.display()))?;
            let analysis = analyze_wav(&args.output, AnalysisConfig::default())
                .with_context(|| "analyzing synthesized fixture")?;
            println!("{}", serde_json::to_string_pretty(&analysis)?);
            Ok(())
        }
    }
}

fn run_fixture_suite(args: SuiteArgs) -> Result<()> {
    let report = build_fixture_suite(FixtureSuiteConfig {
        out_dir: args.out_dir,
        sample_rate: args.sample_rate,
        overwrite: args.overwrite,
    })?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

struct FixtureSuiteConfig {
    out_dir: PathBuf,
    sample_rate: u32,
    overwrite: bool,
}

fn build_fixture_suite(config: FixtureSuiteConfig) -> Result<serde_json::Value> {
    if config.out_dir.exists() && config.out_dir.read_dir()?.next().is_some() && !config.overwrite {
        bail!(
            "output directory is not empty; pass --overwrite: {}",
            config.out_dir.display()
        );
    }
    fs::create_dir_all(&config.out_dir)
        .with_context(|| format!("creating {}", config.out_dir.display()))?;

    let silence = config.out_dir.join("silence.wav");
    let tone = config.out_dir.join("tone-burst.wav");
    let sweep = config.out_dir.join("sweep.wav");
    let noisy = config.out_dir.join("tone-burst-noisy.wav");
    let loopback = config.out_dir.join("tone-burst-loopback.wav");
    let manifest = config.out_dir.join("manifest.toml");
    let host_script = config.out_dir.join("host-script.txt");

    write_synthetic_wav(
        &silence,
        SyntheticWavConfig {
            sample_rate: config.sample_rate,
            duration_seconds: 1.0,
            amplitude: 0.0,
            signal: SyntheticSignal::Silence,
        },
    )
    .with_context(|| format!("writing {}", silence.display()))?;
    write_synthetic_wav(
        &tone,
        SyntheticWavConfig {
            sample_rate: config.sample_rate,
            duration_seconds: 1.0,
            amplitude: 0.5,
            signal: SyntheticSignal::ToneBurst {
                frequency_hz: 1000.0,
                start_seconds: 0.25,
                burst_seconds: 0.375,
            },
        },
    )
    .with_context(|| format!("writing {}", tone.display()))?;
    write_synthetic_wav(
        &sweep,
        SyntheticWavConfig {
            sample_rate: config.sample_rate,
            duration_seconds: 1.0,
            amplitude: 0.5,
            signal: SyntheticSignal::Sweep {
                start_frequency_hz: 300.0,
                end_frequency_hz: 1800.0,
            },
        },
    )
    .with_context(|| format!("writing {}", sweep.display()))?;
    simulate_channel(
        &tone,
        &noisy,
        ChannelConfig {
            gain: 0.8,
            snr_db: Some(24.0),
            sample_rate_drift_ppm: None,
        },
    )
    .with_context(|| format!("writing {}", noisy.display()))?;

    let tone_buffer =
        AudioBuffer::from_wav(&tone).with_context(|| format!("reading {}", tone.display()))?;
    let loopback_buffer = virtual_loopback(
        &tone_buffer,
        LoopbackConfig {
            latency_frames: (config.sample_rate / 100) as usize,
            gain: 0.8,
        },
    )
    .with_context(|| "building loopback fixture")?;
    loopback_buffer
        .write_wav(&loopback)
        .with_context(|| format!("writing {}", loopback.display()))?;

    fs::write(
        &host_script,
        "PING\nMYCALL ja1tst\nCONNECT ja1qso\nCONNECTED\nSEND hello\nACK 1\nDISCONNECT\nPEER-DISCONNECTED\n",
    )
    .with_context(|| format!("writing {}", host_script.display()))?;

    let fixtures = [
        SuiteFixture {
            id: "silence",
            path: &silence,
            payload: "synthetic silence",
            expected: "no-signal",
        },
        SuiteFixture {
            id: "tone-burst",
            path: &tone,
            payload: "synthetic 1000 Hz tone burst",
            expected: "narrowband-burst",
        },
        SuiteFixture {
            id: "sweep",
            path: &sweep,
            payload: "synthetic frequency sweep",
            expected: "wideband-or-unstable-signal",
        },
        SuiteFixture {
            id: "tone-burst-noisy",
            path: &noisy,
            payload: "synthetic 1000 Hz tone burst with deterministic noise",
            expected: "narrowband-burst",
        },
        SuiteFixture {
            id: "tone-burst-loopback",
            path: &loopback,
            payload: "synthetic 1000 Hz tone burst with virtual latency and gain",
            expected: "narrowband-burst",
        },
    ];
    write_suite_manifest(&manifest, config.sample_rate, &fixtures)?;
    validate_manifest(&manifest).with_context(|| format!("validating {}", manifest.display()))?;

    let reports = fixtures
        .iter()
        .map(|fixture| {
            let trace = build_analysis_trace(fixture.path, AnalysisConfig::default())?;
            let classification = classify_trace(&trace);
            Ok(json!({
                "id": fixture.id,
                "audio": fixture.path,
                "sha256": file_sha256_hex(fixture.path)?,
                "candidate_count": classification.candidate_count,
                "classes": classification
                    .candidates
                    .iter()
                    .map(|candidate| format!("{:?}", candidate.class))
                    .collect::<Vec<_>>(),
            }))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(json!({
        "kind": "fixture-suite-report",
        "out_dir": config.out_dir,
        "manifest": manifest,
        "host_script": host_script,
        "fixtures": reports,
    }))
}

fn run_inspect(args: InspectArgs) -> Result<()> {
    let analysis =
        analyze_wav(&args.wav, AnalysisConfig::default()).with_context(|| "analyzing WAV")?;
    println!("{}", serde_json::to_string_pretty(&analysis)?);
    Ok(())
}

struct SuiteFixture<'a> {
    id: &'static str,
    path: &'a Path,
    payload: &'static str,
    expected: &'static str,
}

fn write_suite_manifest(
    path: &Path,
    sample_rate: u32,
    fixtures: &[SuiteFixture<'_>],
) -> Result<()> {
    let mut manifest = String::from("schema_version = 1\n\n");
    for fixture in fixtures {
        let audio = fixture
            .path
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .unwrap_or_default();
        let sha256 = file_sha256_hex(fixture.path)
            .with_context(|| format!("hashing {}", fixture.path.display()))?;
        manifest.push_str(&format!(
            r#"[[fixtures]]
id = "{id}"
audio = "{audio}"
sample_rate = {sample_rate}
channels = 1
payload = "{payload}"
provenance = "clean-public"
expected = "{expected}"
sha256 = "{sha256}"

"#,
            id = fixture.id,
            audio = audio,
            sample_rate = sample_rate,
            payload = fixture.payload,
            expected = fixture.expected,
            sha256 = sha256,
        ));
    }
    fs::write(path, manifest).with_context(|| format!("writing {}", path.display()))
}

fn run_host(args: HostArgs) -> Result<()> {
    match args.command {
        HostCommand::Eval(args) => {
            let mut shell = ModemShell::new();
            let line = args.line.join(" ");
            let reply = shell.execute_line(&line);
            println!("{}", serde_json::to_string_pretty(&reply)?);
            Ok(())
        }
        HostCommand::Script(args) => {
            let report = run_host_script_report(&args.path)?;
            println!("{}", serde_json::to_string_pretty(&report["replies"])?);
            Ok(())
        }
    }
}

fn run_modem(args: ModemArgs) -> Result<()> {
    match args.command {
        ModemCommand::LiveAudio(args) => {
            let hamlib = args.hamlib_host.map(|host| HamlibConfig {
                host,
                timeout_ms: args.hamlib_timeout_ms,
            });
            let report = run_live_audio_modem(LiveAudioModemConfig {
                payload: args.payload,
                input_device: args.input_device,
                output_device: args.output_device,
                sample_rate: args.options.sample_rate,
                channels: args.channels,
                symbol_rate: args.options.symbol_rate,
                mark_frequency_hz: args.options.mark,
                space_frequency_hz: args.options.space,
                amplitude: args.options.amplitude,
                tx_gain: args.tx_gain,
                rx_seconds: args.rx_seconds,
                live: args.live,
                allow_transmit_audio: args.allow_transmit_audio,
                key_ptt: args.key_ptt,
                hamlib,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        ModemCommand::Encode(args) => {
            ensure_parent_dir(&args.output)?;
            let config = packet_config(&args.options);
            let signal = encode_packet_payload(args.payload.as_bytes(), config)
                .with_context(|| "encoding packet payload")?;
            AudioBuffer::new(config.sample_rate, 1, signal.samples)
                .with_context(|| "building packet audio")?
                .write_wav(&args.output)
                .with_context(|| format!("writing {}", args.output.display()))?;
            let mut report = serde_json::to_value(signal.report)?;
            report["output"] = json!(args.output);
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        ModemCommand::Decode(args) => {
            let input = AudioBuffer::from_wav(&args.input)
                .with_context(|| format!("reading {}", args.input.display()))?;
            let report = decode_packet_samples(
                &input.mono_mixdown(),
                input.sample_rate,
                packet_config(&args.options),
            )
            .with_context(|| "decoding packet audio")?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                bail!("packet decode failed");
            }
            Ok(())
        }
        ModemCommand::Roundtrip(args) => {
            let config = packet_config(&args.options);
            let signal = encode_packet_payload(args.payload.as_bytes(), config)
                .with_context(|| "encoding packet payload")?;
            if let Some(path) = &args.out {
                ensure_parent_dir(path)?;
                AudioBuffer::new(config.sample_rate, 1, signal.samples.clone())
                    .with_context(|| "building packet audio")?
                    .write_wav(path)
                    .with_context(|| format!("writing {}", path.display()))?;
            }
            let decoded = decode_packet_samples(&signal.samples, config.sample_rate, config)
                .with_context(|| "decoding packet audio")?;
            let ok = decoded.ok && decoded.payload_hex == hex_string(args.payload.as_bytes());
            let report = json!({
                "kind": "packet-roundtrip-report",
                "ok": ok,
                "output": args.out,
                "encode": signal.report,
                "decode": decoded,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !ok {
                bail!("packet roundtrip failed");
            }
            Ok(())
        }
        ModemCommand::Samples(args) => {
            let report = run_modem_samples(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report["ok"].as_bool().unwrap_or(false) {
                bail!("packet audio sample generation failed");
            }
            Ok(())
        }
        ModemCommand::Sweep(args) => {
            let report = run_modem_sweep(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report["ok"].as_bool().unwrap_or(false) {
                bail!("packet modem sweep failed");
            }
            Ok(())
        }
    }
}

fn run_modem_samples(args: ModemSamplesArgs) -> Result<Value> {
    if args.out_dir.exists() && args.out_dir.read_dir()?.next().is_some() && !args.overwrite {
        bail!(
            "output directory is not empty; pass --overwrite: {}",
            args.out_dir.display()
        );
    }
    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("creating {}", args.out_dir.display()))?;

    let config = packet_config(&args.options);
    let payload = args.payload.as_bytes();
    let payload_hex = hex_string(payload);
    let signal =
        encode_packet_payload(payload, config).with_context(|| "encoding packet samples")?;

    let tx_path = args.out_dir.join("tx-packet.wav");
    let rx_clean_path = args.out_dir.join("rx-clean.wav");
    let rx_loopback_path = args.out_dir.join("rx-loopback.wav");
    let rx_impaired_path = args.out_dir.join("rx-impaired.wav");
    let rx_silence_path = args.out_dir.join("rx-silence.wav");
    let report_path = args.out_dir.join("samples-report.json");
    let tx_decode_path = args.out_dir.join("tx-packet.decode.json");
    let clean_decode_path = args.out_dir.join("rx-clean.decode.json");
    let loopback_decode_path = args.out_dir.join("rx-loopback.decode.json");
    let impaired_decode_path = args.out_dir.join("rx-impaired.decode.json");
    let silence_decode_path = args.out_dir.join("rx-silence.decode.json");

    let tx_buffer = AudioBuffer::new(config.sample_rate, 1, signal.samples.clone())
        .with_context(|| "building TX packet audio")?;
    tx_buffer
        .write_wav(&tx_path)
        .with_context(|| format!("writing {}", tx_path.display()))?;
    AudioBuffer::new(config.sample_rate, 1, signal.samples.clone())
        .with_context(|| "building clean RX packet audio")?
        .write_wav(&rx_clean_path)
        .with_context(|| format!("writing {}", rx_clean_path.display()))?;

    let tx_decode = decode_packet_wav(&tx_path, config)?;
    write_json_file(&tx_decode_path, &tx_decode)?;

    let clean_decode = decode_packet_wav(&rx_clean_path, config)?;
    write_json_file(&clean_decode_path, &clean_decode)?;

    let loopback_buffer = virtual_loopback(
        &tx_buffer,
        LoopbackConfig {
            latency_frames: args.rx_latency_frames,
            gain: args.gain,
        },
    )
    .with_context(|| "building loopback RX sample")?;
    loopback_buffer
        .write_wav(&rx_loopback_path)
        .with_context(|| format!("writing {}", rx_loopback_path.display()))?;
    let loopback_decode = decode_packet_wav(&rx_loopback_path, config)?;
    write_json_file(&loopback_decode_path, &loopback_decode)?;

    let impaired_analysis = simulate_channel(
        &tx_path,
        &rx_impaired_path,
        ChannelConfig {
            gain: args.gain,
            snr_db: Some(args.snr_db),
            sample_rate_drift_ppm: Some(args.drift_ppm),
        },
    )
    .with_context(|| "building impaired RX sample")?;
    let impaired_decode = decode_packet_wav(&rx_impaired_path, config)?;
    write_json_file(&impaired_decode_path, &impaired_decode)?;

    write_synthetic_wav(
        &rx_silence_path,
        SyntheticWavConfig {
            sample_rate: config.sample_rate,
            duration_seconds: (signal.report.duration_seconds as f32).max(1.0),
            amplitude: 0.0,
            signal: SyntheticSignal::Silence,
        },
    )
    .with_context(|| format!("writing {}", rx_silence_path.display()))?;
    let silence_decode = decode_packet_wav(&rx_silence_path, config)?;
    write_json_file(&silence_decode_path, &silence_decode)?;

    let positive_decodes_ok = [
        &tx_decode,
        &clean_decode,
        &loopback_decode,
        &impaired_decode,
    ]
    .iter()
    .all(|decode| packet_decode_matches(decode, &payload_hex));
    let ok = positive_decodes_ok && !silence_decode.ok;

    let report = json!({
        "kind": "packet-audio-samples-report",
        "ok": ok,
        "out_dir": args.out_dir,
        "report_path": report_path,
        "payload_text": args.payload,
        "payload_hex": payload_hex,
        "files": {
            "tx_packet": tx_path,
            "rx_clean": rx_clean_path,
            "rx_loopback": rx_loopback_path,
            "rx_impaired": rx_impaired_path,
            "rx_silence": rx_silence_path,
        },
        "decode_reports": {
            "tx_packet": tx_decode_path,
            "rx_clean": clean_decode_path,
            "rx_loopback": loopback_decode_path,
            "rx_impaired": impaired_decode_path,
            "rx_silence": silence_decode_path,
        },
        "sample_roles": {
            "tx_packet": "playback reference for guarded transmit tests",
            "rx_clean": "receiver golden sample with no impairment",
            "rx_loopback": "receiver sample with virtual latency and gain",
            "rx_impaired": "receiver sample with deterministic noise and sample-rate drift",
            "rx_silence": "negative-control receiver sample that must not decode",
        },
        "codec": {
            "sample_rate": config.sample_rate,
            "symbol_rate": config.symbol_rate,
            "mark_frequency_hz": config.mark_frequency_hz,
            "space_frequency_hz": config.space_frequency_hz,
            "amplitude": config.amplitude,
        },
        "channel": {
            "rx_latency_frames": args.rx_latency_frames,
            "gain": args.gain,
            "snr_db": args.snr_db,
            "sample_rate_drift_ppm": args.drift_ppm,
            "impaired_output_samples": impaired_analysis.total_samples,
            "impaired_rms": impaired_analysis.stats.rms,
        },
        "encode": signal.report,
        "decode": {
            "tx_packet": tx_decode,
            "rx_clean": clean_decode,
            "rx_loopback": loopback_decode,
            "rx_impaired": impaired_decode,
            "rx_silence": silence_decode,
        },
    });
    write_json_file(&report_path, &report)?;
    Ok(report)
}

fn packet_config(options: &ModemOptions) -> PacketCodecConfig {
    PacketCodecConfig {
        sample_rate: options.sample_rate,
        symbol_rate: options.symbol_rate,
        mark_frequency_hz: options.mark,
        space_frequency_hz: options.space,
        amplitude: options.amplitude,
    }
}

fn decode_packet_wav(path: &Path, config: PacketCodecConfig) -> Result<PacketDecodeReport> {
    let input =
        AudioBuffer::from_wav(path).with_context(|| format!("reading {}", path.display()))?;
    decode_packet_samples(&input.mono_mixdown(), input.sample_rate, config)
        .with_context(|| format!("decoding packet audio {}", path.display()))
}

fn packet_decode_matches(report: &PacketDecodeReport, payload_hex: &str) -> bool {
    report.ok && report.payload_hex == payload_hex
}

fn run_modem_sweep(args: ModemSweepArgs) -> Result<Value> {
    if args.out_dir.exists() && args.out_dir.read_dir()?.next().is_some() && !args.overwrite {
        bail!(
            "output directory is not empty; pass --overwrite: {}",
            args.out_dir.display()
        );
    }
    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("creating {}", args.out_dir.display()))?;
    let cases_dir = args.out_dir.join("cases");
    fs::create_dir_all(&cases_dir).with_context(|| format!("creating {}", cases_dir.display()))?;

    let config = packet_config(&args.options);
    let payload = args.payload.as_bytes();
    let payload_hex = hex_string(payload);
    let signal = encode_packet_payload(payload, config).with_context(|| "encoding sweep packet")?;
    let baseline_path = args.out_dir.join("baseline.wav");
    AudioBuffer::new(config.sample_rate, 1, signal.samples)
        .with_context(|| "building sweep baseline audio")?
        .write_wav(&baseline_path)
        .with_context(|| format!("writing {}", baseline_path.display()))?;

    let snr_cases = modem_snr_cases(&args.snr_db);
    let drift_cases = modem_drift_cases(&args.drift_ppm);
    let mut cases = Vec::new();
    let mut case_index = 0_usize;
    for snr_db in &snr_cases {
        for drift_ppm in &drift_cases {
            let id = sanitize_filename(&format!(
                "case-{case_index:02}-snr-{}-drift-{}",
                snr_label(*snr_db),
                number_label(*drift_ppm)
            ));
            let output = cases_dir.join(format!("{id}.wav"));
            let decode_path = cases_dir.join(format!("{id}.decode.json"));
            let analysis = simulate_channel(
                &baseline_path,
                &output,
                ChannelConfig {
                    gain: args.gain,
                    snr_db: *snr_db,
                    sample_rate_drift_ppm: Some(*drift_ppm),
                },
            )
            .with_context(|| format!("running sweep case {id}"))?;
            let received = AudioBuffer::from_wav(&output)
                .with_context(|| format!("reading {}", output.display()))?;
            let decode =
                decode_packet_samples(&received.mono_mixdown(), received.sample_rate, config)
                    .with_context(|| format!("decoding sweep case {id}"))?;
            write_json_file(&decode_path, &decode)?;
            let ok = decode.ok && decode.payload_hex == payload_hex;
            cases.push(json!({
                "id": id,
                "ok": ok,
                "output": output,
                "decode": decode_path,
                "gain": args.gain,
                "snr_db": snr_db,
                "sample_rate_drift_ppm": drift_ppm,
                "payload_hex": decode.payload_hex,
                "crc_expected": decode.crc_expected,
                "crc_actual": decode.crc_actual,
                "observed_symbol_count": decode.observed_symbol_count,
                "output_samples": analysis.total_samples,
                "rms": analysis.stats.rms,
            }));
            case_index += 1;
        }
    }

    let passed_count = cases
        .iter()
        .filter(|case| case["ok"].as_bool().unwrap_or(false))
        .count();
    let report_path = args.out_dir.join("sweep-report.json");
    let report = json!({
        "kind": "packet-sweep-report",
        "ok": passed_count == cases.len(),
        "payload_text": args.payload,
        "payload_hex": payload_hex,
        "out_dir": args.out_dir,
        "report_path": report_path,
        "baseline": baseline_path,
        "encode": signal.report,
        "gain": args.gain,
        "snr_cases": snr_cases,
        "drift_ppm_cases": drift_cases,
        "case_count": cases.len(),
        "passed_count": passed_count,
        "cases": cases,
    });
    write_json_file(&report_path, &report)?;
    Ok(report)
}

fn modem_snr_cases(values: &[f32]) -> Vec<Option<f32>> {
    let mut cases = vec![None];
    if values.is_empty() {
        cases.extend([Some(30.0), Some(24.0)]);
    } else {
        cases.extend(values.iter().copied().map(Some));
    }
    cases
}

fn modem_drift_cases(values: &[f32]) -> Vec<f32> {
    if values.is_empty() {
        vec![0.0, 100.0, -100.0]
    } else {
        values.to_vec()
    }
}

fn snr_label(snr_db: Option<f32>) -> String {
    snr_db
        .map(number_label)
        .unwrap_or_else(|| "clean".to_owned())
}

fn number_label(value: f32) -> String {
    format!("{value:.0}").replace('-', "neg")
}

fn run_lab(args: LabArgs) -> Result<()> {
    match args.command {
        LabCommand::Compare(args) => {
            let expected = load_lab_snapshot(&args.expected)?;
            let actual = load_lab_snapshot(&args.actual)?;
            let report = compare_lab_snapshots(
                &args.expected,
                &expected,
                &args.actual,
                &actual,
                args.tone_tolerance_hz,
            );
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report["ok"].as_bool().unwrap_or(false) {
                bail!("lab comparison failed");
            }
            Ok(())
        }
        LabCommand::Run(args) => {
            let report = run_lab_report(args)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        LabCommand::Snapshot(args) => {
            let source = read_json_file(&args.report)?;
            let snapshot = lab_snapshot_from_value(&source)
                .with_context(|| format!("snapshotting {}", args.report.display()))?;
            if let Some(path) = args.out {
                write_json_file(&path, &snapshot)?;
            }
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
            Ok(())
        }
    }
}

fn run_lab_report(args: LabRunArgs) -> Result<serde_json::Value> {
    if args.out_dir.exists() && args.out_dir.read_dir()?.next().is_some() && !args.overwrite {
        bail!(
            "output directory is not empty; pass --overwrite: {}",
            args.out_dir.display()
        );
    }
    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("creating {}", args.out_dir.display()))?;

    let fixtures_dir = args.out_dir.join("fixtures");
    let artifacts_dir = args.out_dir.join("artifacts");
    let suite = build_fixture_suite(FixtureSuiteConfig {
        out_dir: fixtures_dir.clone(),
        sample_rate: args.sample_rate,
        overwrite: true,
    })?;
    let verification = verify_corpus_manifest(&fixtures_dir)?;
    let artifacts = write_lab_artifacts(&fixtures_dir, &artifacts_dir)?;
    let modem = write_lab_modem_artifacts(&artifacts_dir.join("modem"), args.sample_rate)?;
    let host_script = fixtures_dir.join("host-script.txt");
    let host = run_host_script_report(&host_script)?;
    let chat = write_lab_chat_artifacts(&artifacts_dir.join("chat"))?;
    let ok = verification["ok"].as_bool().unwrap_or(false)
        && modem["ok"].as_bool().unwrap_or(false)
        && host["ok"].as_bool().unwrap_or(false)
        && chat["ok"].as_bool().unwrap_or(false)
        && artifacts
            .iter()
            .all(|artifact| artifact["ok"].as_bool().unwrap_or(false));
    let report_path = args.out_dir.join("lab-report.json");
    let report = json!({
        "kind": "lab-run-report",
        "ok": ok,
        "out_dir": args.out_dir,
        "report_path": report_path,
        "fixtures_dir": fixtures_dir,
        "artifacts_dir": artifacts_dir,
        "suite": suite,
        "verification": verification,
        "host": host,
        "modem": modem,
        "chat": chat,
        "artifacts": artifacts,
    });
    write_json_file(&report_path, &report)?;
    Ok(report)
}

fn load_lab_snapshot(path: &Path) -> Result<Value> {
    let source = read_json_file(path)?;
    lab_snapshot_from_value(&source).with_context(|| format!("loading {}", path.display()))
}

fn lab_snapshot_from_value(value: &Value) -> Result<Value> {
    match value["kind"].as_str() {
        Some("lab-snapshot") => normalize_lab_snapshot(value),
        Some("lab-run-report") => lab_snapshot_from_report(value),
        Some(kind) => bail!("expected lab-run-report or lab-snapshot, got {kind}"),
        None => bail!("expected lab-run-report or lab-snapshot, got JSON without kind"),
    }
}

fn normalize_lab_snapshot(value: &Value) -> Result<Value> {
    let mut snapshot = value.clone();
    let fixture_count = {
        let fixtures = snapshot["fixtures"]
            .as_array_mut()
            .context("lab snapshot missing fixtures array")?;
        fixtures.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
        fixtures.len()
    };
    snapshot["fixture_count"] = json!(fixture_count);
    Ok(snapshot)
}

fn lab_snapshot_from_report(report: &Value) -> Result<Value> {
    let verification_by_id = array_field(&report["verification"], "fixtures")
        .iter()
        .filter_map(|fixture| Some((fixture["id"].as_str()?.to_string(), fixture)))
        .collect::<HashMap<_, _>>();
    let mut fixtures = array_field(report, "artifacts")
        .iter()
        .map(|artifact| {
            let id = artifact["id"]
                .as_str()
                .context("lab artifact missing fixture id")?;
            let verification = verification_by_id.get(id);
            Ok(json!({
                "id": id,
                "expected_label": verification
                    .and_then(|fixture| fixture["expected"].as_str())
                    .unwrap_or("unknown"),
                "verified": verification
                    .and_then(|fixture| fixture["passed"].as_bool())
                    .unwrap_or(false),
                "candidate_count": artifact["candidate_count"].as_u64().unwrap_or(0),
                "classes": artifact["classes"].as_array().cloned().unwrap_or_default(),
                "dominant_frequency_hz": artifact
                    .get("dominant_frequency_hz")
                    .cloned()
                    .unwrap_or(Value::Null),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    fixtures.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));

    let host_commands = array_field(&report["host"], "replies")
        .iter()
        .filter_map(|reply| reply["command"].as_str())
        .collect::<Vec<_>>();
    let host_replies = array_field(&report["host"], "replies")
        .iter()
        .map(|entry| {
            json!({
                "command": field_value(entry, "command"),
                "ok": field_value(&entry["reply"], "ok"),
                "message": field_value(&entry["reply"], "message"),
                "actions": field_value(&entry["reply"], "actions"),
            })
        })
        .collect::<Vec<_>>();
    let modem = json!({
        "ok": report["modem"]["ok"].as_bool().unwrap_or(false),
        "payload_hex": field_value(&report["modem"], "payload_hex"),
        "encode_crc16": field_value(&report["modem"]["encode"], "crc16"),
        "direct_decode": {
            "ok": field_value(&report["modem"]["direct_decode"], "ok"),
            "payload_hex": field_value(&report["modem"]["direct_decode"], "payload_hex"),
            "crc_actual": field_value(&report["modem"]["direct_decode"], "crc_actual"),
        },
        "impaired_decode": {
            "ok": field_value(&report["modem"]["impaired_decode"], "ok"),
            "payload_hex": field_value(&report["modem"]["impaired_decode"], "payload_hex"),
            "crc_actual": field_value(&report["modem"]["impaired_decode"], "crc_actual"),
            "observed_symbol_count": field_value(
                &report["modem"]["impaired_decode"],
                "observed_symbol_count"
            ),
        },
    });
    let chat = json!({
        "ok": report["chat"]["ok"].as_bool().unwrap_or(false),
        "fake_script_ok": field_value(&report["chat"]["fake_script"], "ok"),
        "app_script_ok": field_value(&report["chat"]["app_script"], "ok"),
        "app_beacon_count": report["chat"]["app_script"]["state"]["beacons"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "app_cq_count": report["chat"]["app_script"]["state"]["cq_calls"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "app_mailbox_count": report["chat"]["app_script"]["state"]["mailbox"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "app_file_offer_count": report["chat"]["app_script"]["state"]["file_offers"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "parse_log_ok": field_value(&report["chat"]["parsed_log"], "ok"),
        "script_log_comparison_ok": field_value(
            &report["chat"]["script_log_comparison"],
            "ok"
        ),
        "peer_log_comparison_ok": field_value(
            &report["chat"]["peer_log_comparison"],
            "ok"
        ),
        "local_peer_ok": field_value(&report["chat"]["local_peer"], "ok"),
        "local_peer_app_ok": field_value(&report["chat"]["local_peer_app"], "ok"),
        "message_count": report["chat"]["fake_script"]["transcript"]["messages"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "local_peer_message_count": report["chat"]["local_peer"]["station_a"]["messages"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "local_peer_packet_count": report["chat"]["local_peer"]["packets"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "local_peer_app_packet_count": report["chat"]["local_peer_app"]["packets"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "local_peer_app_beacon_count": report["chat"]["local_peer_app"]["station_a_app"]["beacons"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "local_peer_app_file_offer_count": report["chat"]["local_peer_app"]["station_a_app"]["file_offers"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "local_peer_packets": array_field(&report["chat"]["local_peer"], "packets")
            .iter()
            .map(|packet| {
                json!({
                    "sequence": field_value(packet, "sequence"),
                    "from": field_value(packet, "from"),
                    "to": field_value(packet, "to"),
                    "payload_text": field_value(packet, "payload_text"),
                    "decode_ok": field_value(&packet["decode"], "ok"),
                    "crc_actual": field_value(&packet["decode"], "crc_actual"),
                })
            })
            .collect::<Vec<_>>(),
        "peer_mismatch_count": report["chat"]["peer_log_comparison"]["mismatches"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
    });

    Ok(json!({
        "kind": "lab-snapshot",
        "source_kind": "lab-run-report",
        "ok": report["ok"].as_bool().unwrap_or(false),
        "verification_ok": report["verification"]["ok"].as_bool().unwrap_or(false),
        "host_ok": report["host"]["ok"].as_bool().unwrap_or(false),
        "chat_ok": report["chat"]["ok"].as_bool().unwrap_or(false),
        "host_commands": host_commands,
        "host_replies": host_replies,
        "chat": chat,
        "modem": modem,
        "fixture_count": fixtures.len(),
        "fixtures": fixtures,
    }))
}

fn compare_lab_snapshots(
    expected_path: &Path,
    expected: &Value,
    actual_path: &Path,
    actual: &Value,
    tone_tolerance_hz: f64,
) -> Value {
    let mut differences = Vec::new();

    compare_snapshot_field(
        &mut differences,
        expected,
        actual,
        "ok",
        "lab status mismatch",
    );
    compare_snapshot_field(
        &mut differences,
        expected,
        actual,
        "verification_ok",
        "corpus verification status mismatch",
    );
    compare_snapshot_field(
        &mut differences,
        expected,
        actual,
        "host_ok",
        "host script status mismatch",
    );
    compare_snapshot_field(
        &mut differences,
        expected,
        actual,
        "chat_ok",
        "chat lab status mismatch",
    );
    compare_snapshot_field(
        &mut differences,
        expected,
        actual,
        "host_commands",
        "host command sequence mismatch",
    );
    compare_snapshot_field(
        &mut differences,
        expected,
        actual,
        "host_replies",
        "host reply flow mismatch",
    );
    compare_snapshot_field(
        &mut differences,
        expected,
        actual,
        "chat",
        "chat lab flow mismatch",
    );
    compare_snapshot_field(
        &mut differences,
        expected,
        actual,
        "modem",
        "modem packet flow mismatch",
    );

    let expected_fixtures = array_field(expected, "fixtures");
    let actual_fixtures = array_field(actual, "fixtures");
    let expected_ids = fixture_ids(expected_fixtures);
    let actual_ids = fixture_ids(actual_fixtures);
    if expected_ids != actual_ids {
        push_difference(
            &mut differences,
            "fixtures.ids",
            "fixture set mismatch",
            json!(expected_ids),
            json!(actual_ids),
        );
    }

    let actual_by_id = fixtures_by_id(actual_fixtures);
    for expected_fixture in expected_fixtures {
        let Some(id) = expected_fixture["id"].as_str() else {
            push_difference(
                &mut differences,
                "fixtures.<unknown>.id",
                "expected fixture is missing id",
                expected_fixture.clone(),
                Value::Null,
            );
            continue;
        };
        let Some(actual_fixture) = actual_by_id.get(id) else {
            push_difference(
                &mut differences,
                format!("fixtures.{id}"),
                "fixture missing from actual snapshot",
                expected_fixture.clone(),
                Value::Null,
            );
            continue;
        };
        compare_fixture_field(
            &mut differences,
            id,
            expected_fixture,
            actual_fixture,
            "expected_label",
            "fixture expectation mismatch",
        );
        compare_fixture_field(
            &mut differences,
            id,
            expected_fixture,
            actual_fixture,
            "verified",
            "fixture verification status mismatch",
        );
        compare_fixture_field(
            &mut differences,
            id,
            expected_fixture,
            actual_fixture,
            "candidate_count",
            "candidate count mismatch",
        );
        compare_fixture_field(
            &mut differences,
            id,
            expected_fixture,
            actual_fixture,
            "classes",
            "candidate classes mismatch",
        );
        compare_tone_field(
            &mut differences,
            id,
            field_value(expected_fixture, "dominant_frequency_hz"),
            field_value(actual_fixture, "dominant_frequency_hz"),
            tone_tolerance_hz,
        );
    }

    let expected_by_id = fixtures_by_id(expected_fixtures);
    for actual_fixture in actual_fixtures {
        let Some(id) = actual_fixture["id"].as_str() else {
            push_difference(
                &mut differences,
                "fixtures.<unknown>.id",
                "actual fixture is missing id",
                Value::Null,
                actual_fixture.clone(),
            );
            continue;
        };
        if !expected_by_id.contains_key(id) {
            push_difference(
                &mut differences,
                format!("fixtures.{id}"),
                "unexpected fixture in actual snapshot",
                Value::Null,
                actual_fixture.clone(),
            );
        }
    }

    json!({
        "kind": "lab-compare-report",
        "ok": differences.is_empty(),
        "expected": expected_path,
        "actual": actual_path,
        "tone_tolerance_hz": tone_tolerance_hz,
        "difference_count": differences.len(),
        "differences": differences,
        "expected_snapshot": expected,
        "actual_snapshot": actual,
    })
}

fn compare_snapshot_field(
    differences: &mut Vec<Value>,
    expected: &Value,
    actual: &Value,
    field: &str,
    issue: &str,
) {
    let expected_value = field_value(expected, field);
    let actual_value = field_value(actual, field);
    if expected_value != actual_value {
        push_difference(differences, field, issue, expected_value, actual_value);
    }
}

fn compare_fixture_field(
    differences: &mut Vec<Value>,
    id: &str,
    expected: &Value,
    actual: &Value,
    field: &str,
    issue: &str,
) {
    let expected_value = field_value(expected, field);
    let actual_value = field_value(actual, field);
    if expected_value != actual_value {
        push_difference(
            differences,
            format!("fixtures.{id}.{field}"),
            issue,
            expected_value,
            actual_value,
        );
    }
}

fn compare_tone_field(
    differences: &mut Vec<Value>,
    id: &str,
    expected: Value,
    actual: Value,
    tolerance_hz: f64,
) {
    match (expected.as_f64(), actual.as_f64()) {
        (Some(expected_hz), Some(actual_hz)) => {
            if (expected_hz - actual_hz).abs() > tolerance_hz {
                push_difference(
                    differences,
                    format!("fixtures.{id}.dominant_frequency_hz"),
                    "dominant frequency drift exceeded tolerance",
                    expected,
                    actual,
                );
            }
        }
        (None, None) if expected.is_null() && actual.is_null() => {}
        _ => push_difference(
            differences,
            format!("fixtures.{id}.dominant_frequency_hz"),
            "dominant frequency presence mismatch",
            expected,
            actual,
        ),
    }
}

fn push_difference(
    differences: &mut Vec<Value>,
    path: impl Into<String>,
    issue: &str,
    expected: Value,
    actual: Value,
) {
    differences.push(json!({
        "path": path.into(),
        "issue": issue,
        "expected": expected,
        "actual": actual,
    }));
}

fn field_value(value: &Value, field: &str) -> Value {
    value.get(field).cloned().unwrap_or(Value::Null)
}

fn array_field<'a>(value: &'a Value, field: &str) -> &'a [Value] {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn fixture_ids(fixtures: &[Value]) -> Vec<String> {
    fixtures
        .iter()
        .filter_map(|fixture| fixture["id"].as_str().map(ToOwned::to_owned))
        .collect()
}

fn fixtures_by_id(fixtures: &[Value]) -> HashMap<String, &Value> {
    fixtures
        .iter()
        .filter_map(|fixture| Some((fixture["id"].as_str()?.to_string(), fixture)))
        .collect()
}

fn write_lab_modem_artifacts(modem_dir: &Path, sample_rate: u32) -> Result<Value> {
    fs::create_dir_all(modem_dir).with_context(|| format!("creating {}", modem_dir.display()))?;
    let payload = b"hello chattybara";
    let payload_hex = hex_string(payload);
    let config = PacketCodecConfig {
        sample_rate,
        ..PacketCodecConfig::default()
    };
    let packet_path = modem_dir.join("packet.wav");
    let impaired_path = modem_dir.join("packet-impaired.wav");
    let report_path = modem_dir.join("modem-lab-report.json");

    let signal =
        encode_packet_payload(payload, config).with_context(|| "encoding lab modem packet")?;
    AudioBuffer::new(config.sample_rate, 1, signal.samples.clone())
        .with_context(|| "building lab modem packet audio")?
        .write_wav(&packet_path)
        .with_context(|| format!("writing {}", packet_path.display()))?;
    let direct_decode = decode_packet_samples(&signal.samples, config.sample_rate, config)
        .with_context(|| "decoding lab modem packet")?;

    let impaired_analysis = simulate_channel(
        &packet_path,
        &impaired_path,
        ChannelConfig {
            gain: 0.8,
            snr_db: Some(30.0),
            sample_rate_drift_ppm: Some(100.0),
        },
    )
    .with_context(|| "running lab modem channel")?;
    let impaired = AudioBuffer::from_wav(&impaired_path)
        .with_context(|| format!("reading {}", impaired_path.display()))?;
    let impaired_decode =
        decode_packet_samples(&impaired.mono_mixdown(), impaired.sample_rate, config)
            .with_context(|| "decoding impaired lab modem packet")?;

    let ok = direct_decode.ok
        && impaired_decode.ok
        && direct_decode.payload_hex == payload_hex
        && impaired_decode.payload_hex == payload_hex;
    let report = json!({
        "kind": "modem-lab-report",
        "ok": ok,
        "payload_text": "hello chattybara",
        "payload_hex": payload_hex,
        "packet": packet_path,
        "impaired_packet": impaired_path,
        "report_path": report_path,
        "encode": signal.report,
        "direct_decode": direct_decode,
        "impaired_decode": impaired_decode,
        "channel": {
            "gain": 0.8,
            "snr_db": 30.0,
            "sample_rate_drift_ppm": 100.0,
            "output_samples": impaired_analysis.total_samples,
            "duration_seconds": impaired_analysis.duration_seconds,
            "rms": impaired_analysis.stats.rms,
            "peak": impaired_analysis.stats.peak,
            "peak_frequency_hz": impaired_analysis.spectral_summary.peak_frequency_hz,
        },
    });
    write_json_file(&report_path, &report)?;
    Ok(report)
}

fn write_lab_artifacts(
    fixtures_dir: &Path,
    artifacts_dir: &Path,
) -> Result<Vec<serde_json::Value>> {
    let manifest = fixtures_dir.join("manifest.toml");
    let validation = validate_manifest(&manifest)
        .with_context(|| format!("validating {}", manifest.display()))?;
    let traces_dir = artifacts_dir.join("traces");
    let classifications_dir = artifacts_dir.join("classifications");
    let pipelines_dir = artifacts_dir.join("pipelines");
    let dsp_dir = artifacts_dir.join("dsp");
    fs::create_dir_all(&traces_dir)
        .with_context(|| format!("creating {}", traces_dir.display()))?;
    fs::create_dir_all(&classifications_dir)
        .with_context(|| format!("creating {}", classifications_dir.display()))?;
    fs::create_dir_all(&pipelines_dir)
        .with_context(|| format!("creating {}", pipelines_dir.display()))?;
    fs::create_dir_all(&dsp_dir).with_context(|| format!("creating {}", dsp_dir.display()))?;

    validation
        .fixtures
        .iter()
        .map(|fixture| {
            let stem = sanitize_filename(&fixture.id);
            let trace_path = traces_dir.join(format!("{stem}.trace.json"));
            let classification_path =
                classifications_dir.join(format!("{stem}.classification.json"));
            let pipeline_path = pipelines_dir.join(format!("{stem}.pipeline.json"));
            let tone_path = dsp_dir.join(format!("{stem}.tone.json"));

            let trace = build_analysis_trace(&fixture.audio_path, AnalysisConfig::default())
                .with_context(|| format!("building trace for {}", fixture.id))?;
            write_json_file(&trace_path, &trace)?;

            let classification = classify_trace(&trace);
            write_json_file(&classification_path, &classification)?;

            let pipeline = run_receive_pipeline(&trace);
            write_json_file(&pipeline_path, &pipeline)?;

            let buffer = AudioBuffer::from_wav(&fixture.audio_path)
                .with_context(|| format!("reading {}", fixture.audio_path.display()))?;
            let tone = estimate_tone(
                &buffer.mono_mixdown(),
                buffer.sample_rate,
                AnalysisConfig::default(),
            );
            write_json_file(&tone_path, &tone)?;

            Ok(json!({
                "id": fixture.id,
                "ok": true,
                "audio": fixture.audio_path,
                "trace": trace_path,
                "classification": classification_path,
                "pipeline": pipeline_path,
                "tone": tone_path,
                "candidate_count": classification.candidate_count,
                "classes": classification
                    .candidates
                    .iter()
                    .map(|candidate| candidate_class_name(candidate.class))
                    .collect::<Vec<_>>(),
                "dominant_frequency_hz": tone.frequency_hz,
            }))
        })
        .collect::<Result<Vec<_>>>()
}

fn run_host_script_report(path: &Path) -> Result<serde_json::Value> {
    let mut shell = ModemShell::new();
    let script = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let replies = script
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            json!({
                "command": line,
                "reply": shell.execute_line(line),
            })
        })
        .collect::<Vec<_>>();
    let ok = replies
        .iter()
        .all(|reply| reply["reply"]["ok"].as_bool().unwrap_or(false));

    Ok(json!({
        "kind": "host-script-report",
        "ok": ok,
        "script": path,
        "replies": replies,
    }))
}

fn write_lab_chat_artifacts(chat_dir: &Path) -> Result<serde_json::Value> {
    fs::create_dir_all(chat_dir).with_context(|| format!("creating {}", chat_dir.display()))?;
    let script_path = chat_dir.join("chat-script.txt");
    let app_script_path = chat_dir.join("app-script.txt");
    let local_peer_script_path = chat_dir.join("local-peer-script.txt");
    let local_peer_app_script_path = chat_dir.join("local-peer-app-script.txt");
    let local_peer_dir = chat_dir.join("local-peer");
    let local_peer_app_dir = chat_dir.join("local-peer-app");
    let station_a_log_path = chat_dir.join("station-a-log.txt");
    let station_b_log_path = chat_dir.join("station-b-log.txt");
    let fake_script_path = chat_dir.join("fake-script-report.json");
    let app_script_report_path = chat_dir.join("app-script-report.json");
    let parsed_log_path = chat_dir.join("parsed-log-report.json");
    let script_log_path = chat_dir.join("script-log-comparison.json");
    let peer_log_path = chat_dir.join("peer-log-comparison.json");
    let report_path = chat_dir.join("chat-lab-report.json");

    let script =
        "CONNECT JA1QSO\nSEND hello from chattybara\nRX JA1QSO roger from peer\nDISCONNECT\n";
    let app_script = concat!(
        "BEACON monitoring synthetic local channel\n",
        "CQ testing chattybara app model\n",
        "MAIL JA1QSO Test subject | Synthetic mailbox body.\n",
        "FILE-OFFER JA1QSO sample.txt 42 ",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 ",
        "synthetic file metadata only\n",
        "STATUS\n",
    );
    let local_peer_script =
        "A CONNECT\nA SEND hello from chattybara\nB SEND roger from peer\nA DISCONNECT\n";
    let local_peer_app_script = concat!(
        "A BEACON monitoring synthetic local channel\n",
        "B CQ testing packetized app metadata\n",
        "A MAIL Test subject | Synthetic mailbox body.\n",
        "B FILE-OFFER sample.txt 42 ",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 ",
        "synthetic file metadata only\n",
    );
    let station_a_log = "OUT JA1QSO hello from chattybara\nIN JA1QSO roger from peer\n";
    let station_b_log = "IN JA1TST hello from chattybara\nOUT JA1TST roger from peer\n";
    fs::write(&script_path, script)
        .with_context(|| format!("writing {}", script_path.display()))?;
    fs::write(&app_script_path, app_script)
        .with_context(|| format!("writing {}", app_script_path.display()))?;
    fs::write(&local_peer_script_path, local_peer_script)
        .with_context(|| format!("writing {}", local_peer_script_path.display()))?;
    fs::write(&local_peer_app_script_path, local_peer_app_script)
        .with_context(|| format!("writing {}", local_peer_app_script_path.display()))?;
    fs::write(&station_a_log_path, station_a_log)
        .with_context(|| format!("writing {}", station_a_log_path.display()))?;
    fs::write(&station_b_log_path, station_b_log)
        .with_context(|| format!("writing {}", station_b_log_path.display()))?;

    let fake_script =
        run_fake_script("JA1TST", script).with_context(|| "running lab chat fake script")?;
    write_json_file(&fake_script_path, &fake_script)?;
    let app_script =
        run_app_script("JA1TST", app_script).with_context(|| "running lab chat app script")?;
    write_json_file(&app_script_report_path, &app_script)?;
    let parsed_log =
        parse_simple_log("JA1TST", station_a_log).with_context(|| "parsing lab chat log")?;
    write_json_file(&parsed_log_path, &parsed_log)?;
    let script_log_comparison = compare_fake_script_to_simple_log("JA1TST", script, station_a_log)
        .with_context(|| "comparing lab chat script to log")?;
    write_json_file(&script_log_path, &script_log_comparison)?;
    let peer_log_comparison = compare_peer_logs("JA1TST", station_a_log, "JA1QSO", station_b_log)
        .with_context(|| "comparing lab peer chat logs")?;
    write_json_file(&peer_log_path, &peer_log_comparison)?;
    let local_peer = run_local_peer_script(
        LocalPeerScriptConfig {
            station_a: "JA1TST".to_owned(),
            station_b: "JA1QSO".to_owned(),
            out_dir: local_peer_dir.clone(),
            overwrite: true,
            channel: ChannelConfig::default(),
        },
        local_peer_script,
    )
    .with_context(|| "running lab local peer chat script")?;
    let local_peer_app = run_local_peer_script(
        LocalPeerScriptConfig {
            station_a: "JA1TST".to_owned(),
            station_b: "JA1QSO".to_owned(),
            out_dir: local_peer_app_dir.clone(),
            overwrite: true,
            channel: ChannelConfig::default(),
        },
        local_peer_app_script,
    )
    .with_context(|| "running lab local peer app feature script")?;

    let ok = fake_script.ok
        && app_script.ok
        && parsed_log.ok
        && script_log_comparison.ok
        && peer_log_comparison.ok
        && local_peer.ok
        && local_peer_app.ok;
    let report = json!({
        "kind": "chat-lab-report",
        "ok": ok,
        "directory": chat_dir,
        "script": script_path,
        "app_script_file": app_script_path,
        "local_peer_script": local_peer_script_path,
        "local_peer_app_script": local_peer_app_script_path,
        "local_peer_directory": local_peer_dir,
        "local_peer_app_directory": local_peer_app_dir,
        "station_a_log": station_a_log_path,
        "station_b_log": station_b_log_path,
        "fake_script_report": fake_script_path,
        "app_script_report": app_script_report_path,
        "parsed_log_report": parsed_log_path,
        "script_log_comparison_report": script_log_path,
        "peer_log_comparison_report": peer_log_path,
        "fake_script": fake_script,
        "app_script": app_script,
        "parsed_log": parsed_log,
        "script_log_comparison": script_log_comparison,
        "peer_log_comparison": peer_log_comparison,
        "local_peer": local_peer,
        "local_peer_app": local_peer_app,
    });
    write_json_file(&report_path, &report)?;
    Ok(report)
}

fn run_simulate(args: SimulateArgs) -> Result<()> {
    match args.command {
        SimulateCommand::AppLink(args) => {
            if args.payload_bytes == 0 {
                bail!("--payload-bytes must be greater than zero");
            }
            let mut protocol = AppProtocolState::new(&args.station);
            let mail = protocol.mail(&args.peer, "Synthetic mailbox test", "No-hardware app link");
            let ack = protocol.ack(
                &args.peer,
                mail.id.as_deref().context("mail packet is missing id")?,
                AppDeliveryState::Acknowledged,
            );
            let payload_bytes = vec![b'x'; args.payload_bytes];
            let fragments = protocol.fragment_payload(
                &args.peer,
                "synthetic-payload",
                &payload_bytes,
                DEFAULT_FRAGMENT_DATA_BYTES,
            )?;
            let file_packets = protocol.file_transfer_packets(
                &args.peer,
                "synthetic.bin",
                &payload_bytes,
                Some("simulated transfer".to_owned()),
                DEFAULT_FILE_CHUNK_DATA_BYTES,
            )?;
            let reassembled_payload = reassemble_fragments(&fragments)?;
            let reassembled_file = reassemble_file_chunks(&file_packets)?;
            let mut packets = vec![mail, ack];
            packets.extend(fragments);
            packets.extend(file_packets);
            let max_encoded_bytes = packets
                .iter()
                .map(encode_app_packet)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .map(|payload| payload.len())
                .max()
                .unwrap_or(0);
            let reliability = simulate_reliable_delivery(
                &packets,
                SimulatedAppLinkConfig {
                    max_retries: args.max_retries,
                    timeout_ticks: args.timeout_ticks,
                    drop_first_attempt: args.drop_first_attempt,
                    drop_all_attempts: args.drop_all_attempts,
                    duplicate_deliveries: args.duplicate_deliveries,
                },
            )?;
            let output = json!({
                "kind": "app-link-simulation-report",
                "station": args.station,
                "peer": args.peer,
                "packet_count": packets.len(),
                "payload_bytes": args.payload_bytes,
                "max_encoded_bytes": max_encoded_bytes,
                "fragment": {
                    "message_id": reassembled_payload.message_id,
                    "label": reassembled_payload.label,
                    "byte_count": reassembled_payload.bytes.len(),
                    "sha256": reassembled_payload.sha256,
                },
                "file": {
                    "file_id": reassembled_file.file_id,
                    "filename": reassembled_file.filename,
                    "byte_count": reassembled_file.bytes.len(),
                    "sha256": reassembled_file.sha256,
                },
                "reliability": reliability,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            Ok(())
        }
        SimulateCommand::Channel(args) => {
            ensure_parent_dir(&args.output)?;
            let analysis = simulate_channel(
                &args.input,
                &args.output,
                ChannelConfig {
                    gain: args.gain,
                    snr_db: args.snr,
                    sample_rate_drift_ppm: args.sample_rate_drift_ppm,
                },
            )
            .with_context(|| "running channel simulator")?;
            println!("{}", serde_json::to_string_pretty(&analysis)?);
            Ok(())
        }
    }
}

fn synth_signal(args: &SynthArgs) -> SyntheticSignal {
    match args.kind {
        SynthKind::Silence => SyntheticSignal::Silence,
        SynthKind::ToneBurst => SyntheticSignal::ToneBurst {
            frequency_hz: args.frequency,
            start_seconds: args.start,
            burst_seconds: args.burst_duration,
        },
        SynthKind::Sweep => SyntheticSignal::Sweep {
            start_frequency_hz: args.start_frequency,
            end_frequency_hz: args.end_frequency,
        },
    }
}

fn resolve_manifest_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join("manifest.toml")
    } else {
        path.to_path_buf()
    }
}

fn write_json_file(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    ensure_parent_dir(path)?;
    let json = serde_json::to_string_pretty(value)?;
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

fn read_json_file(path: &Path) -> Result<Value> {
    let json = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    Ok(())
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

fn hex_string(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn verify_corpus_manifest(path: &Path) -> Result<serde_json::Value> {
    let manifest = resolve_manifest_path(path);
    let validation = validate_manifest(&manifest)
        .with_context(|| format!("validating {}", manifest.display()))?;
    let expected_by_id = load_manifest(&manifest)
        .with_context(|| format!("loading {}", manifest.display()))?
        .fixtures
        .into_iter()
        .map(|fixture| (fixture.id, fixture.expected))
        .collect::<HashMap<_, _>>();

    let fixtures = validation
        .fixtures
        .iter()
        .map(|fixture| {
            let expected = expected_by_id
                .get(&fixture.id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned());
            let trace = build_analysis_trace(&fixture.audio_path, AnalysisConfig::default())
                .with_context(|| format!("building trace for {}", fixture.id))?;
            let report = classify_trace(&trace);
            let classes = report
                .candidates
                .iter()
                .map(|candidate| candidate_class_name(candidate.class))
                .collect::<Vec<_>>();
            let (passed, note) = expectation_matches(&expected, &report);

            Ok(json!({
                "id": fixture.id,
                "audio": fixture.audio_path,
                "expected": expected,
                "passed": passed,
                "candidate_count": report.candidate_count,
                "classes": classes,
                "note": note,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let ok = fixtures
        .iter()
        .all(|fixture| fixture["passed"].as_bool().unwrap_or(false));

    Ok(json!({
        "kind": "corpus-verification-report",
        "manifest_path": validation.manifest_path,
        "ok": ok,
        "fixtures": fixtures,
    }))
}

fn expectation_matches(expected: &str, report: &orca_frames::FrameReport) -> (bool, String) {
    let normalized = normalize_expected(expected);
    let has_class = |class| {
        report
            .candidates
            .iter()
            .any(|candidate| candidate.class == class)
    };

    match normalized.as_str() {
        "no-signal" | "silence" => (
            report.candidate_count == 0,
            "expected no detected signal".to_owned(),
        ),
        "any-signal" | "signal" => (
            report.candidate_count > 0,
            "expected at least one detected signal".to_owned(),
        ),
        "narrowband-burst" => (
            has_class(CandidateClass::NarrowbandBurst),
            "expected a narrowband burst candidate".to_owned(),
        ),
        "short-tone" => (
            has_class(CandidateClass::ShortTone),
            "expected a short tone candidate".to_owned(),
        ),
        "weak-signal" => (
            has_class(CandidateClass::WeakSignal),
            "expected a weak signal candidate".to_owned(),
        ),
        "wideband-or-unstable-signal" | "noisy-or-unstable" | "unstable-signal" => (
            has_class(CandidateClass::NoisyOrUnstable),
            "expected an unstable or wideband candidate".to_owned(),
        ),
        _ => (
            false,
            format!("unsupported expected label: {}", expected.trim()),
        ),
    }
}

fn normalize_expected(expected: &str) -> String {
    expected
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| match character {
            ' ' | '_' => '-',
            other => other,
        })
        .collect()
}

fn candidate_class_name(class: CandidateClass) -> &'static str {
    match class {
        CandidateClass::NarrowbandBurst => "narrowband-burst",
        CandidateClass::ShortTone => "short-tone",
        CandidateClass::WeakSignal => "weak-signal",
        CandidateClass::NoisyOrUnstable => "noisy-or-unstable",
        CandidateClass::Unknown => "unknown",
    }
}

fn audit_corpus(root: &Path) -> Result<serde_json::Value> {
    let mut checked = Vec::new();
    let mut issues = Vec::new();
    for manifest in collect_toml_files(&root.join("corpus"))? {
        let text = fs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        if text.contains("tainted-review") || text.contains("blocked") {
            issues.push(json!({
                "path": manifest,
                "issue": "non-public provenance label appears in corpus manifest",
            }));
            continue;
        }
        if text.contains("[[fixtures]]") {
            match validate_manifest(&manifest) {
                Ok(_) => checked.push(manifest),
                Err(error) => issues.push(json!({
                    "path": manifest,
                    "issue": error.to_string(),
                })),
            }
        } else if text.contains("observation_id") {
            match validate_observation_manifest(&manifest) {
                Ok(_) => checked.push(manifest),
                Err(error) => issues.push(json!({
                    "path": manifest,
                    "issue": error.to_string(),
                })),
            }
        }
    }

    Ok(json!({
        "kind": "corpus-audit-report",
        "root": root,
        "ok": issues.is_empty(),
        "checked_manifests": checked,
        "issues": issues,
    }))
}

fn collect_toml_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    collect_toml_files_inner(root, &mut files)?;
    Ok(files)
}

fn collect_toml_files_inner(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root).with_context(|| format!("reading {}", root.display()))? {
        let entry = entry.with_context(|| format!("reading {}", root.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_toml_files_inner(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            files.push(path);
        }
    }
    Ok(())
}

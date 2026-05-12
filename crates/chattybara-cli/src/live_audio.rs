use crate::hamlib::{HamlibConfig, HamlibPttState, hamlib_set_ptt};
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use orca_frames::{
    PacketCodecConfig, PacketDecodeReport, PacketEncodeReport, decode_packet_samples,
    encode_packet_payload,
};
use serde::Serialize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct LiveAudioModemConfig {
    pub payload: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub symbol_rate: f32,
    pub mark_frequency_hz: f32,
    pub space_frequency_hz: f32,
    pub amplitude: f32,
    pub tx_gain: f32,
    pub rx_seconds: f32,
    pub live: bool,
    pub allow_transmit_audio: bool,
    pub key_ptt: bool,
    pub hamlib: Option<HamlibConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiveAudioModemReport {
    pub kind: &'static str,
    pub dry_run: bool,
    pub live_requested: bool,
    pub allow_transmit_audio: bool,
    pub key_ptt: bool,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub rx_seconds: f32,
    pub tx_gain: f32,
    pub payload_len: usize,
    pub encode: PacketEncodeReport,
    pub played_samples: usize,
    pub captured_samples: usize,
    pub decode: Option<PacketDecodeReport>,
    pub ptt_host: Option<String>,
    pub note: String,
}

pub fn run_live_audio_modem(config: LiveAudioModemConfig) -> Result<LiveAudioModemReport> {
    validate_live_audio_config(&config)?;
    let packet_config = packet_config_from_live_audio(&config);
    let signal = encode_packet_payload(config.payload.as_bytes(), packet_config)
        .context("encoding live audio packet")?;
    let payload_len = config.payload.len();
    if !config.live {
        return Ok(LiveAudioModemReport {
            kind: "live-audio-modem-report",
            dry_run: true,
            live_requested: false,
            allow_transmit_audio: config.allow_transmit_audio,
            key_ptt: config.key_ptt,
            input_device: config.input_device,
            output_device: config.output_device,
            sample_rate: config.sample_rate,
            channels: config.channels,
            rx_seconds: config.rx_seconds,
            tx_gain: config.tx_gain,
            payload_len,
            played_samples: 0,
            captured_samples: 0,
            decode: None,
            ptt_host: config.hamlib.map(|value| value.host),
            note: "Dry-run only. No audio device was opened; pass --live and --allow-transmit-audio to play packet audio.".to_owned(),
            encode: signal.report,
        });
    }

    if !config.allow_transmit_audio {
        bail!("live packet audio transmit requires --allow-transmit-audio");
    }
    if config.key_ptt && config.hamlib.is_none() {
        bail!("--key-ptt requires --hamlib-host");
    }

    let hamlib = config.hamlib.clone();
    let key_ptt = config.key_ptt;
    if key_ptt {
        hamlib_set_ptt(hamlib.as_ref().expect("hamlib config"), HamlibPttState::Tx)
            .context("keying Hamlib PTT TX")?;
    }
    let result = run_live_audio_streams(config, signal.samples, signal.report);
    let ptt_rx_result = if key_ptt {
        let hamlib = hamlib.expect("hamlib config");
        Some(hamlib_set_ptt(&hamlib, HamlibPttState::Rx).context("returning Hamlib PTT to RX"))
    } else {
        None
    };
    let report = result?;
    if let Some(result) = ptt_rx_result {
        result?;
    }
    Ok(report)
}

fn run_live_audio_streams(
    config: LiveAudioModemConfig,
    samples: Vec<f32>,
    encode: PacketEncodeReport,
) -> Result<LiveAudioModemReport> {
    let host = cpal::default_host();
    let input_device = select_input_device(&host, config.input_device.as_deref())?;
    let output_device = select_output_device(&host, config.output_device.as_deref())?;
    let input_device_name = device_name(&input_device).ok();
    let output_device_name = device_name(&output_device).ok();
    let input_format = input_device
        .default_input_config()
        .context("reading default input stream config")?
        .sample_format();
    let output_format = output_device
        .default_output_config()
        .context("reading default output stream config")?
        .sample_format();
    let stream_config = cpal::StreamConfig {
        channels: config.channels,
        sample_rate: config.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let captured = Arc::new(Mutex::new(Vec::<f32>::new()));
    let input_stream = build_input_stream(
        &input_device,
        input_format,
        &stream_config,
        captured.clone(),
    )?;
    let cursor = Arc::new(AtomicUsize::new(0));
    let output_samples = Arc::new(samples);
    let output_stream = build_output_stream(
        &output_device,
        output_format,
        &stream_config,
        output_samples.clone(),
        cursor.clone(),
        config.tx_gain,
    )?;

    input_stream.play().context("starting input stream")?;
    output_stream.play().context("starting output stream")?;
    std::thread::sleep(Duration::from_secs_f32(config.rx_seconds));
    drop(output_stream);
    drop(input_stream);

    let captured = captured
        .lock()
        .map_err(|_| anyhow::anyhow!("captured audio lock poisoned"))?
        .clone();
    let decode = decode_packet_samples(
        &captured,
        config.sample_rate,
        packet_config_from_live_audio(&config),
    )
    .ok();
    Ok(LiveAudioModemReport {
        kind: "live-audio-modem-report",
        dry_run: false,
        live_requested: true,
        allow_transmit_audio: config.allow_transmit_audio,
        key_ptt: config.key_ptt,
        input_device: input_device_name,
        output_device: output_device_name,
        sample_rate: config.sample_rate,
        channels: config.channels,
        rx_seconds: config.rx_seconds,
        tx_gain: config.tx_gain,
        payload_len: config.payload.len(),
        played_samples: cursor.load(Ordering::Relaxed).min(output_samples.len()),
        captured_samples: captured.len(),
        decode,
        ptt_host: config.hamlib.map(|value| value.host),
        note: "Live audio stream completed.".to_owned(),
        encode,
    })
}

fn build_input_stream(
    device: &cpal::Device,
    format: cpal::SampleFormat,
    config: &cpal::StreamConfig,
    captured: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream> {
    let channels = config.channels as usize;
    match format {
        cpal::SampleFormat::I8 => {
            build_typed_input_stream::<i8>(device, config, channels, captured)
        }
        cpal::SampleFormat::I16 => {
            build_typed_input_stream::<i16>(device, config, channels, captured)
        }
        cpal::SampleFormat::I24 => {
            build_typed_input_stream::<cpal::I24>(device, config, channels, captured)
        }
        cpal::SampleFormat::I32 => {
            build_typed_input_stream::<i32>(device, config, channels, captured)
        }
        cpal::SampleFormat::I64 => {
            build_typed_input_stream::<i64>(device, config, channels, captured)
        }
        cpal::SampleFormat::U8 => {
            build_typed_input_stream::<u8>(device, config, channels, captured)
        }
        cpal::SampleFormat::U16 => {
            build_typed_input_stream::<u16>(device, config, channels, captured)
        }
        cpal::SampleFormat::U24 => {
            build_typed_input_stream::<cpal::U24>(device, config, channels, captured)
        }
        cpal::SampleFormat::U32 => {
            build_typed_input_stream::<u32>(device, config, channels, captured)
        }
        cpal::SampleFormat::U64 => {
            build_typed_input_stream::<u64>(device, config, channels, captured)
        }
        cpal::SampleFormat::F32 => {
            build_typed_input_stream::<f32>(device, config, channels, captured)
        }
        cpal::SampleFormat::F64 => {
            build_typed_input_stream::<f64>(device, config, channels, captured)
        }
        format => bail!("unsupported input sample format {format:?}"),
    }
}

fn build_typed_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    captured: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream>
where
    T: SizedSample + Sample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| collect_input_samples(data, channels, &captured),
            log_input_stream_error,
            None,
        )
        .context("building input stream")
}

fn build_output_stream(
    device: &cpal::Device,
    format: cpal::SampleFormat,
    config: &cpal::StreamConfig,
    samples: Arc<Vec<f32>>,
    cursor: Arc<AtomicUsize>,
    gain: f32,
) -> Result<cpal::Stream> {
    let channels = config.channels as usize;
    match format {
        cpal::SampleFormat::I8 => {
            build_typed_output_stream::<i8>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::I16 => {
            build_typed_output_stream::<i16>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::I24 => {
            build_typed_output_stream::<cpal::I24>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::I32 => {
            build_typed_output_stream::<i32>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::I64 => {
            build_typed_output_stream::<i64>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::U8 => {
            build_typed_output_stream::<u8>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::U16 => {
            build_typed_output_stream::<u16>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::U24 => {
            build_typed_output_stream::<cpal::U24>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::U32 => {
            build_typed_output_stream::<u32>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::U64 => {
            build_typed_output_stream::<u64>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::F32 => {
            build_typed_output_stream::<f32>(device, config, channels, samples, cursor, gain)
        }
        cpal::SampleFormat::F64 => {
            build_typed_output_stream::<f64>(device, config, channels, samples, cursor, gain)
        }
        format => bail!("unsupported output sample format {format:?}"),
    }
}

fn build_typed_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    samples: Arc<Vec<f32>>,
    cursor: Arc<AtomicUsize>,
    gain: f32,
) -> Result<cpal::Stream>
where
    T: SizedSample + Sample + FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| fill_output_samples(data, channels, &samples, &cursor, gain),
            log_output_stream_error,
            None,
        )
        .context("building output stream")
}

fn select_input_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device> {
    match name {
        Some(name) => find_device_by_name(host, name, AudioDirection::Input),
        None => host
            .default_input_device()
            .context("no default input device available"),
    }
}

fn select_output_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device> {
    match name {
        Some(name) => find_device_by_name(host, name, AudioDirection::Output),
        None => host
            .default_output_device()
            .context("no default output device available"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioDirection {
    Input,
    Output,
}

fn find_device_by_name(
    host: &cpal::Host,
    requested: &str,
    direction: AudioDirection,
) -> Result<cpal::Device> {
    let requested_lower = requested.to_ascii_lowercase();
    let mut partial = None;
    for device in host.devices().context("enumerating audio devices")? {
        let name = device_name(&device).unwrap_or_default();
        let supports_direction = match direction {
            AudioDirection::Input => device.default_input_config().is_ok(),
            AudioDirection::Output => device.default_output_config().is_ok(),
        };
        if !supports_direction {
            continue;
        }
        if name == requested {
            return Ok(device);
        }
        if name.to_ascii_lowercase().contains(&requested_lower) {
            partial = Some(device);
        }
    }
    partial.with_context(|| format!("audio device {requested:?} not found for {direction:?}"))
}

fn device_name(device: &cpal::Device) -> Result<String, cpal::DeviceNameError> {
    device
        .description()
        .map(|description| description.name().to_owned())
}

fn collect_input_samples<T>(input: &[T], channels: usize, captured: &Arc<Mutex<Vec<f32>>>)
where
    T: Sample,
    f32: FromSample<T>,
{
    if channels == 0 {
        return;
    }
    if let Ok(mut captured) = captured.try_lock() {
        captured.extend(input.chunks(channels).map(|frame| {
            frame
                .iter()
                .map(|sample| f32::from_sample(*sample))
                .sum::<f32>()
                / frame.len().max(1) as f32
        }));
    }
}

fn fill_output_samples<T>(
    output: &mut [T],
    channels: usize,
    samples: &[f32],
    cursor: &AtomicUsize,
    gain: f32,
) where
    T: Sample + FromSample<f32>,
{
    if channels == 0 {
        return;
    }
    for frame in output.chunks_mut(channels) {
        let index = cursor.fetch_add(1, Ordering::Relaxed);
        let value = samples.get(index).copied().unwrap_or(0.0) * gain;
        let value = T::from_sample(value.clamp(-1.0, 1.0));
        for sample in frame {
            *sample = value;
        }
    }
}

fn log_input_stream_error(error: cpal::StreamError) {
    eprintln!("input stream error: {error}");
}

fn log_output_stream_error(error: cpal::StreamError) {
    eprintln!("output stream error: {error}");
}

fn validate_live_audio_config(config: &LiveAudioModemConfig) -> Result<()> {
    if config.sample_rate == 0 {
        bail!("sample rate must be greater than zero");
    }
    if config.channels == 0 {
        bail!("channel count must be greater than zero");
    }
    if config.rx_seconds <= 0.0 {
        bail!("--rx-seconds must be greater than zero");
    }
    if !(0.0..=1.0).contains(&config.tx_gain) {
        bail!("--tx-gain must be between 0.0 and 1.0");
    }
    Ok(())
}

fn packet_config_from_live_audio(config: &LiveAudioModemConfig) -> PacketCodecConfig {
    PacketCodecConfig {
        sample_rate: config.sample_rate,
        symbol_rate: config.symbol_rate,
        mark_frequency_hz: config.mark_frequency_hz,
        space_frequency_hz: config.space_frequency_hz,
        amplitude: config.amplitude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_audio_modem_dry_run_does_not_open_devices() {
        let report = run_live_audio_modem(LiveAudioModemConfig {
            payload: "hello".to_owned(),
            input_device: Some("input".to_owned()),
            output_device: Some("output".to_owned()),
            sample_rate: 8000,
            channels: 1,
            symbol_rate: 100.0,
            mark_frequency_hz: 1200.0,
            space_frequency_hz: 1800.0,
            amplitude: 0.55,
            tx_gain: 0.2,
            rx_seconds: 1.0,
            live: false,
            allow_transmit_audio: false,
            key_ptt: false,
            hamlib: None,
        })
        .expect("dry run");

        assert!(report.dry_run);
        assert_eq!(report.payload_len, 5);
        assert_eq!(report.played_samples, 0);
        assert_eq!(report.captured_samples, 0);
        assert!(report.decode.is_none());
    }

    #[test]
    fn output_fill_duplicates_mono_samples_across_channels() {
        let mut output = vec![0.0_f32; 6];
        let cursor = AtomicUsize::new(0);
        fill_output_samples(&mut output, 2, &[0.5, -0.25], &cursor, 0.5);

        assert_eq!(output, vec![0.25, 0.25, -0.125, -0.125, 0.0, 0.0]);
        assert_eq!(cursor.load(Ordering::Relaxed), 3);
    }
}

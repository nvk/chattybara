use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_RADIO_ADDRESS: u8 = 0xA4;
pub const DEFAULT_CONTROLLER_ADDRESS: u8 = 0xE0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ic705Profile {
    pub kind: String,
    pub model: String,
    pub audio: Ic705AudioProfile,
    pub control: Ic705ControlProfile,
    pub safety: Ic705SafetyProfile,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ic705AudioProfile {
    pub input_device: String,
    pub output_device: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub rx_gain: f32,
    pub tx_gain: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ic705ControlProfile {
    pub port: String,
    pub baud_rate: String,
    pub radio_address: String,
    pub controller_address: String,
    pub civ_echo: bool,
    pub ptt_method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ic705SafetyProfile {
    pub allow_transmit: bool,
    pub require_manual_ptt_confirm: bool,
    pub max_tx_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ic705ValidationReport {
    pub kind: &'static str,
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub profile: Ic705Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ic705CivOperation {
    ReadFrequency,
    ReadMode,
    PttRx,
    PttTx,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ic705CivFrameReport {
    pub kind: &'static str,
    pub model: &'static str,
    pub operation: &'static str,
    pub frame_hex: String,
    pub frame_bytes: Vec<u8>,
    pub radio_address: String,
    pub controller_address: String,
    pub expected_ok_hex: String,
    pub transmit_risk: bool,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ic705CivSerialReport {
    pub kind: &'static str,
    pub model: &'static str,
    pub dry_run: bool,
    pub live_requested: bool,
    pub port: Option<String>,
    pub baud_rate: u32,
    pub timeout_ms: u64,
    pub allow_transmit: bool,
    pub frame: Ic705CivFrameReport,
    pub wrote_bytes: usize,
    pub response_hex: Option<String>,
    pub response_bytes: Vec<u8>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ic705CivSerialConfig {
    pub operation: Ic705CivOperation,
    pub radio_address: String,
    pub controller_address: String,
    pub port: Option<String>,
    pub baud_rate: u32,
    pub timeout_ms: u64,
    pub live: bool,
    pub allow_transmit: bool,
}

pub fn default_ic705_profile() -> Ic705Profile {
    Ic705Profile {
        kind: "chattybara-ic705-profile".to_owned(),
        model: "IC-705".to_owned(),
        audio: Ic705AudioProfile {
            input_device: "USB Audio CODEC (IC-705 receive)".to_owned(),
            output_device: "USB Audio CODEC (IC-705 transmit)".to_owned(),
            sample_rate: 48_000,
            channels: 1,
            rx_gain: 0.70,
            tx_gain: 0.20,
        },
        control: Ic705ControlProfile {
            port: "IC-705 Serial Port A (CI-V)".to_owned(),
            baud_rate: "auto".to_owned(),
            radio_address: format!("{DEFAULT_RADIO_ADDRESS:02X}"),
            controller_address: format!("{DEFAULT_CONTROLLER_ADDRESS:02X}"),
            civ_echo: false,
            ptt_method: "ci-v".to_owned(),
        },
        safety: Ic705SafetyProfile {
            allow_transmit: false,
            require_manual_ptt_confirm: true,
            max_tx_seconds: 30,
        },
        notes: vec![
            "Dry-run hardware profile; chattybara does not open audio or serial devices from this profile yet.".to_owned(),
            "Match input_device/output_device to the IC-705 USB audio devices exposed by the host OS.".to_owned(),
            "Match control.port to the IC-705 CI-V serial port before enabling live rig control.".to_owned(),
        ],
    }
}

pub fn ic705_profile_toml(profile: &Ic705Profile) -> Result<String> {
    toml::to_string_pretty(profile).context("serializing IC-705 profile")
}

pub fn load_ic705_profile(path: &Path) -> Result<Ic705Profile> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

pub fn validate_ic705_profile(profile: Ic705Profile) -> Ic705ValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if profile.kind != "chattybara-ic705-profile" {
        errors.push("kind must be chattybara-ic705-profile".to_owned());
    }
    if profile.model != "IC-705" {
        errors.push("model must be IC-705".to_owned());
    }
    if profile.audio.input_device.trim().is_empty() {
        errors.push("audio.input_device is required".to_owned());
    }
    if profile.audio.output_device.trim().is_empty() {
        errors.push("audio.output_device is required".to_owned());
    }
    if profile.audio.sample_rate == 0 {
        errors.push("audio.sample_rate must be greater than zero".to_owned());
    }
    if profile.audio.channels == 0 {
        errors.push("audio.channels must be greater than zero".to_owned());
    }
    if !(0.0..=1.0).contains(&profile.audio.rx_gain) {
        errors.push("audio.rx_gain must be between 0.0 and 1.0".to_owned());
    }
    if !(0.0..=1.0).contains(&profile.audio.tx_gain) {
        errors.push("audio.tx_gain must be between 0.0 and 1.0".to_owned());
    }
    if profile.audio.tx_gain > 0.50 {
        warnings.push(
            "audio.tx_gain is above 0.50; verify ALC stays low before transmitting".to_owned(),
        );
    }
    if profile.control.port.trim().is_empty() {
        errors.push("control.port is required".to_owned());
    }
    if parse_hex_byte(&profile.control.radio_address).is_err() {
        errors.push("control.radio_address must be one CI-V byte such as A4".to_owned());
    }
    if parse_hex_byte(&profile.control.controller_address).is_err() {
        errors.push("control.controller_address must be one CI-V byte such as E0".to_owned());
    }
    if profile.control.baud_rate.trim().is_empty() {
        errors.push("control.baud_rate is required; use auto or a numeric baud rate".to_owned());
    } else if profile.control.baud_rate != "auto" {
        match profile.control.baud_rate.parse::<u32>() {
            Ok(0) => errors.push("control.baud_rate must be greater than zero".to_owned()),
            Ok(_) => {}
            Err(_) => {
                errors.push("control.baud_rate must be auto or a numeric baud rate".to_owned())
            }
        }
    }
    if profile.control.ptt_method != "ci-v" && profile.control.ptt_method != "none" {
        errors.push("control.ptt_method must be ci-v or none".to_owned());
    }
    if profile.safety.allow_transmit {
        warnings.push(
            "safety.allow_transmit is true; dry-run validation cannot prove RF safety".to_owned(),
        );
    }
    if !profile.safety.require_manual_ptt_confirm {
        warnings.push("safety.require_manual_ptt_confirm is false".to_owned());
    }
    if profile.safety.max_tx_seconds == 0 {
        errors.push("safety.max_tx_seconds must be greater than zero".to_owned());
    }

    Ic705ValidationReport {
        kind: "ic705-validation-report",
        ok: errors.is_empty(),
        errors,
        warnings,
        profile,
    }
}

pub fn build_ic705_civ_frame_report(
    operation: Ic705CivOperation,
    radio_address: &str,
    controller_address: &str,
) -> Result<Ic705CivFrameReport> {
    let radio = parse_hex_byte(radio_address).context("parsing radio CI-V address")?;
    let controller =
        parse_hex_byte(controller_address).context("parsing controller CI-V address")?;
    let command = match operation {
        Ic705CivOperation::ReadFrequency => vec![0x03],
        Ic705CivOperation::ReadMode => vec![0x04],
        Ic705CivOperation::PttRx => vec![0x1C, 0x00, 0x00],
        Ic705CivOperation::PttTx => vec![0x1C, 0x00, 0x01],
    };
    let mut frame = vec![0xFE, 0xFE, radio, controller];
    frame.extend(command);
    frame.push(0xFD);
    let expected_ok = vec![0xFE, 0xFE, controller, radio, 0xFB, 0xFD];

    Ok(Ic705CivFrameReport {
        kind: "ic705-civ-frame",
        model: "IC-705",
        operation: operation.label(),
        frame_hex: bytes_to_hex(&frame),
        frame_bytes: frame,
        radio_address: format!("{radio:02X}"),
        controller_address: format!("{controller:02X}"),
        expected_ok_hex: bytes_to_hex(&expected_ok),
        transmit_risk: matches!(operation, Ic705CivOperation::PttTx),
        note: if matches!(operation, Ic705CivOperation::PttTx) {
            "Dry-run frame only. Sending this over a live CI-V port would request transmit."
                .to_owned()
        } else {
            "Dry-run frame only. No serial port was opened.".to_owned()
        },
    })
}

pub fn run_ic705_civ_serial(config: Ic705CivSerialConfig) -> Result<Ic705CivSerialReport> {
    if config.baud_rate == 0 {
        bail!("CI-V serial baud rate must be greater than zero");
    }
    if config.timeout_ms == 0 {
        bail!("CI-V serial timeout must be greater than zero");
    }
    let frame = build_ic705_civ_frame_report(
        config.operation,
        &config.radio_address,
        &config.controller_address,
    )?;

    if !config.live {
        return Ok(Ic705CivSerialReport {
            kind: "ic705-civ-serial-report",
            model: "IC-705",
            dry_run: true,
            live_requested: false,
            port: config.port,
            baud_rate: config.baud_rate,
            timeout_ms: config.timeout_ms,
            allow_transmit: config.allow_transmit,
            wrote_bytes: 0,
            response_hex: None,
            response_bytes: Vec::new(),
            note: "Dry-run only. No serial port was opened; pass --live to send this frame."
                .to_owned(),
            frame,
        });
    }

    let port_name = config
        .port
        .clone()
        .context("--live CI-V serial requires --port")?;
    if frame.transmit_risk && !config.allow_transmit {
        bail!("live CI-V transmit-risk operation requires --allow-transmit");
    }

    let mut port = serialport::new(&port_name, config.baud_rate)
        .timeout(Duration::from_millis(config.timeout_ms))
        .open()
        .with_context(|| format!("opening CI-V serial port {port_name}"))?;
    port.write_all(&frame.frame_bytes)
        .with_context(|| format!("writing CI-V frame to {port_name}"))?;
    port.flush()
        .with_context(|| format!("flushing CI-V frame to {port_name}"))?;
    let mut response = vec![0_u8; 256];
    let read_count = match port.read(&mut response) {
        Ok(count) => count,
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => 0,
        Err(error) => {
            return Err(error).with_context(|| format!("reading CI-V response from {port_name}"));
        }
    };
    response.truncate(read_count);
    let response_hex = (!response.is_empty()).then(|| bytes_to_hex(&response));

    Ok(Ic705CivSerialReport {
        kind: "ic705-civ-serial-report",
        model: "IC-705",
        dry_run: false,
        live_requested: true,
        port: Some(port_name),
        baud_rate: config.baud_rate,
        timeout_ms: config.timeout_ms,
        allow_transmit: config.allow_transmit,
        wrote_bytes: frame.frame_bytes.len(),
        response_hex,
        response_bytes: response,
        note: "Live CI-V frame was written to the configured serial port.".to_owned(),
        frame,
    })
}

impl Ic705CivOperation {
    fn label(self) -> &'static str {
        match self {
            Self::ReadFrequency => "read-frequency",
            Self::ReadMode => "read-mode",
            Self::PttRx => "ptt-rx",
            Self::PttTx => "ptt-tx",
        }
    }
}

fn parse_hex_byte(value: &str) -> Result<u8> {
    let normalized = value
        .trim()
        .strip_prefix("0x")
        .or_else(|| value.trim().strip_prefix("0X"))
        .unwrap_or_else(|| value.trim());
    if normalized.len() != 2 {
        bail!("expected exactly two hex digits");
    }
    u8::from_str_radix(normalized, 16).context("parsing hex byte")
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_validates() {
        let report = validate_ic705_profile(default_ic705_profile());
        assert!(report.ok);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn profile_toml_roundtrips() {
        let profile = default_ic705_profile();
        let raw = ic705_profile_toml(&profile).unwrap();
        assert!(raw.contains("chattybara-ic705-profile"));
        let parsed: Ic705Profile = toml::from_str(&raw).unwrap();
        assert_eq!(parsed, profile);
    }

    #[test]
    fn builds_read_frequency_civ_frame() {
        let report =
            build_ic705_civ_frame_report(Ic705CivOperation::ReadFrequency, "A4", "E0").unwrap();
        assert_eq!(report.frame_hex, "FE FE A4 E0 03 FD");
        assert_eq!(report.expected_ok_hex, "FE FE E0 A4 FB FD");
        assert!(!report.transmit_risk);
    }

    #[test]
    fn ptt_tx_frame_is_marked_as_transmit_risk() {
        let report = build_ic705_civ_frame_report(Ic705CivOperation::PttTx, "A4", "E0").unwrap();
        assert_eq!(report.frame_hex, "FE FE A4 E0 1C 00 01 FD");
        assert!(report.transmit_risk);
    }

    #[test]
    fn civ_serial_is_dry_run_unless_live_is_requested() {
        let report = run_ic705_civ_serial(Ic705CivSerialConfig {
            operation: Ic705CivOperation::ReadFrequency,
            radio_address: "A4".to_owned(),
            controller_address: "E0".to_owned(),
            port: Some("/dev/not-opened".to_owned()),
            baud_rate: 19200,
            timeout_ms: 500,
            live: false,
            allow_transmit: false,
        })
        .unwrap();

        assert!(report.dry_run);
        assert_eq!(report.wrote_bytes, 0);
        assert_eq!(report.frame.frame_hex, "FE FE A4 E0 03 FD");
        assert_eq!(report.port.as_deref(), Some("/dev/not-opened"));
    }

    #[test]
    fn live_ptt_tx_requires_transmit_opt_in_before_opening_port() {
        let error = run_ic705_civ_serial(Ic705CivSerialConfig {
            operation: Ic705CivOperation::PttTx,
            radio_address: "A4".to_owned(),
            controller_address: "E0".to_owned(),
            port: Some("/dev/not-opened".to_owned()),
            baud_rate: 19200,
            timeout_ms: 500,
            live: true,
            allow_transmit: false,
        })
        .unwrap_err();

        assert!(error.to_string().contains("--allow-transmit"));
    }
}

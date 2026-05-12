use crate::hamlib::DEFAULT_RIGCTLD_HOST;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadioProfile {
    pub kind: String,
    pub model: String,
    pub audio: RadioAudioProfile,
    pub control: RadioControlProfile,
    pub safety: RadioSafetyProfile,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadioAudioProfile {
    pub input_device: String,
    pub output_device: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub rx_gain: f32,
    pub tx_gain: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadioControlProfile {
    pub backend: String,
    pub hamlib_host: String,
    pub ptt_method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioSafetyProfile {
    pub allow_transmit: bool,
    pub require_manual_ptt_confirm: bool,
    pub max_tx_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RadioProfileValidationReport {
    pub kind: &'static str,
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub profile: RadioProfile,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RadioProfileTemplate {
    pub model: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub hamlib_host: String,
}

pub fn default_radio_profile(template: RadioProfileTemplate) -> RadioProfile {
    let model = template.model.trim();
    let model = if model.is_empty() {
        "generic-hamlib-radio"
    } else {
        model
    };
    RadioProfile {
        kind: "chattybara-radio-profile".to_owned(),
        model: model.to_owned(),
        audio: RadioAudioProfile {
            input_device: template
                .input_device
                .unwrap_or_else(|| "default input".to_owned()),
            output_device: template
                .output_device
                .unwrap_or_else(|| "default output".to_owned()),
            sample_rate: template.sample_rate,
            channels: template.channels,
            rx_gain: 0.70,
            tx_gain: 0.20,
        },
        control: RadioControlProfile {
            backend: "hamlib-rigctld".to_owned(),
            hamlib_host: if template.hamlib_host.trim().is_empty() {
                DEFAULT_RIGCTLD_HOST.to_owned()
            } else {
                template.hamlib_host
            },
            ptt_method: "hamlib".to_owned(),
        },
        safety: RadioSafetyProfile {
            allow_transmit: false,
            require_manual_ptt_confirm: true,
            max_tx_seconds: 30,
        },
        notes: vec![
            "Generic Hamlib rigctld profile; run rigctld separately for your radio model."
                .to_owned(),
            "Use `chattybara audio devices --include-supported` to copy exact audio device names."
                .to_owned(),
            "PTT and live audio transmit remain opt-in at command time.".to_owned(),
        ],
    }
}

pub fn radio_profile_toml(profile: &RadioProfile) -> Result<String> {
    toml::to_string_pretty(profile).context("serializing radio profile")
}

pub fn load_radio_profile(path: &Path) -> Result<RadioProfile> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

pub fn validate_radio_profile(profile: RadioProfile) -> RadioProfileValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if profile.kind != "chattybara-radio-profile" {
        errors.push("kind must be chattybara-radio-profile".to_owned());
    }
    if profile.model.trim().is_empty() {
        errors.push("model is required".to_owned());
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
            "audio.tx_gain is above 0.50; verify audio drive and ALC before transmitting"
                .to_owned(),
        );
    }
    if profile.control.backend != "hamlib-rigctld" {
        errors.push("control.backend must be hamlib-rigctld".to_owned());
    }
    if profile.control.hamlib_host.trim().is_empty() {
        errors.push("control.hamlib_host is required".to_owned());
    }
    if profile.control.ptt_method != "hamlib" && profile.control.ptt_method != "none" {
        errors.push("control.ptt_method must be hamlib or none".to_owned());
    }
    if profile.safety.allow_transmit {
        warnings
            .push("safety.allow_transmit is true; validation cannot prove RF safety".to_owned());
    }
    if !profile.safety.require_manual_ptt_confirm {
        warnings.push("safety.require_manual_ptt_confirm is false".to_owned());
    }
    if profile.safety.max_tx_seconds == 0 {
        errors.push("safety.max_tx_seconds must be greater than zero".to_owned());
    }

    RadioProfileValidationReport {
        kind: "radio-profile-validation-report",
        ok: errors.is_empty(),
        errors,
        warnings,
        profile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_radio_profile_validates() {
        let profile = default_radio_profile(RadioProfileTemplate {
            model: "IC-7300".to_owned(),
            input_device: Some("USB Audio CODEC".to_owned()),
            output_device: Some("USB Audio CODEC".to_owned()),
            sample_rate: 48_000,
            channels: 1,
            hamlib_host: DEFAULT_RIGCTLD_HOST.to_owned(),
        });

        let report = validate_radio_profile(profile.clone());
        assert!(report.ok);
        assert_eq!(profile.control.backend, "hamlib-rigctld");
        assert!(radio_profile_toml(&profile).unwrap().contains("IC-7300"));
    }
}

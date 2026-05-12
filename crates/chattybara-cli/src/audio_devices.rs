use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AudioDeviceRequest {
    pub sample_rate: u32,
    pub channels: u16,
    pub include_supported: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AudioDeviceInventory {
    pub kind: &'static str,
    pub host: String,
    pub requested_config: AudioDeviceRequest,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
    pub devices: Vec<AudioDeviceReport>,
    pub device_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AudioDeviceReport {
    pub name: String,
    pub default_input: bool,
    pub default_output: bool,
    pub input: AudioDeviceSideReport,
    pub output: AudioDeviceSideReport,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AudioDeviceSideReport {
    pub available: bool,
    pub default_config: Option<AudioStreamConfigReport>,
    pub supports_requested_config: Option<bool>,
    pub supported_configs: Option<Vec<AudioSupportedConfigReport>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AudioStreamConfigReport {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AudioSupportedConfigReport {
    pub channels: u16,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    pub sample_format: String,
}

pub(crate) fn enumerate_audio_devices(request: AudioDeviceRequest) -> AudioDeviceInventory {
    let host = cpal::default_host();
    let default_input = host
        .default_input_device()
        .and_then(|device| device_name(&device).ok());
    let default_output = host
        .default_output_device()
        .and_then(|device| device_name(&device).ok());

    let mut device_error = None;
    let devices = match host.devices() {
        Ok(devices) => devices
            .map(|device| {
                let name = device_name(&device)
                    .unwrap_or_else(|error| format!("unknown audio device: {error}"));
                let default_input_device = default_input.as_deref() == Some(name.as_str());
                let default_output_device = default_output.as_deref() == Some(name.as_str());
                AudioDeviceReport {
                    input: side_report(&device, AudioDeviceDirection::Input, &request),
                    output: side_report(&device, AudioDeviceDirection::Output, &request),
                    default_input: default_input_device,
                    default_output: default_output_device,
                    name,
                }
            })
            .collect(),
        Err(error) => {
            device_error = Some(error.to_string());
            Vec::new()
        }
    };

    AudioDeviceInventory {
        kind: "audio-device-inventory",
        host: format!("{:?}", host.id()),
        requested_config: request,
        default_input,
        default_output,
        devices,
        device_error,
    }
}

#[derive(Debug, Clone, Copy)]
enum AudioDeviceDirection {
    Input,
    Output,
}

fn side_report(
    device: &cpal::Device,
    direction: AudioDeviceDirection,
    request: &AudioDeviceRequest,
) -> AudioDeviceSideReport {
    let default_config = match direction {
        AudioDeviceDirection::Input => device.default_input_config(),
        AudioDeviceDirection::Output => device.default_output_config(),
    };
    let default_config = match default_config {
        Ok(config) => Some(AudioStreamConfigReport {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            sample_format: format!("{:?}", config.sample_format()),
        }),
        Err(_) => None,
    };

    let supported_configs = match direction {
        AudioDeviceDirection::Input => device
            .supported_input_configs()
            .map(|configs| configs.map(supported_config_report).collect::<Vec<_>>()),
        AudioDeviceDirection::Output => device
            .supported_output_configs()
            .map(|configs| configs.map(supported_config_report).collect::<Vec<_>>()),
    };

    match supported_configs {
        Ok(configs) => {
            let supports_requested_config = configs
                .iter()
                .any(|config| supports_requested_config(config, request));
            AudioDeviceSideReport {
                available: !configs.is_empty(),
                default_config,
                supports_requested_config: Some(supports_requested_config),
                supported_configs: request.include_supported.then_some(configs),
                error: None,
            }
        }
        Err(error) => AudioDeviceSideReport {
            available: default_config.is_some(),
            default_config,
            supports_requested_config: None,
            supported_configs: None,
            error: Some(error.to_string()),
        },
    }
}

fn device_name(device: &cpal::Device) -> Result<String, cpal::DeviceNameError> {
    device
        .description()
        .map(|description| description.name().to_owned())
}

fn supported_config_report(config: cpal::SupportedStreamConfigRange) -> AudioSupportedConfigReport {
    AudioSupportedConfigReport {
        channels: config.channels(),
        min_sample_rate: config.min_sample_rate(),
        max_sample_rate: config.max_sample_rate(),
        sample_format: format!("{:?}", config.sample_format()),
    }
}

fn supports_requested_config(
    config: &AudioSupportedConfigReport,
    request: &AudioDeviceRequest,
) -> bool {
    config.channels == request.channels
        && request.sample_rate >= config.min_sample_rate
        && request.sample_rate <= config.max_sample_rate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_config_matches_supported_range() {
        let config = AudioSupportedConfigReport {
            channels: 1,
            min_sample_rate: 8000,
            max_sample_rate: 48000,
            sample_format: "F32".to_owned(),
        };
        assert!(supports_requested_config(
            &config,
            &AudioDeviceRequest {
                sample_rate: 8000,
                channels: 1,
                include_supported: false,
            }
        ));
        assert!(!supports_requested_config(
            &config,
            &AudioDeviceRequest {
                sample_rate: 96000,
                channels: 1,
                include_supported: false,
            }
        ));
    }
}

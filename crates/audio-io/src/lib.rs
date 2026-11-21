//! Audio hardware discovery. Backend types do not cross this boundary.

mod stream;
pub use stream::{
    CHANNELS, Capture, ClickPreview, ClickSettings, DEFAULT_BPM, Meter, Playback, TEMPO_RANGE,
    TimingChange,
};

use cpal::traits::{DeviceTrait, HostTrait};
use std::{error::Error, fmt};

#[derive(Debug)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub input: bool,
    pub output: bool,
}

#[derive(Debug)]
pub struct DeviceInventory {
    pub backend: &'static str,
    pub devices: Vec<AudioDevice>,
    pub default_output: Option<String>,
    pub unavailable: usize,
}

#[derive(Debug)]
pub struct DiscoveryError(cpal::Error);

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for DiscoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// List the devices reported by the default host without starting a stream.
/// Run this off the UI thread: probing drivers can block.
pub fn discover_devices() -> Result<DeviceInventory, DiscoveryError> {
    let host = cpal::default_host();
    let mut inventory = DeviceInventory {
        backend: host.id().name(),
        devices: Vec::new(),
        default_output: host
            .default_output_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string()),
        unavailable: 0,
    };

    for device in host.devices().map_err(DiscoveryError)? {
        match (device.id(), device.description()) {
            (Ok(id), Ok(description)) => {
                let mut name = description.name().to_owned();
                let alsa = inventory.backend.eq_ignore_ascii_case("alsa");
                if alsa {
                    name = format!("{name} ({})", id.id());
                }
                inventory.devices.push(AudioDevice {
                    id: id.to_string(),
                    name,
                    // ALSA conversion plugins can report four channels for a stereo source.
                    input: description.supports_input() && (!alsa || id.id().starts_with("hw:")),
                    output: description.supports_output(),
                });
            }
            _ => inventory.unavailable += 1,
        }
    }
    inventory.devices.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(inventory)
}

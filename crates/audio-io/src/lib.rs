//! Audio hardware discovery. Backend types do not cross this boundary.

use cpal::traits::{DeviceTrait, HostTrait};
use std::{error::Error, fmt};

#[derive(Debug)]
pub struct AudioDevice {
    pub name: String,
    pub input: bool,
    pub output: bool,
}

#[derive(Debug)]
pub struct DeviceInventory {
    pub backend: &'static str,
    pub devices: Vec<AudioDevice>,
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
        unavailable: 0,
    };

    for device in host.devices().map_err(DiscoveryError)? {
        match device.description() {
            Ok(description) => inventory.devices.push(AudioDevice {
                name: description.name().to_owned(),
                input: description.supports_input(),
                output: description.supports_output(),
            }),
            Err(_) => inventory.unavailable += 1,
        }
    }
    inventory.devices.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(inventory)
}

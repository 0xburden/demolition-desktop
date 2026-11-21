use demolition_audio_io::{DiscoveryError, discover_devices};

fn main() -> Result<(), DiscoveryError> {
    let inventory = discover_devices()?;
    println!("Audio system: {}", inventory.backend);
    for device in inventory.devices {
        println!(
            "{} (input: {}, output: {})",
            device.name, device.input, device.output
        );
    }
    if inventory.unavailable > 0 {
        eprintln!("{} device(s) could not be read", inventory.unavailable);
    }
    Ok(())
}

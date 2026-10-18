//! Read-only look at InputPlumber: version, managed controllers, their mode, profile and the
//! physical pad behind each. Changes nothing.
//!
//! `cargo run -p hearth-platform --example ip_probe`

use hearth_platform::input::inputplumber::client::Client;

fn main() {
    let client = match Client::connect() {
        Ok(client) => client,
        Err(err) => {
            eprintln!("InputPlumber unavailable: {err}");
            std::process::exit(1);
        }
    };
    println!("version: {}", client.version().unwrap_or_default());
    println!("manage all devices: {:?}", client.manage_all());
    let devices = client.composite_devices().unwrap_or_default();
    if devices.is_empty() {
        println!("no composite devices");
    }
    for device in devices {
        println!("{device}");
        println!("  intercept mode: {:?}", client.intercept_mode(&device));
        println!("  profile: {:?}", client.profile_path(&device));
        println!("  dbus targets: {:?}", client.dbus_targets(&device));
        println!("  physical pad: {:?}", client.source_pad(&device));
    }
}

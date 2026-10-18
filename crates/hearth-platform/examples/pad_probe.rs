//! Lists the controllers gilrs sees, with bus and battery, then prints raw events (no deadzone
//! or jitter filtering) for a few seconds. Read-only, like the input backend:
//! `cargo run -p hearth-platform --example pad_probe -- [seconds]`
use std::time::{Duration, Instant};

use gilrs::GilrsBuilder;

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut gilrs = GilrsBuilder::new()
        .with_default_filters(false)
        .build()
        .expect("gilrs");
    for (id, pad) in gilrs.gamepads() {
        let uuid = pad.uuid();
        let bus = u16::from_le_bytes([uuid[0], uuid[1]]);
        println!(
            "{id}: {:?} (os {:?}) vendor {:04x?} product {:04x?} bus 0x{bus:02x} power {:?}",
            pad.name(),
            pad.os_name(),
            pad.vendor_id(),
            pad.product_id(),
            pad.power_info()
        );
    }
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        if let Some(event) = gilrs.next_event_blocking(Some(Duration::from_millis(100))) {
            println!("{:?}", event.event);
        }
    }
}

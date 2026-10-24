//! Connected controllers as Settings → Controllers, the Welcome widget and the top bar show
//! them.

use hearth_core::controls::ControllerKind;
use hearth_platform::input::{Connection, PadInfo, Power};

use crate::PadRow;

/// "DualSense", "Xbox controller"…, else the name the system gives the pad.
pub fn display_name(pad: &PadInfo) -> String {
    let name = match ControllerKind::from_ids(pad.vendor_id, pad.product_id) {
        ControllerKind::DualSense if pad.product_id == Some(0x0df2) => "DualSense Edge",
        ControllerKind::DualSense => "DualSense",
        ControllerKind::DualShock4 => "DualShock 4",
        ControllerKind::Xbox => "Xbox controller",
        ControllerKind::SwitchPro => "Switch Pro Controller",
        ControllerKind::Generic if !pad.name.is_empty() => return pad.name.clone(),
        ControllerKind::Generic => return pad.os_name.clone(),
    };
    name.to_owned()
}

fn connection(pad: &PadInfo) -> &'static str {
    match pad.connection {
        Connection::Usb => "USB",
        Connection::Bluetooth => "Bluetooth",
        Connection::Other => "",
    }
}

/// Battery percent (`None` when not reported or wired) and whether it is charging.
fn battery(pad: &PadInfo) -> (Option<u8>, bool) {
    match pad.power {
        Power::Discharging(p) => (Some(p), false),
        Power::Charging(p) => (Some(p), true),
        Power::Charged => (Some(100), true),
        Power::Wired | Power::Unknown => (None, false),
    }
}

/// The controller icon for a pad (`Icons.controller` in Slint): 0 generic, 1 DualSense,
/// 2 DualShock 4, 3 Xbox, 4 Switch Pro.
pub fn device(pad: Option<&PadInfo>) -> i32 {
    match pad.map(|p| ControllerKind::from_ids(p.vendor_id, p.product_id)) {
        Some(ControllerKind::DualSense) => 1,
        Some(ControllerKind::DualShock4) => 2,
        Some(ControllerKind::Xbox) => 3,
        Some(ControllerKind::SwitchPro) => 4,
        Some(ControllerKind::Generic) | None => 0,
    }
}

/// The row of one controller.
pub fn row(pad: &PadInfo) -> PadRow {
    let (percent, charging) = battery(pad);
    PadRow {
        id: pad.id.as_str().into(),
        name: display_name(pad).into(),
        connection: connection(pad).into(),
        battery: percent.map_or(-1, i32::from),
        charging,
        device: device(Some(pad)),
    }
}

/// What Welcome, the top bar and the Controllers category say about the connected pads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// "1 connected", "None connected".
    pub count: String,
    /// "DualSense · 85 %", "DualSense · Charging 45 %", "Connect a controller".
    pub detail: String,
    /// The first pad's battery for the top bar ("85 %"), or empty.
    pub battery: String,
}

/// While the input backend is still looking for pads at startup.
pub fn checking() -> Summary {
    Summary {
        count: "Checking…".into(),
        detail: "Looking for controllers".into(),
        battery: String::new(),
    }
}

/// Sums up `pads` (the first one is player 1's).
pub fn summary(pads: &[PadInfo]) -> Summary {
    let Some(first) = pads.first() else {
        return Summary {
            count: "None connected".into(),
            detail: "Connect a controller".into(),
            battery: String::new(),
        };
    };
    let state = match battery(first) {
        (Some(p), false) => format!("{p} %"),
        (Some(100), true) => "Fully charged".to_owned(),
        (Some(p), true) => format!("Charging {p} %"),
        (None, _) => connection(first).to_owned(),
    };
    let mut detail = display_name(first);
    if !state.is_empty() {
        detail = format!("{detail} · {state}");
    }
    if pads.len() > 1 {
        detail = format!("{detail} and {} more", pads.len() - 1);
    }
    Summary {
        count: format!("{} connected", pads.len()),
        detail,
        battery: battery(first)
            .0
            .map_or_else(String::new, |p| format!("{p} %")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dualsense(power: Power, connection: Connection) -> PadInfo {
        PadInfo {
            id: "ds".into(),
            name: "PS5 Controller".into(),
            os_name: "Sony Interactive Entertainment DualSense Wireless Controller".into(),
            vendor_id: Some(0x054c),
            product_id: Some(0x0ce6),
            connection,
            power,
        }
    }

    #[test]
    fn names_are_short() {
        let pad = dualsense(Power::Unknown, Connection::Usb);
        assert_eq!(display_name(&pad), "DualSense");
        let other = PadInfo {
            vendor_id: Some(0x1234),
            product_id: None,
            name: String::new(),
            ..pad
        };
        assert_eq!(display_name(&other), other.os_name);
    }

    #[test]
    fn rows_carry_connection_and_battery() {
        let row = row(&dualsense(Power::Discharging(85), Connection::Bluetooth));
        assert_eq!(row.name, "DualSense");
        assert_eq!(row.connection, "Bluetooth");
        assert_eq!((row.battery, row.charging), (85, false));
        let wired = super::row(&dualsense(Power::Wired, Connection::Usb));
        assert_eq!(wired.battery, -1);
    }

    #[test]
    fn summaries() {
        assert_eq!(summary(&[]).count, "None connected");
        let s = summary(&[dualsense(Power::Discharging(85), Connection::Bluetooth)]);
        assert_eq!(
            (s.count.as_str(), s.detail.as_str(), s.battery.as_str()),
            ("1 connected", "DualSense · 85 %", "85 %")
        );
        let s = summary(&[
            dualsense(Power::Charging(45), Connection::Usb),
            dualsense(Power::Unknown, Connection::Usb),
        ]);
        assert_eq!(s.count, "2 connected");
        assert_eq!(s.detail, "DualSense · Charging 45 % and 1 more");
        let s = summary(&[dualsense(Power::Unknown, Connection::Usb)]);
        assert_eq!(
            (s.detail.as_str(), s.battery.as_str()),
            ("DualSense · USB", "")
        );
    }
}

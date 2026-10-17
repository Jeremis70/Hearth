//! Which physical controller player 1 is, and how each emulator names it.
//!
//! This is the one place that knows controller names. The same DualSense is "PS5 Controller"
//! to gilrs (SDL mapping database), "Sony Interactive Entertainment DualSense Wireless
//! Controller" to evdev and RetroArch's autoconfig, and "DualSense Wireless Controller" to SDL3
//! (Dolphin's `SDL/0/...`). With InputPlumber the identity becomes its virtual pad.
//!
//! `<config>/controls/device.toml` can override the detected values:
//! ```toml
//! sdl_name = "Xbox Series X Controller"
//! ```

use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::model::PadInput;

/// Controller family, for button glyphs, display names and the controller type's default
/// bindings (`data/controls/pads/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerKind {
    /// Sony DualSense / DualSense Edge.
    DualSense,
    /// Sony DualShock 4.
    DualShock4,
    /// Microsoft Xbox (360, One, Series).
    Xbox,
    /// Nintendo Switch Pro Controller.
    SwitchPro,
    /// Anything else.
    Generic,
}

impl ControllerKind {
    /// From USB vendor and product ids.
    pub fn from_ids(vendor: Option<u16>, product: Option<u16>) -> Self {
        match (vendor, product) {
            (Some(0x054c), Some(0x0ce6 | 0x0df2)) => Self::DualSense,
            (Some(0x054c), Some(0x05c4 | 0x09cc | 0x0ba0)) => Self::DualShock4,
            (Some(0x045e), _) => Self::Xbox,
            (Some(0x057e), Some(0x2009)) => Self::SwitchPro,
            _ => Self::Generic,
        }
    }
}

/// Player 1's controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerIdentity {
    /// Family.
    pub kind: ControllerKind,
    /// Kernel (evdev) name, as RetroArch's udev driver and autoconfig see it.
    pub os_name: String,
    /// USB vendor id.
    pub vendor_id: Option<u16>,
    /// USB product id.
    pub product_id: Option<u16>,
    /// SDL3's name, as Dolphin's SDL backend sees it.
    pub sdl_name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Overrides {
    sdl_name: Option<String>,
}

impl ControllerIdentity {
    /// From what the input backend reports about a connected pad.
    pub fn from_pad(os_name: &str, vendor_id: Option<u16>, product_id: Option<u16>) -> Self {
        let kind = ControllerKind::from_ids(vendor_id, product_id);
        Self {
            kind,
            os_name: os_name.to_owned(),
            vendor_id,
            product_id,
            sdl_name: sdl_name(kind, vendor_id, product_id, os_name),
        }
    }

    /// Used when no controller is connected at launch: the DualSense this was developed with
    /// (USB), so a pad connected later still matches.
    pub fn fallback() -> Self {
        Self::from_pad(
            "Sony Interactive Entertainment DualSense Wireless Controller",
            Some(0x054c),
            Some(0x0ce6),
        )
    }

    /// Applies `<controls dir>/device.toml`, if present.
    pub fn with_overrides(mut self, controls_dir: &Path) -> Self {
        let path = controls_dir.join("device.toml");
        let Ok(text) = fs::read_to_string(&path) else {
            return self;
        };
        match toml::from_str::<Overrides>(&text) {
            Ok(overrides) => {
                if let Some(name) = overrides.sdl_name {
                    self.sdl_name = name;
                }
            }
            Err(err) => tracing::warn!("{}: {}", path.display(), err.message()),
        }
        self
    }

    /// The extra inputs this controller has (touchpad, share, back paddles), for the
    /// controls page. Only what the controller physically has; whether Hearth or an emulator
    /// can read it is another matter (gilrs can't see the DualSense touchpad).
    pub fn extra_inputs(&self) -> Vec<PadInput> {
        match (self.kind, self.product_id) {
            (ControllerKind::DualSense | ControllerKind::DualShock4, _) => vec![PadInput::Touchpad],
            (ControllerKind::SwitchPro, _) => vec![PadInput::Share],
            // Xbox Series X|S controller (USB, Bluetooth).
            (ControllerKind::Xbox, Some(0x0b12 | 0x0b13)) => vec![PadInput::Share],
            // Xbox Elite Series 2 (USB, Bluetooth).
            (ControllerKind::Xbox, Some(0x0b00 | 0x0b05 | 0x0b22)) => vec![
                PadInput::Paddle1,
                PadInput::Paddle2,
                PadInput::Paddle3,
                PadInput::Paddle4,
            ],
            _ => Vec::new(),
        }
    }

    /// Dolphin's device string for this controller.
    pub fn dolphin_device(&self) -> String {
        format!("SDL/0/{}", self.sdl_name)
    }
}

/// SDL3's gamepad name. Known controllers get SDL's HIDAPI name; others keep the kernel name
/// without a leading vendor (Bluetooth pads already have it that way). DualSense is verified
/// (Dolphin 2609 log); the others follow SDL3's HIDAPI drivers and can be overridden.
fn sdl_name(
    kind: ControllerKind,
    vendor: Option<u16>,
    product: Option<u16>,
    os_name: &str,
) -> String {
    match (kind, product) {
        (ControllerKind::DualSense, Some(0x0df2)) => "DualSense Edge Wireless Controller".into(),
        (ControllerKind::DualSense, _) => "DualSense Wireless Controller".into(),
        (ControllerKind::DualShock4, _) => "PS4 Controller".into(),
        (ControllerKind::SwitchPro, _) => "Nintendo Switch Pro Controller".into(),
        _ => {
            let vendor_prefix = match vendor {
                Some(0x054c) => Some("Sony Interactive Entertainment "),
                Some(0x045e) => Some("Microsoft "),
                _ => None,
            };
            vendor_prefix
                .and_then(|p| os_name.strip_prefix(p))
                .unwrap_or(os_name)
                .to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    #[test]
    fn dualsense_over_usb_and_bluetooth() {
        for os_name in [
            "Sony Interactive Entertainment DualSense Wireless Controller",
            "DualSense Wireless Controller",
        ] {
            let id = ControllerIdentity::from_pad(os_name, Some(0x054c), Some(0x0ce6));
            assert_eq!(id.kind, ControllerKind::DualSense);
            assert_eq!(id.dolphin_device(), "SDL/0/DualSense Wireless Controller");
        }
    }

    #[test]
    fn extras_by_controller() {
        assert_eq!(
            ControllerIdentity::fallback().extra_inputs(),
            [PadInput::Touchpad]
        );
        let series =
            ControllerIdentity::from_pad("Xbox Wireless Controller", Some(0x045e), Some(0x0b13));
        assert_eq!(series.extra_inputs(), [PadInput::Share]);
        let one = ControllerIdentity::from_pad("Xbox One Controller", Some(0x045e), Some(0x02ea));
        assert!(one.extra_inputs().is_empty());
        let elite = ControllerIdentity::from_pad("Xbox Elite", Some(0x045e), Some(0x0b00));
        assert_eq!(elite.extra_inputs().len(), 4);
    }

    #[test]
    fn unknown_pad_keeps_its_kernel_name() {
        let id = ControllerIdentity::from_pad("8BitDo Pro 2", Some(0x2dc8), Some(0x6003));
        assert_eq!(id.kind, ControllerKind::Generic);
        assert_eq!(id.sdl_name, "8BitDo Pro 2");
    }

    #[test]
    fn override_file_wins() {
        let dir = tempdir();
        fs::write(dir.path().join("device.toml"), "sdl_name = \"My Pad\"\n").unwrap();
        let id = ControllerIdentity::fallback().with_overrides(dir.path());
        assert_eq!(id.dolphin_device(), "SDL/0/My Pad");
    }
}

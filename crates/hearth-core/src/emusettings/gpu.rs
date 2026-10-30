//! Which GPU emulators will run on, from sysfs (read-only), to pick a settings preset.

use std::fs;
use std::path::Path;

/// Where the kernel lists display devices.
pub const SYSFS_DRM: &str = "/sys/class/drm";

/// A GPU's PCI ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gpu {
    pub vendor: u16,
    pub device: u16,
}

impl std::fmt::Display for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vendor, self.device)
    }
}

/// The GPU games run on: of the display-class devices under `drm` (`card<N>/device`), the one
/// the firmware booted with (`boot_vga`, the integrated one on hybrid laptops: emulators use it
/// unless offloaded), else the first.
pub fn detect(drm: &Path) -> Option<Gpu> {
    let mut cards: Vec<_> = fs::read_dir(drm)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card") && !n.contains('-'))
        })
        .collect();
    cards.sort();
    let mut gpus = cards.iter().filter_map(|card| {
        let dev = card.join("device");
        let read = |name: &str| fs::read_to_string(dev.join(name)).ok();
        if !read("class")?.trim().starts_with("0x03") {
            return None;
        }
        let hex =
            |name: &str| u16::from_str_radix(read(name)?.trim().trim_start_matches("0x"), 16).ok();
        let gpu = Gpu {
            vendor: hex("vendor")?,
            device: hex("device")?,
        };
        let boot = read("boot_vga").is_some_and(|v| v.trim() == "1");
        Some((boot, gpu))
    });
    let first = gpus.next()?;
    if first.0 {
        return Some(first.1);
    }
    Some(gpus.find(|(boot, _)| *boot).unwrap_or(first).1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    fn card(drm: &Path, name: &str, class: &str, vendor: &str, device: &str, boot: Option<&str>) {
        let dev = drm.join(name).join("device");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("class"), format!("{class}\n")).unwrap();
        fs::write(dev.join("vendor"), format!("{vendor}\n")).unwrap();
        fs::write(dev.join("device"), format!("{device}\n")).unwrap();
        if let Some(boot) = boot {
            fs::write(dev.join("boot_vga"), boot).unwrap();
        }
    }

    #[test]
    fn picks_the_boot_gpu() {
        let dir = tempdir();
        let drm = dir.path();
        assert_eq!(detect(&drm.join("missing")), None);
        card(drm, "card0", "0x030200", "0x10de", "0x2684", Some("0"));
        card(drm, "card1", "0x030000", "0x8086", "0x9a49", Some("1"));
        // A connector, not a card.
        fs::create_dir_all(drm.join("card1-eDP-1")).unwrap();
        assert_eq!(
            detect(drm),
            Some(Gpu {
                vendor: 0x8086,
                device: 0x9a49
            })
        );
    }

    #[test]
    fn falls_back_to_the_first_display_device() {
        let dir = tempdir();
        let drm = dir.path();
        card(drm, "card0", "0x058000", "0x1af4", "0x1050", None); // not a display device
        card(drm, "card1", "0x030000", "0x1002", "0x15bf", None);
        card(drm, "card2", "0x030000", "0x10de", "0x2684", None);
        assert_eq!(detect(drm).unwrap().to_string(), "1002:15bf");
    }
}

//! Per-game controls keys: stable across rescans, renames and moves.
//!
//! The key is the game's serial from its ROM header (`gc/GZ2E01`, `n64/CZLE`), so the same dump
//! keeps its controls wherever it lives. Without a readable serial, the cleaned title is used
//! (`gc/title-the-legend-of-zelda`).

use std::io;
use std::path::{Path, PathBuf};

use crate::model::SystemId;
use crate::rom_header;

/// Which per-game controls files apply to a game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameKey {
    system: SystemId,
    serial: Option<String>,
    title_slug: String,
}

impl GameKey {
    /// Reads the serial from `rom` (a few bytes) for `system`; `title` is the fallback.
    pub fn for_rom(system: &SystemId, rom: &Path, title: &str) -> Self {
        let serial = match read_serial(system, rom) {
            Ok(serial) => serial,
            Err(err) => {
                tracing::debug!(rom = %rom.display(), "can't read the serial: {err}");
                None
            }
        };
        Self {
            system: system.clone(),
            serial,
            title_slug: slug(title),
        }
    }

    /// The key, e.g. `gc/GZ2E01` or `gc/title-pikmin`.
    pub fn as_string(&self) -> String {
        match &self.serial {
            Some(serial) => format!("{}/{serial}", self.system),
            None => format!("{}/title-{}", self.system, self.title_slug),
        }
    }

    /// The serial, if one was read.
    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    /// Per-game files to look for, relative to the `games` folder, most specific first:
    /// the full serial, then the serial without region (and maker) for any-region overrides.
    pub fn candidates(&self) -> Vec<PathBuf> {
        let dir = PathBuf::from(self.system.as_str());
        match &self.serial {
            Some(serial) => {
                let mut files = vec![dir.join(format!("{serial}.toml"))];
                if let Some(base) = region_free(&self.system, serial) {
                    files.push(dir.join(format!("{base}.toml")));
                }
                files
            }
            None => vec![dir.join(format!("title-{}.toml", self.title_slug))],
        }
    }
}

/// `GZ2E01` → `GZ2`, `CZLE` → `CZL`.
fn region_free(system: &SystemId, serial: &str) -> Option<String> {
    let len = match system.as_str() {
        "gc" | "wii" if serial.len() == 6 => 3,
        "n64" if serial.len() == 4 => 3,
        _ => return None,
    };
    Some(serial[..len].to_owned())
}

fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

fn read_serial(system: &SystemId, rom: &Path) -> io::Result<Option<String>> {
    Ok(rom_header::read(system, rom)?.serial)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom_header::tests::n64_rom;
    use crate::testdir::tempdir;

    #[test]
    fn n64_code_in_every_byte_order() {
        let dir = tempdir();
        for order in ["z64", "v64", "n64"] {
            let path = dir.path().join(format!("oot.{order}"));
            std::fs::write(&path, n64_rom(order, b"CZLE", "ZELDA")).unwrap();
            let key = GameKey::for_rom(&SystemId::new("n64"), &path, "OoT");
            assert_eq!(key.as_string(), "n64/CZLE", "{order}");
        }
    }

    #[test]
    fn disc_ids_from_iso_and_rvz() {
        let dir = tempdir();
        let iso = dir.path().join("tp.iso");
        let mut data = b"GZ2E01".to_vec();
        data.resize(0x100, 0);
        std::fs::write(&iso, &data).unwrap();
        let key = GameKey::for_rom(&SystemId::new("gc"), &iso, "TP");
        assert_eq!(key.as_string(), "gc/GZ2E01");
        assert_eq!(
            key.candidates(),
            [
                PathBuf::from("gc/GZ2E01.toml"),
                PathBuf::from("gc/GZ2.toml")
            ]
        );

        let rvz = dir.path().join("tp.rvz");
        let mut data = b"RVZ\x01".to_vec();
        data.resize(0x58, 0);
        data.extend_from_slice(b"GZ2P01");
        std::fs::write(&rvz, &data).unwrap();
        assert_eq!(
            GameKey::for_rom(&SystemId::new("gc"), &rvz, "TP").as_string(),
            "gc/GZ2P01"
        );
    }

    #[test]
    fn unreadable_serial_falls_back_to_title() {
        let dir = tempdir();
        let bad = dir.path().join("bad.gcz");
        std::fs::write(&bad, [0xB1, 0x0B, 0xC0, 0x01, 0, 0, 0, 0, 0, 0]).unwrap();
        let key = GameKey::for_rom(
            &SystemId::new("gc"),
            &bad,
            "The Legend of Zelda: Wind Waker",
        );
        assert_eq!(key.as_string(), "gc/title-the-legend-of-zelda-wind-waker");
        assert_eq!(
            key.candidates(),
            [PathBuf::from(
                "gc/title-the-legend-of-zelda-wind-waker.toml"
            )]
        );
        let missing = GameKey::for_rom(&SystemId::new("n64"), &dir.path().join("nope"), "X");
        assert_eq!(missing.serial(), None);
    }
}

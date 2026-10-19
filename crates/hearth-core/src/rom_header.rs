//! What a ROM's header says about the game: its serial and its internal name.
//!
//! Only a few bytes are read. Used for the per-game controls key and to identify games for
//! artwork.
//!
//! - GameCube/Wii: the disc header holds the 6-character game id at 0x00 and the internal
//!   name (up to 0x60 bytes, NUL-padded) at 0x20. Containers keep a copy of that header at a
//!   fixed offset: RVZ/WIA at 0x58, CISO at 0x8000, WBFS at 0x200; plain ISO/GCM (and NKit)
//!   at 0.
//! - N64: the internal name is 20 bytes at 0x20 and the game code 4 bytes at 0x3B, in one of
//!   three byte orders (`.z64` big-endian, `.v64` 16-bit swapped, `.n64` 32-bit swapped).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::model::SystemId;

/// Header fields that could be read. Both are `None` for systems without a known header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RomHeader {
    /// Game serial (`GZ2E01`, `CZLE`): ASCII letters and digits only.
    pub serial: Option<String>,
    /// Internal name, trimmed (`THE LEGEND OF ZELDA`). Only kept when it is printable ASCII:
    /// Japanese discs use Shift-JIS, which is no use for searching.
    pub internal_name: Option<String>,
}

/// Reads the header of `rom` for `system`.
pub fn read(system: &SystemId, rom: &Path) -> io::Result<RomHeader> {
    match system.as_str() {
        "gc" | "wii" => disc_header(rom),
        "n64" => n64_header(rom),
        _ => Ok(RomHeader::default()),
    }
}

fn read_at(file: &mut File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

fn disc_header(rom: &Path) -> io::Result<RomHeader> {
    let mut file = File::open(rom)?;
    let mut magic = [0u8; 4];
    read_at(&mut file, 0, &mut magic)?;
    let offset = match &magic {
        b"RVZ\x01" | b"WIA\x01" => 0x58,
        b"CISO" => 0x8000,
        b"WBFS" => 0x200,
        _ => 0,
    };
    let mut id = [0u8; 6];
    read_at(&mut file, offset, &mut id)?;
    // The name is optional: a truncated test file or odd dump still has its id.
    let mut name = [0u8; 0x60];
    let internal_name = read_at(&mut file, offset + 0x20, &mut name)
        .ok()
        .and_then(|()| text(&name));
    Ok(RomHeader {
        serial: valid_serial(&id),
        internal_name,
    })
}

fn n64_header(rom: &Path) -> io::Result<RomHeader> {
    let mut header = [0u8; 0x40];
    read_at(&mut File::open(rom)?, 0, &mut header)?;
    match header[..4] {
        [0x80, 0x37, 0x12, 0x40] => {}
        // .v64: 16-bit words byte-swapped.
        [0x37, 0x80, 0x40, 0x12] => header
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .for_each(|w| w.swap(0, 1)),
        // .n64: 32-bit words little-endian.
        [0x40, 0x12, 0x37, 0x80] => header
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|w| w.reverse()),
        _ => return Ok(RomHeader::default()),
    }
    Ok(RomHeader {
        serial: valid_serial(&header[0x3B..0x3F]),
        internal_name: text(&header[0x20..0x34]),
    })
}

fn valid_serial(bytes: &[u8]) -> Option<String> {
    bytes
        .iter()
        .all(u8::is_ascii_alphanumeric)
        .then(|| String::from_utf8_lossy(bytes).into_owned())
}

/// NUL- or space-padded ASCII text; `None` if empty or not printable ASCII.
fn text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let bytes = &bytes[..end];
    if !bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return None;
    }
    let name = String::from_utf8_lossy(bytes)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testdir::tempdir;

    /// An N64 header with `code` and `name`, in byte order `order` (z64, v64 or n64).
    pub(crate) fn n64_rom(order: &str, code: &[u8; 4], name: &str) -> Vec<u8> {
        let mut rom = vec![0u8; 0x40];
        rom[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        let mut padded = [b' '; 20];
        padded[..name.len()].copy_from_slice(name.as_bytes());
        rom[0x20..0x34].copy_from_slice(&padded);
        rom[0x3B..0x3F].copy_from_slice(code);
        match order {
            "v64" => rom
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .for_each(|w| w.swap(0, 1)),
            "n64" => rom
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .for_each(|w| w.reverse()),
            _ => {}
        }
        rom
    }

    /// A disc image: `prefix` (container header, possibly empty), then a disc header with
    /// `id` and `name`.
    pub(crate) fn disc(prefix: &[u8], offset: usize, id: &[u8; 6], name: &str) -> Vec<u8> {
        let mut data = prefix.to_vec();
        data.resize(offset, 0);
        let mut header = vec![0u8; 0x80];
        header[..6].copy_from_slice(id);
        header[0x20..0x20 + name.len()].copy_from_slice(name.as_bytes());
        data.extend_from_slice(&header);
        data
    }

    #[test]
    fn n64_name_and_code_in_every_byte_order() {
        let dir = tempdir();
        for order in ["z64", "v64", "n64"] {
            let path = dir.path().join(format!("oot.{order}"));
            std::fs::write(&path, n64_rom(order, b"CZLE", "THE LEGEND OF ZELDA")).unwrap();
            let header = read(&SystemId::new("n64"), &path).unwrap();
            assert_eq!(header.serial.as_deref(), Some("CZLE"), "{order}");
            assert_eq!(
                header.internal_name.as_deref(),
                Some("THE LEGEND OF ZELDA"),
                "{order}"
            );
        }
    }

    #[test]
    fn disc_id_and_name_from_every_container() {
        let dir = tempdir();
        let name = "The Legend of Zelda Twilight Princess";
        for (file, prefix, offset) in [
            ("tp.iso", &b""[..], 0),
            ("tp.rvz", b"RVZ\x01", 0x58),
            ("tp.wia", b"WIA\x01", 0x58),
            ("tp.ciso", b"CISO", 0x8000),
            ("tp.wbfs", b"WBFS", 0x200),
        ] {
            let path = dir.path().join(file);
            std::fs::write(&path, disc(prefix, offset, b"GZ2E01", name)).unwrap();
            let header = read(&SystemId::new("gc"), &path).unwrap();
            assert_eq!(header.serial.as_deref(), Some("GZ2E01"), "{file}");
            assert_eq!(header.internal_name.as_deref(), Some(name), "{file}");
        }
    }

    #[test]
    fn non_ascii_names_and_unknown_systems_give_nothing() {
        let dir = tempdir();
        let path = dir.path().join("j.iso");
        // Shift-JIS bytes in the name.
        let mut data = disc(b"", 0, b"GZ2J01", "");
        data[0x20..0x24].copy_from_slice(&[0x83, 0x5B, 0x83, 0x8B]);
        std::fs::write(&path, data).unwrap();
        let header = read(&SystemId::new("gc"), &path).unwrap();
        assert_eq!(header.serial.as_deref(), Some("GZ2J01"));
        assert_eq!(header.internal_name, None);
        assert_eq!(
            read(&SystemId::new("snes"), &path).unwrap(),
            RomHeader::default()
        );
    }

    #[test]
    fn short_disc_keeps_its_id() {
        let dir = tempdir();
        let path = dir.path().join("short.iso");
        std::fs::write(&path, b"GZ2E01").unwrap();
        let header = read(&SystemId::new("gc"), &path).unwrap();
        assert_eq!(header.serial.as_deref(), Some("GZ2E01"));
        assert_eq!(header.internal_name, None);
    }
}

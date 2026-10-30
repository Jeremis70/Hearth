//! The one-time copy of an emulator's config files before Hearth first merges into them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::store::Applied;
use crate::users::write_atomic;

/// Copies the files of `rels` (relative to `dir`) that exist into
/// `<backup_root>/<stamp>-emulator-settings/<label>/`, with `SHA256SUMS` (the `sha256sum -c`
/// format, paths relative to the stamp folder), once: `applied.backed_up` records it. Each copy
/// is read back and compared with its original. Returns the folder, if anything was copied.
pub fn back_up_once(
    applied: &mut Applied,
    backup_root: &Path,
    label: &str,
    dir: &Path,
    rels: &[String],
) -> io::Result<Option<PathBuf>> {
    if applied.backed_up {
        return Ok(None);
    }
    let files: Vec<&String> = rels.iter().filter(|rel| dir.join(rel).is_file()).collect();
    if files.is_empty() {
        applied.backed_up = true;
        return Ok(None);
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut root = backup_root.join(format!("{stamp}-emulator-settings"));
    let mut n = 1;
    while root.join(label).exists() {
        n += 1;
        root = backup_root.join(format!("{stamp}-{n}-emulator-settings"));
    }
    let mut sums = fs::read_to_string(root.join("SHA256SUMS")).unwrap_or_default();
    for rel in files {
        let from = dir.join(rel);
        let name = Path::new(label).join(rel);
        let to = root.join(&name);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        let original = fs::read(&from)?;
        fs::write(&to, &original)?;
        if fs::read(&to)? != original {
            return Err(io::Error::other(format!(
                "backup copy of {} differs",
                from.display()
            )));
        }
        let hash = Sha256::digest(&original);
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        sums.push_str(&format!("{hex}  {}\n", name.display()));
    }
    write_atomic(&root.join("SHA256SUMS"), sums.as_bytes())?;
    applied.backed_up = true;
    tracing::info!(backup = %root.display(), label, "emulator config files backed up");
    Ok(Some(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    #[test]
    fn copies_once_with_checksums() {
        let dir = tempdir();
        let emu = dir.path().join("emu");
        fs::create_dir_all(emu.join("Config")).unwrap();
        fs::write(emu.join("Config/GFX.ini"), "[Settings]\n").unwrap();
        let rels = [
            "Config/GFX.ini".to_string(),
            "Config/Missing.ini".to_string(),
        ];
        let mut applied = Applied::default();
        let backup = dir.path().join("backup");
        let out = back_up_once(&mut applied, &backup, "default/dolphin", &emu, &rels)
            .unwrap()
            .unwrap();
        assert!(applied.backed_up);
        assert_eq!(
            fs::read_to_string(out.join("default/dolphin/Config/GFX.ini")).unwrap(),
            "[Settings]\n"
        );
        let sums = fs::read_to_string(out.join("SHA256SUMS")).unwrap();
        assert!(
            sums.ends_with("  default/dolphin/Config/GFX.ini\n"),
            "{sums}"
        );
        assert_eq!(sums.lines().count(), 1);
        // Once only.
        assert!(
            back_up_once(&mut applied, &backup, "default/dolphin", &emu, &rels)
                .unwrap()
                .is_none()
        );
    }
}

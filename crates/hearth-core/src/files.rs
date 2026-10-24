//! The file list behind "Use a file…" and "Choose a file…": images or audio files from a
//! fixed set of folders (Downloads, the game's own folder), newest first.
//!
//! Read-only: folders are listed, never changed (a chosen file is copied, see
//! [`crate::custom`]). Only regular files directly inside each folder count; links are
//! skipped, so nothing outside the folders is reached.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::extras::{IMAGE_EXTENSIONS, THEME_EXTENSIONS};

/// Most files listed.
pub const MAX_FILES: usize = 200;

/// What to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Images (art).
    Image,
    /// Audio (themes).
    Audio,
}

impl FileKind {
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Image => &IMAGE_EXTENSIONS,
            Self::Audio => &THEME_EXTENSIONS,
        }
    }

    /// True for a file name with one of this kind's extensions.
    pub fn matches(self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| self.extensions().contains(&e.to_ascii_lowercase().as_str()))
    }
}

/// A folder to list, and how to call it ("Downloads", "Game folder").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Shown beside each file.
    pub label: String,
    /// The folder.
    pub dir: PathBuf,
}

/// One file found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedFile {
    /// The file.
    pub path: PathBuf,
    /// Its name.
    pub name: String,
    /// Where it is ([`Place::label`]).
    pub place: String,
    /// Last modified.
    pub modified: SystemTime,
    /// Size in bytes.
    pub size: u64,
}

/// `kind` files directly inside each place, newest first (at most [`MAX_FILES`]). Missing or
/// unreadable folders are skipped.
pub fn list(places: &[Place], kind: FileKind) -> Vec<ListedFile> {
    let mut found = Vec::new();
    for place in places {
        let entries = match std::fs::read_dir(&place.dir) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::debug!(dir = %place.dir.display(), "can't list: {err}");
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !kind.matches(&path) {
                continue;
            }
            // `DirEntry::metadata` doesn't follow links: a link is not a regular file.
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            found.push(ListedFile {
                path,
                name,
                place: place.label.clone(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                size: meta.len(),
            });
        }
    }
    found.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    found.truncate(MAX_FILES);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    #[test]
    fn newest_first_matching_files_only_no_links() {
        let dir = tempdir();
        let downloads = dir.path().join("Downloads");
        let game = dir.path().join("game");
        std::fs::create_dir_all(downloads.join("sub")).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        let old = downloads.join("old.PNG");
        std::fs::write(&old, b"x").unwrap();
        let long_ago = SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        std::fs::write(downloads.join("new.webp"), b"x").unwrap();
        std::fs::write(downloads.join("notes.txt"), b"x").unwrap();
        std::fs::write(downloads.join(".hidden.png"), b"x").unwrap();
        std::fs::write(downloads.join("sub/deep.png"), b"x").unwrap();
        std::fs::write(game.join("song.mp3"), b"x").unwrap();
        std::fs::write(game.join("hero.jpeg"), b"x").unwrap();
        std::os::unix::fs::symlink(downloads.join("sub/deep.png"), downloads.join("link.png"))
            .unwrap();
        let places = [
            Place {
                label: "Downloads".into(),
                dir: downloads,
            },
            Place {
                label: "Game folder".into(),
                dir: game,
            },
            Place {
                label: "Gone".into(),
                dir: dir.path().join("missing"),
            },
        ];
        let images: Vec<(String, String)> = list(&places, FileKind::Image)
            .into_iter()
            .map(|f| (f.name, f.place))
            .collect();
        assert_eq!(images.len(), 3, "{images:?}");
        assert_eq!(images[2], ("old.PNG".into(), "Downloads".into()));
        assert!(images.contains(&("hero.jpeg".into(), "Game folder".into())));
        let audio = list(&places, FileKind::Audio);
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].name, "song.mp3");
    }
}

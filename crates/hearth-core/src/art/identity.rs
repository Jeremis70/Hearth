//! Which game a ROM is, for artwork: a stable cache key and the names to search for.
//!
//! The ROM header comes first (GameCube: game id and internal name; N64: game code and
//! internal name). The file name, cleaned of No-Intro/Redump tags, is the fallback name and,
//! without a serial, the source of the key.

use std::fmt;
use std::path::Path;

use crate::model::SystemId;
use crate::rom_header::{self, RomHeader};
use crate::title::clean_title;

/// Where a game's art lives in the cache and in the `art` table: `gc/GZ2E01` with a header
/// serial, else `n64/name-<hash of the cleaned file name>`. Only letters, digits, `-` and one
/// `/`, so it is also a safe relative path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtKey(String);

impl ArtKey {
    /// The key for a game with this header serial, or (without one) this cleaned title.
    pub fn new(system: &SystemId, serial: Option<&str>, title: &str) -> Self {
        let system = sanitize(system.as_str());
        match serial {
            Some(serial) => Self(format!("{system}/{}", sanitize(serial))),
            None => Self(format!(
                "{system}/name-{:016x}",
                fnv1a64(title.to_lowercase().as_bytes())
            )),
        }
    }

    /// Wraps a key read back from the database.
    pub fn from_stored(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Keeps a key component path-safe whatever a header or system id holds.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// FNV-1a, 64 bits: stable across runs and Rust versions (unlike std's hasher).
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A game, identified for the art pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Cache key.
    pub key: ArtKey,
    /// Names to search for, best first (never empty).
    pub search_names: Vec<String>,
}

/// Identifies the ROM at `rom` of `system` from its header, falling back to its file name.
pub fn identify(system: &SystemId, rom: &Path) -> Identity {
    let header = rom_header::read(system, rom).unwrap_or_else(|err| {
        tracing::debug!(rom = %rom.display(), "can't read the header: {err}");
        RomHeader::default()
    });
    let stem = rom
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    identity_from(system, &header, &clean_title(&stem))
}

fn identity_from(system: &SystemId, header: &RomHeader, file_title: &str) -> Identity {
    Identity {
        key: ArtKey::new(system, header.serial.as_deref(), file_title),
        search_names: search_names(header.internal_name.as_deref(), file_title),
    }
}

/// The header name first, then the file name; duplicates dropped. A name that is the start
/// of another is a truncation (N64 names stop at 20 characters: `THE LEGEND OF ZELDA` for
/// Ocarina of Time) and would match the wrong game exactly, so it gives way to the longer one.
fn search_names(header_name: Option<&str>, file_title: &str) -> Vec<String> {
    let candidates: Vec<&str> = header_name
        .into_iter()
        .chain([file_title])
        .filter(|n| !tokens(n).is_empty())
        .collect();
    let mut names: Vec<String> = Vec::new();
    for (i, name) in candidates.iter().enumerate() {
        let own = tokens(name);
        let truncation_of_other = candidates
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && is_strict_prefix(&own, &tokens(other)));
        let duplicate = names.iter().any(|n| tokens(n) == own);
        if !truncation_of_other && !duplicate {
            names.push((*name).to_owned());
        }
    }
    if names.is_empty() {
        names.push(file_title.to_owned());
    }
    names
}

fn is_strict_prefix(short: &[String], long: &[String]) -> bool {
    short.len() < long.len() && long.starts_with(short)
}

/// Lowercase words: `&` reads as "and", anything else not alphanumeric separates words.
pub fn tokens(name: &str) -> Vec<String> {
    name.to_lowercase()
        .replace('&', " and ")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Minimum similarity for a search result to count as the game.
pub const MATCH_THRESHOLD: f64 = 0.75;

/// How alike two names are, 0 to 1: 1 for the same words in the same order, else the share
/// of words they have in common (Dice coefficient on word sets).
pub fn similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (tokens(a), tokens(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let mut a_set = a;
    a_set.sort();
    a_set.dedup();
    let mut b_set = b;
    b_set.sort();
    b_set.dedup();
    let common = a_set.iter().filter(|w| b_set.contains(w)).count();
    // Never 1.0 for a reordering: an exact match must win over it.
    let dice = 2.0 * common as f64 / (a_set.len() + b_set.len()) as f64;
    dice.min(0.99)
}

/// Picks the result that is `query`: the most similar name at or above [`MATCH_THRESHOLD`];
/// on a tie, the earlier result (the provider's own relevance order).
pub fn pick_match<'a, T>(
    results: &'a [T],
    query: &str,
    name: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    let mut best: Option<(&T, f64)> = None;
    for result in results {
        let score = similarity(query, name(result));
        if score >= MATCH_THRESHOLD && best.is_none_or(|(_, s)| score > s) {
            best = Some((result, score));
        }
    }
    best.map(|(result, _)| result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom_header::tests::{disc, n64_rom};
    use crate::testdir::tempdir;

    #[test]
    fn keys_from_serial_or_title_hash() {
        let gc = SystemId::new("gc");
        assert_eq!(ArtKey::new(&gc, Some("GZ2E01"), "x").as_str(), "gc/GZ2E01");
        let a = ArtKey::new(&gc, None, "Pikmin");
        assert_eq!(a, ArtKey::new(&gc, None, "PIKMIN"), "case-insensitive");
        assert!(a.as_str().starts_with("gc/name-"), "{a}");
        assert_eq!(a.as_str().len(), "gc/name-".len() + 16);
        assert_ne!(a, ArtKey::new(&gc, None, "Pikmin 2"));
        // Pinned: the key names cache folders, so it must never change.
        assert_eq!(a.as_str(), "gc/name-49f7cc5f087d65ad");
        assert_eq!(
            ArtKey::new(&SystemId::new("../x"), Some("A/B"), "").as_str(),
            "---x/A-B"
        );
    }

    #[test]
    fn identify_prefers_header_names_over_truncations() {
        let dir = tempdir();
        let n64 = SystemId::new("n64");
        let oot = dir
            .path()
            .join("Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!].z64");
        std::fs::write(&oot, n64_rom("z64", b"CZLE", "THE LEGEND OF ZELDA")).unwrap();
        let id = identify(&n64, &oot);
        assert_eq!(id.key.as_str(), "n64/CZLE");
        assert_eq!(id.search_names, ["The Legend of Zelda: Ocarina of Time"]);

        let mk = dir.path().join("Mario Kart 64 (USA).z64");
        std::fs::write(&mk, n64_rom("v64", b"NKTE", "MARIOKART64")).unwrap();
        assert_eq!(
            identify(&n64, &mk).search_names,
            ["MARIOKART64", "Mario Kart 64"]
        );
    }

    #[test]
    fn identify_gamecube_header_name_and_filename_fallback() {
        let dir = tempdir();
        let gc = SystemId::new("gc");
        let tp = dir
            .path()
            .join("Legend of Zelda, The - Twilight Princess (USA).nkit.iso");
        std::fs::write(
            &tp,
            disc(b"", 0, b"GZ2E01", "The Legend of Zelda Twilight Princess"),
        )
        .unwrap();
        let id = identify(&gc, &tp);
        assert_eq!(id.key.as_str(), "gc/GZ2E01");
        // Same words as the file name: one search.
        assert_eq!(id.search_names, ["The Legend of Zelda Twilight Princess"]);

        let bad = dir.path().join("Pikmin (USA) [!].gcz");
        std::fs::write(&bad, [0xB1, 0x0B, 0xC0, 0x01, 0, 0, 0, 0, 0, 0]).unwrap();
        let id = identify(&gc, &bad);
        assert_eq!(id.key, ArtKey::new(&gc, None, "Pikmin"));
        assert_eq!(id.search_names, ["Pikmin"]);

        let missing = identify(&gc, &dir.path().join("Metroid Prime (USA) (Rev 1).iso"));
        assert_eq!(missing.search_names, ["Metroid Prime"]);
    }

    #[test]
    fn filename_tags_are_stripped_for_search() {
        for (stem, title) in [
            ("Super Mario 64 (USA)", "Super Mario 64"),
            ("Mario Kart 64 (Europe) (Rev 1) [!]", "Mario Kart 64"),
            ("Paper Mario (USA) (Rev A) [b1]", "Paper Mario"),
            ("Pikmin (Europe) (En,Fr,De,Es,It)", "Pikmin"),
        ] {
            assert_eq!(search_names(None, &clean_title(stem)), [title]);
        }
    }

    #[derive(Debug, PartialEq)]
    struct Hit(&'static str);

    #[test]
    fn exact_match_beats_order_and_near_misses() {
        let results = [
            Hit("The Legend of Zelda"),
            Hit("The Legend of Zelda: Ocarina of Time 3D"),
            Hit("The Legend of Zelda: Ocarina of Time"),
        ];
        assert_eq!(
            pick_match(&results, "The Legend of Zelda: Ocarina of Time", |h| h.0),
            Some(&results[2])
        );
    }

    #[test]
    fn best_close_match_or_nothing() {
        let results = [
            Hit("Zelda II: The Adventure of Link"),
            Hit("The Legend of Zelda: Twilight Princess HD"),
        ];
        assert_eq!(
            pick_match(&results, "The Legend of Zelda Twilight Princess", |h| h.0),
            Some(&results[1])
        );
        assert_eq!(pick_match(&results, "Pikmin", |h| h.0), None);
        assert_eq!(pick_match::<Hit>(&[], "Pikmin", |h| h.0), None);
        // Ties go to the provider's first result.
        let twins = [Hit("Pikmin"), Hit("PIKMIN")];
        assert_eq!(pick_match(&twins, "pikmin", |h| h.0), Some(&twins[0]));
    }

    #[test]
    fn similarity_reads_ampersand_and_punctuation() {
        assert_eq!(similarity("Banjo & Kazooie", "banjo and kazooie"), 1.0);
        assert_eq!(similarity("F-Zero X", "F Zero X"), 1.0);
        assert!(similarity("Time of Ocarina", "Ocarina of Time") < 1.0);
        assert_eq!(similarity("", "x"), 0.0);
    }
}

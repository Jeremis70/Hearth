//! Dolphin's shipped Gecko codes (read-only): finding a game's widescreen code in its
//! `Sys/GameSettings` files and whether RetroAchievements approves it.
//!
//! Only codes Dolphin ships are used; nothing is downloaded. Everything here was checked
//! against Dolphin 2609:
//! - a game's shipped settings are `<id[0]>.ini`, `<id[0..3]>.ini`, `<id>.ini` and
//!   `<id>r<rev>.ini`, merged in that order; a section present in several files gets the
//!   lines of all of them (`SConfig::LoadDefaultGameIni`, `GetGameIniFilenames`);
//! - in `[Gecko]`, `$Name [Creator]` starts a code and each `XXXXXXXX YYYYYYYY` line is one of
//!   its address/data pairs; a line that doesn't parse counts as 0/0 (`GeckoCodeConfig.cpp`);
//! - `Sys/ApprovedInis.json` maps an INI name (`GZ2E01`) and a code name to the SHA-1 of the
//!   code: its number of lines (u64) then each address and data (u32), little-endian
//!   (`AchievementManager::GetCodeHash`, `IsApprovedCode`).

use std::fs;
use std::path::Path;

use sha1::{Digest, Sha1};

/// A shipped Gecko code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeckoCode {
    /// Its name, as `[Gecko_Enabled]` lists it (`$<name>`).
    pub name: String,
    /// Address/data pairs.
    pub lines: Vec<(u32, u32)>,
}

/// A game's widescreen code and whether it keeps RetroAchievements hardcore mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widescreen {
    pub code: GeckoCode,
    /// On Dolphin's list of codes approved for RetroAchievements (name and hash).
    pub approved: bool,
}

impl Widescreen {
    /// Enabling it would end hardcore mode (stored only; no behavior yet).
    pub fn breaks_hardcore(&self) -> bool {
        !self.approved
    }
}

/// The shipped settings files that apply to game `id`, without `.ini`, lowest first.
pub fn ini_names(id: &str, revision: Option<u16>) -> Vec<String> {
    let mut names = Vec::new();
    if id.len() == 6 && id.is_ascii() {
        names.push(id[..1].to_owned());
        names.push(id[..3].to_owned());
    }
    names.push(id.to_owned());
    if let Some(rev) = revision {
        names.push(format!("{id}r{rev}"));
    }
    names
}

/// The lines of `section` across the shipped files of game `id`, in Dolphin's order.
fn section_lines(
    game_settings: &Path,
    id: &str,
    revision: Option<u16>,
    section: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    for name in ini_names(id, revision) {
        let Ok(text) = fs::read_to_string(game_settings.join(format!("{name}.ini"))) else {
            continue;
        };
        let mut inside = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(header) = trimmed.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                inside = header.trim() == section;
                continue;
            }
            if inside {
                out.push(trimmed.to_owned());
            }
        }
    }
    out
}

/// The Gecko codes Dolphin ships for game `id` (`game_settings` is `Sys/GameSettings`).
pub fn shipped_codes(game_settings: &Path, id: &str, revision: Option<u16>) -> Vec<GeckoCode> {
    let mut codes: Vec<GeckoCode> = Vec::new();
    for line in section_lines(game_settings, id, revision, "Gecko") {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+').unwrap_or(&line).strip_prefix('$') {
            let name = rest.split('[').next().unwrap_or("").trim().to_owned();
            codes.push(GeckoCode {
                name,
                lines: Vec::new(),
            });
        } else if line.starts_with('*') {
            // A note.
        } else if let Some(code) = codes.last_mut() {
            code.lines.push(parse_line(&line).unwrap_or((0, 0)));
        }
    }
    codes.retain(|c| !c.name.is_empty());
    codes
}

fn parse_line(line: &str) -> Option<(u32, u32)> {
    let mut items = line.split(' ');
    let address = u32::from_str_radix(items.next()?, 16).ok()?;
    let data = u32::from_str_radix(items.next()?, 16).ok()?;
    Some((address, data))
}

/// Whether a code's name says it makes the game widescreen (not an add-on to another code).
fn is_widescreen(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.contains("widescreen")
        || lower.contains("wide screen")
        || lower.contains("16:9")
        || lower.contains("16x9"))
        && !lower.contains("add-on")
}

/// SHA-1 of a code as RetroAchievements' approved list stores it (hex, upper case).
pub fn code_hash(code: &GeckoCode) -> String {
    let mut hasher = Sha1::new();
    hasher.update((code.lines.len() as u64).to_le_bytes());
    for (address, data) in &code.lines {
        hasher.update(address.to_le_bytes());
        hasher.update(data.to_le_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

/// Whether `Sys/ApprovedInis.json` approves `code` for game `id`.
pub fn is_approved(
    approved_json: &Path,
    id: &str,
    revision: Option<u16>,
    code: &GeckoCode,
) -> bool {
    let Ok(text) = fs::read_to_string(approved_json) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let hash = code_hash(code);
    ini_names(id, revision).iter().any(|name| {
        json.get(name)
            .and_then(|codes| codes.get(&code.name))
            .and_then(|h| h.as_str())
            .is_some_and(|h| h.eq_ignore_ascii_case(&hash))
    })
}

/// Game `id`'s widescreen code: the first shipped code, in Dolphin's order, whose name says
/// widescreen (or 16:9) and isn't an add-on. `sys` is Dolphin's `Sys` folder.
pub fn widescreen(sys: &Path, id: &str, revision: Option<u16>) -> Option<Widescreen> {
    let code = shipped_codes(&sys.join("GameSettings"), id, revision)
        .into_iter()
        .find(|c| is_widescreen(&c.name) && !c.lines.is_empty())?;
    let approved = is_approved(&sys.join("ApprovedInis.json"), id, revision, &code);
    Some(Widescreen { code, approved })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    /// TP's shipped "16:9 Widescreen v2" (Dolphin 2609, `GZ2E01.ini`): the hash's test vector.
    const TP_CODE: &str = "06003F30 00000068
C002A014 D0040000
C062A030 EC03002A
D0040008 482E58C8
38C0FF9A B0C48000
4E800020 C022A014
C042A708 C062A030
4E800020 3C004391
90040000 3C0043FE
60008000 90040008
482E5148 C001001C
C042A024 EC020028
C0629F04 EC030032
EC020028 4E800020
0400AF3C 4BFF8FF5
041999F0 4BE6A58D
0419BDF0 4BE68165
0419BE78 4BE680DD
041B7628 C0429F1C
041BAD08 4BE4925D
041BAFE0 4BE48F85
041C51A0 4BE3EDB5
041D013C C0429F04
041D0140 ECC11024
041D0144 EC41302A
041D0154 EC003028
041D015C EC001828
041D87B4 4BE2B7A1
041DB604 4BE28951
041DD574 4BE269E1
041DD634 EC40002A
041DD650 60000000
041E3720 4BE20835
041F7808 4BE0C74D
041FE278 4BE05CD1
041FE28C 380002C6
041FE2C0 4BE05C89
0420D7E4 3860FFCD
0423A018 FC20F090
0423DA9C C02294F0
0423DAA0 C042B1C0
042480BC C02294F0
042480C0 C042B304
0442EBF0 C2C20000
0442EC28 C2BC0000
0442EC3C C2BC0000
0442EC54 42AE0000
0442EC58 42AE0000
0442EF18 42CC0000
0442EF28 42B00000
0442EFC8 C2B40000
0442EFD0 C2B40000
0442F670 42A00000
0442FA54 42A80000
04430418 C2CC0000
04430438 C2CC0000
0445391C 3FE79E79
04453A14 C2CC0000
04453A30 444B0000
04454840 C30A0000";

    #[test]
    fn names_follow_dolphins_order() {
        assert_eq!(ini_names("GZ2E01", None), ["G", "GZ2", "GZ2E01"]);
        assert_eq!(
            ini_names("GZ2E01", Some(2)),
            ["G", "GZ2", "GZ2E01", "GZ2E01r2"]
        );
        assert_eq!(ini_names("HAXX", None), ["HAXX"]);
    }

    #[test]
    fn tps_real_code_matches_dolphins_approved_hash() {
        // Installed Dolphin: check against its own files when present.
        let sys = Path::new("/usr/share/dolphin-emu/Sys");
        if sys.join("GameSettings/GZ2E01.ini").is_file() {
            let ws = widescreen(sys, "GZ2E01", Some(0)).unwrap();
            assert_eq!(ws.code.name, "16:9 Widescreen v2");
            assert_eq!(
                code_hash(&ws.code),
                "E7521ED27BFC972628906CBE8D5403ED57253BB8"
            );
            assert!(ws.approved);
            assert!(!ws.breaks_hardcore());
        }
    }

    fn sys_with(files: &[(&str, &str)], approved: &str) -> tempfile::TempDir {
        let dir = tempdir();
        let gs = dir.path().join("GameSettings");
        fs::create_dir_all(&gs).unwrap();
        for (name, text) in files {
            fs::write(gs.join(name), text).unwrap();
        }
        fs::write(dir.path().join("ApprovedInis.json"), approved).unwrap();
        dir
    }

    #[test]
    fn finds_the_widescreen_code_across_files() {
        let code = GeckoCode {
            name: "16:9 Widescreen v2".into(),
            lines: vec![(0x0600_3F30, 0x68)],
        };
        let approved = format!(
            "{{\"GZ2E01\": {{\"16:9 Widescreen v2\": \"{}\"}}}}",
            code_hash(&code)
        );
        let dir = sys_with(
            &[
                (
                    "GZ2.ini",
                    "[Gecko]\n# comment\n$Infinite Hearts [Someone]\n04000000 00000001\n",
                ),
                (
                    "GZ2E01.ini",
                    "[OnFrame]\n$Not gecko\n0x8000:dword:0x1\n[Gecko]\n$16:9 Widescreen v2 [Ralf]\n*a note\n06003F30 00000068\n[Gecko_RetroAchievements_Verified]\n$16:9 Widescreen v2\n",
                ),
            ],
            &approved,
        );
        let codes = shipped_codes(&dir.path().join("GameSettings"), "GZ2E01", None);
        assert_eq!(
            codes.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["Infinite Hearts", "16:9 Widescreen v2"]
        );
        let ws = widescreen(dir.path(), "GZ2E01", None).unwrap();
        assert_eq!(ws.code, code);
        assert!(ws.approved);
        // A revision file adds its codes too.
        fs::write(
            dir.path().join("GameSettings/GZ2E01r1.ini"),
            "[Gecko]\n$Rev fix\n04000000 00000002\n",
        )
        .unwrap();
        assert_eq!(
            shipped_codes(&dir.path().join("GameSettings"), "GZ2E01", Some(1)).len(),
            3
        );
    }

    #[test]
    fn variants_add_ons_unapproved_and_none() {
        let dir = sys_with(
            &[
                (
                    "GM4E01.ini",
                    "[Gecko]\n$16:9 Menu Backgrounds Add-On\n04000000 00000001\n$16:9 Widescreen v2\n04000000 00000002\n",
                ),
                (
                    "G8ME01.ini",
                    "[Gecko]\n$16:9 Aspect Ratio Fix - Normal HUD\n04000000 00000003\n$16:9 Aspect Ratio Fix - Centered HUD\n04000000 00000004\n",
                ),
                ("GXXE01.ini", "[Gecko]\n$Moon Jump\n04000000 00000005\n"),
            ],
            "{}",
        );
        let ws = widescreen(dir.path(), "GM4E01", None).unwrap();
        assert_eq!(ws.code.name, "16:9 Widescreen v2");
        // Not on the approved list: it would end hardcore mode.
        assert!(ws.breaks_hardcore());
        assert_eq!(
            widescreen(dir.path(), "G8ME01", None).unwrap().code.name,
            "16:9 Aspect Ratio Fix - Normal HUD"
        );
        assert_eq!(widescreen(dir.path(), "GXXE01", None), None);
        assert_eq!(widescreen(dir.path(), "GABE01", None), None);
    }

    #[test]
    fn tps_code_hashes_like_dolphins_approved_list() {
        let code = GeckoCode {
            name: "16:9 Widescreen v2".into(),
            lines: TP_CODE.lines().map(|l| parse_line(l).unwrap()).collect(),
        };
        assert_eq!(code.lines.len(), 60);
        // Sys/ApprovedInis.json, "GZ2E01" → "16:9 Widescreen v2".
        assert_eq!(code_hash(&code), "E7521ED27BFC972628906CBE8D5403ED57253BB8");
        assert_eq!(parse_line("ZZZZZZZZ 00000000"), None);
    }
}

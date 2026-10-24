//! Asset packs: user-installed console icons and UI sounds that replace Hearth's defaults.
//!
//! A pack is a folder `<packs>/<id>/` with an optional `pack.toml` (name, author, license,
//! notes) and any of:
//! - `icons/consoles/<system>.svg` or `.png`
//! - `sounds/<name>.ogg`, `.wav`, `.flac` or `.mp3`
//!
//! Assets are looked up by logical name (`consoles/gc`, `sounds/move`): first in the pack
//! selected for that category, then in Hearth's bundled defaults (`assets/`). Each file falls
//! back on its own, so a pack may hold only some of them.
//!
//! A sound pack's `pack.toml` may point logical names at other files inside the pack and give
//! loop points for music, so a pack can use files as they came:
//!
//! ```toml
//! [sounds]
//! ambient_loop = "themes/003. Home Menu.mp3"
//!
//! [loops.ambient_loop]
//! start = 1.0          # seconds; the loop restarts here
//! end = 884.4          # seconds; default: the end of the file
//! crossfade_ms = 2000  # the end fades into the start (equal power)
//!
//! [gain]                # corrects the pack's own mastering, in dB (±24)
//! ui_db = 10            # UI sounds
//! music_db = -4         # music (ambient loops)
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;

use crate::config::{Config, PackCategory};

/// Logical names of the UI sounds. `achievement`, `mastery`, `ambient_profiles` and `startup`
/// are reserved: they resolve, but nothing plays them yet.
pub const SOUND_NAMES: [&str; 11] = [
    "move",
    "accept",
    "back",
    "overlay_open",
    "overlay_close",
    "edge_bump",
    "ambient_loop",
    "achievement",
    "mastery",
    "ambient_profiles",
    "startup",
];

/// Logical sound names that are music (streamed, looped), not one-shot UI sounds.
pub const MUSIC_NAMES: [&str; 2] = ["ambient_loop", "ambient_profiles"];

/// The largest gain correction a pack may ask for, in dB.
const MAX_GAIN_DB: f64 = 24.0;

const ICON_EXTENSIONS: [&str; 2] = ["svg", "png"];
const SOUND_EXTENSIONS: [&str; 4] = ["ogg", "wav", "flac", "mp3"];

/// An installed pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackInfo {
    /// Folder name, stored in the config.
    pub id: String,
    /// Display name (`pack.toml`, else the id).
    pub name: String,
    /// Author, if given.
    pub author: Option<String>,
    /// License, if given.
    pub license: Option<String>,
    /// Free-form notes, if given.
    pub notes: Option<String>,
    /// The pack's folder.
    pub dir: PathBuf,
    /// Categories it has files for.
    pub categories: Vec<PackCategory>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PackToml {
    name: Option<String>,
    author: Option<String>,
    license: Option<String>,
    notes: Option<String>,
    /// Logical sound name → file inside the pack.
    sounds: BTreeMap<String, String>,
    /// Logical sound name → loop points.
    loops: BTreeMap<String, LoopToml>,
    gain: GainToml,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GainToml {
    ui_db: Option<f64>,
    music_db: Option<f64>,
}

fn gain_db(db: Option<f64>) -> i32 {
    db.unwrap_or(0.0).clamp(-MAX_GAIN_DB, MAX_GAIN_DB).round() as i32
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LoopToml {
    start: Option<f64>,
    end: Option<f64>,
    crossfade_ms: Option<u32>,
}

/// Where a looping sound restarts, from a pack's `[loops.<name>]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoopSpec {
    /// The loop restarts here (ms from the start of the file).
    pub start_ms: u64,
    /// The loop ends here (ms); `None`: the end of the file.
    pub end_ms: Option<u64>,
    /// How long the end fades into the start.
    pub crossfade_ms: u32,
    /// Where the first play starts (ms; later passes start at `start_ms`). Themes skip their
    /// leading silence with it.
    pub intro_ms: u64,
}

impl LoopToml {
    fn spec(&self) -> LoopSpec {
        let ms = |secs: f64| (secs.max(0.0) * 1000.0).round() as u64;
        LoopSpec {
            start_ms: self.start.map(ms).unwrap_or(0),
            end_ms: self.end.map(ms),
            crossfade_ms: self.crossfade_ms.unwrap_or(0),
            intro_ms: 0,
        }
    }
}

fn read_pack_toml(id: &str, dir: &Path) -> PackToml {
    match fs::read_to_string(dir.join("pack.toml")) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|err: toml::de::Error| {
            tracing::warn!(pack = %id, "ignoring its pack.toml: {}", err.message());
            PackToml::default()
        }),
        Err(_) => PackToml::default(),
    }
}

/// A relative path that stays inside its folder (no `..`, not absolute).
fn inside_pack(rel: &str) -> Option<PathBuf> {
    let path = Path::new(rel);
    let ok = !rel.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then(|| path.to_path_buf())
}

/// The sound mappings and loop points of the selected sound pack.
#[derive(Debug, Default)]
struct SoundMap {
    files: HashMap<String, PathBuf>,
    loops: HashMap<String, LoopSpec>,
    ui_db: i32,
    music_db: i32,
}

impl SoundMap {
    fn load(packs_dir: &Path, pack: Option<&str>) -> Self {
        let Some(id) = pack else {
            return Self::default();
        };
        let meta = read_pack_toml(id, &packs_dir.join(id));
        let files = meta
            .sounds
            .into_iter()
            .filter_map(|(name, rel)| match inside_pack(&rel) {
                Some(path) => Some((name, path)),
                None => {
                    tracing::warn!(pack = %id, sound = %name, "ignoring a mapping outside the pack");
                    None
                }
            })
            .collect();
        let loops = meta
            .loops
            .into_iter()
            .map(|(name, spec)| (name, spec.spec()))
            .collect();
        Self {
            files,
            loops,
            ui_db: gain_db(meta.gain.ui_db),
            music_db: gain_db(meta.gain.music_db),
        }
    }
}

impl PackCategory {
    /// The folder inside a pack (and inside `assets/`) for this category.
    fn folder(self) -> &'static str {
        match self {
            Self::ConsoleIcons => "icons/consoles",
            Self::UiSounds => "sounds",
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::ConsoleIcons => &ICON_EXTENSIONS,
            Self::UiSounds => &SOUND_EXTENSIONS,
        }
    }

    /// The category of a logical name's prefix (`consoles/…`, `sounds/…`).
    fn of_logical(name: &str) -> Option<(Self, &str)> {
        let (prefix, rest) = name.split_once('/')?;
        let category = match prefix {
            "consoles" => Self::ConsoleIcons,
            "sounds" => Self::UiSounds,
            _ => return None,
        };
        let safe = !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        safe.then_some((category, rest))
    }
}

/// Installed packs in `packs_dir`, by name. Folders with nothing usable are skipped.
pub fn installed(packs_dir: &Path) -> Vec<PackInfo> {
    let Ok(entries) = fs::read_dir(packs_dir) else {
        return Vec::new();
    };
    let mut packs: Vec<PackInfo> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let id = e.file_name().to_str()?.to_owned();
            if id.starts_with('.') {
                return None;
            }
            read_pack(id, e.path())
        })
        .collect();
    packs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    packs
}

fn read_pack(id: String, dir: PathBuf) -> Option<PackInfo> {
    let meta = read_pack_toml(&id, &dir);
    let categories: Vec<_> = PackCategory::ALL
        .into_iter()
        .filter(|c| dir.join(c.folder()).is_dir())
        .collect();
    if categories.is_empty() {
        return None;
    }
    Some(PackInfo {
        name: meta.name.unwrap_or_else(|| id.clone()),
        id,
        author: meta.author,
        license: meta.license,
        notes: meta.notes,
        dir,
        categories,
    })
}

/// Where a resolved asset came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The selected pack (its id).
    Pack(String),
    /// Hearth's bundled defaults.
    Default,
}

/// A file found for a logical name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The file.
    pub path: PathBuf,
    /// Pack or default.
    pub origin: Origin,
    /// Loop points, for a sound whose pack gives them.
    pub loop_spec: Option<LoopSpec>,
    /// Gain correction from the pack's `[gain]`, in dB (0 for Hearth's defaults and icons).
    pub gain_db: i32,
}

/// Finds asset files by logical name, given the selected packs.
#[derive(Debug)]
pub struct Resolver {
    packs_dir: PathBuf,
    defaults_dir: PathBuf,
    console_icons: Option<String>,
    ui_sounds: Option<String>,
    /// `[sounds]` and `[loops]` of the selected sound pack.
    sound_map: SoundMap,
    /// Logical names already reported missing (logged once each).
    reported: Mutex<HashSet<String>>,
}

impl Resolver {
    /// A resolver over `packs_dir` (installed packs) and `defaults_dir` (Hearth's `assets/`),
    /// with the packs selected in `config`.
    pub fn new(
        packs_dir: impl Into<PathBuf>,
        defaults_dir: impl Into<PathBuf>,
        config: &Config,
    ) -> Self {
        let packs_dir = packs_dir.into();
        let ui_sounds = config.pack(PackCategory::UiSounds).map(str::to_owned);
        Self {
            sound_map: SoundMap::load(&packs_dir, ui_sounds.as_deref()),
            packs_dir,
            defaults_dir: defaults_dir.into(),
            console_icons: config.pack(PackCategory::ConsoleIcons).map(str::to_owned),
            ui_sounds,
            reported: Mutex::new(HashSet::new()),
        }
    }

    /// Selects a pack (or the defaults) for a category.
    pub fn select(&mut self, category: PackCategory, pack: Option<&str>) {
        let pack = pack.map(str::to_owned);
        match category {
            PackCategory::ConsoleIcons => self.console_icons = pack,
            PackCategory::UiSounds => {
                self.sound_map = SoundMap::load(&self.packs_dir, pack.as_deref());
                self.ui_sounds = pack;
            }
        }
        // A new selection may have the files that were missing.
        if let Ok(mut reported) = self.reported.lock() {
            reported.clear();
        }
    }

    /// The pack selected for a category.
    pub fn selected(&self, category: PackCategory) -> Option<&str> {
        match category {
            PackCategory::ConsoleIcons => self.console_icons.as_deref(),
            PackCategory::UiSounds => self.ui_sounds.as_deref(),
        }
    }

    /// The file for `logical` (`consoles/gc`, `sounds/move`): the selected pack's, else the
    /// default, else `None`.
    pub fn resolve(&self, logical: &str) -> Option<Resolved> {
        let (category, name) = PackCategory::of_logical(logical)?;
        let find = |dir: &Path| {
            category
                .extensions()
                .iter()
                .map(|ext| dir.join(category.folder()).join(format!("{name}.{ext}")))
                .find(|path| path.is_file())
        };
        if let Some(pack) = self.selected(category) {
            let pack_dir = self.packs_dir.join(pack);
            let mapped = match category {
                PackCategory::UiSounds => self
                    .sound_map
                    .files
                    .get(name)
                    .map(|rel| pack_dir.join(rel))
                    .filter(|path| path.is_file()),
                PackCategory::ConsoleIcons => None,
            };
            if let Some(path) = mapped.or_else(|| find(&pack_dir)) {
                let (loop_spec, gain_db) = match category {
                    PackCategory::UiSounds => (
                        self.sound_map.loops.get(name).copied(),
                        if MUSIC_NAMES.contains(&name) {
                            self.sound_map.music_db
                        } else {
                            self.sound_map.ui_db
                        },
                    ),
                    PackCategory::ConsoleIcons => (None, 0),
                };
                return Some(Resolved {
                    path,
                    origin: Origin::Pack(pack.to_owned()),
                    loop_spec,
                    gain_db,
                });
            }
        }
        find(&self.defaults_dir).map(|path| Resolved {
            path,
            origin: Origin::Default,
            loop_spec: None,
            gain_db: 0,
        })
    }

    /// Like [`resolve`](Self::resolve), logging a missing asset once.
    pub fn resolve_or_log(&self, logical: &str) -> Option<Resolved> {
        let found = self.resolve(logical);
        if found.is_none()
            && let Ok(mut reported) = self.reported.lock()
            && reported.insert(logical.to_owned())
        {
            tracing::info!(
                asset = logical,
                "no file in the selected pack or Hearth's defaults"
            );
        }
        found
    }

    /// The console icon of `system`.
    pub fn console_icon(&self, system: &str) -> Option<Resolved> {
        self.resolve(&format!("consoles/{system}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn pack_files_win_and_missing_ones_fall_back_one_by_one() {
        let dir = tempdir();
        let packs = dir.path().join("packs");
        let assets = dir.path().join("assets");
        touch(&packs.join("mine/icons/consoles/gc.png"));
        touch(&packs.join("mine/sounds/move.mp3"));
        touch(&assets.join("icons/consoles/gc.svg"));
        touch(&assets.join("icons/consoles/n64.svg"));

        let mut config = Config::default();
        config.set_pack(PackCategory::ConsoleIcons, Some("mine"));
        let mut resolver = Resolver::new(&packs, &assets, &config);

        let gc = resolver.console_icon("gc").unwrap();
        assert_eq!(gc.origin, Origin::Pack("mine".into()));
        assert_eq!(gc.path, packs.join("mine/icons/consoles/gc.png"));
        let n64 = resolver.console_icon("n64").unwrap();
        assert_eq!(n64.origin, Origin::Default);
        assert_eq!(resolver.console_icon("wii"), None);

        // Sounds use their own selection: none yet, so the pack's sound isn't used.
        assert_eq!(resolver.resolve("sounds/move"), None);
        resolver.select(PackCategory::UiSounds, Some("mine"));
        assert_eq!(
            resolver.resolve_or_log("sounds/move").unwrap().path,
            packs.join("mine/sounds/move.mp3")
        );
        assert_eq!(resolver.resolve_or_log("sounds/accept"), None);

        resolver.select(PackCategory::ConsoleIcons, None);
        assert_eq!(resolver.console_icon("gc").unwrap().origin, Origin::Default);
    }

    #[test]
    fn pack_toml_maps_sounds_inside_the_pack_with_loop_points() {
        let dir = tempdir();
        let packs = dir.path().join("packs");
        let assets = dir.path().join("assets");
        touch(&packs.join("ps5/themes/003. Home Menu.mp3"));
        touch(&packs.join("ps5/sounds/move.mp3"));
        touch(&packs.join("secret.ogg"));
        touch(&assets.join("sounds/ambient_loop.ogg"));
        touch(&assets.join("sounds/back.wav"));
        fs::write(
            packs.join("ps5/pack.toml"),
            "[sounds]\nambient_loop = \"themes/003. Home Menu.mp3\"\n\
             accept = \"../secret.ogg\"\nback = \"themes/gone.ogg\"\n\
             [loops.ambient_loop]\nstart = 1.0\nend = 884.4\ncrossfade_ms = 2000\n\
             [gain]\nui_db = 10.4\nmusic_db = -40\n",
        )
        .unwrap();
        let mut config = Config::default();
        config.set_pack(PackCategory::UiSounds, Some("ps5"));
        let mut resolver = Resolver::new(&packs, &assets, &config);

        let ambient = resolver.resolve("sounds/ambient_loop").unwrap();
        assert_eq!(ambient.path, packs.join("ps5/themes/003. Home Menu.mp3"));
        assert_eq!(
            ambient.loop_spec,
            Some(LoopSpec {
                start_ms: 1000,
                end_ms: Some(884_400),
                crossfade_ms: 2000,
                intro_ms: 0,
            })
        );
        assert_eq!(ambient.gain_db, -24, "music gain, clamped");
        // Unmapped names use the default location; escaping mappings are ignored.
        let click = resolver.resolve("sounds/move").unwrap();
        assert_eq!(click.path, packs.join("ps5/sounds/move.mp3"));
        assert_eq!(click.gain_db, 10);
        assert_eq!(resolver.resolve("sounds/accept"), None);
        // A mapping to a missing file falls back like a missing file.
        assert_eq!(
            resolver.resolve("sounds/back").unwrap().origin,
            Origin::Default
        );

        resolver.select(PackCategory::UiSounds, None);
        let ambient = resolver.resolve("sounds/ambient_loop").unwrap();
        assert_eq!(ambient.origin, Origin::Default);
        assert_eq!(ambient.loop_spec, None);
    }

    #[test]
    fn missing_or_unknown_packs_and_odd_names_fall_back() {
        let dir = tempdir();
        let assets = dir.path().join("assets");
        touch(&assets.join("icons/consoles/gc.svg"));
        let mut config = Config::default();
        config.set_pack(PackCategory::ConsoleIcons, Some("gone"));
        let resolver = Resolver::new(dir.path().join("packs"), &assets, &config);
        assert_eq!(resolver.console_icon("gc").unwrap().origin, Origin::Default);
        assert_eq!(resolver.resolve("consoles/../gc"), None);
        assert_eq!(resolver.resolve("music/gc"), None);
        assert_eq!(resolver.resolve("consoles/"), None);
    }

    #[test]
    fn installed_packs_read_pack_toml_or_default_to_the_id() {
        let dir = tempdir();
        let packs = dir.path().join("packs");
        touch(&packs.join("zeta/icons/consoles/gc.svg"));
        fs::write(
            packs.join("zeta/pack.toml"),
            "name = \"Alpha icons\"\nauthor = \"Me\"\nlicense = \"CC0\"\n",
        )
        .unwrap();
        touch(&packs.join("beta-sounds/sounds/move.wav"));
        touch(&packs.join("empty/readme.txt"));
        touch(&packs.join("broken/icons/consoles/n64.svg"));
        fs::write(packs.join("broken/pack.toml"), "name = [").unwrap();

        let found = installed(&packs);
        let names: Vec<_> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Alpha icons", "beta-sounds", "broken"]);
        assert_eq!(found[0].id, "zeta");
        assert_eq!(found[0].author.as_deref(), Some("Me"));
        assert_eq!(found[0].categories, [PackCategory::ConsoleIcons]);
        assert_eq!(found[1].categories, [PackCategory::UiSounds]);
        assert!(installed(&dir.path().join("none")).is_empty());
    }
}

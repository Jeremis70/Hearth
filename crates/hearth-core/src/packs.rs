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

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Deserialize;

use crate::config::{Config, PackCategory};

/// Logical names of the UI sounds (played from the next round on).
pub const SOUND_NAMES: [&str; 7] = [
    "move",
    "accept",
    "back",
    "overlay_open",
    "overlay_close",
    "edge_bump",
    "ambient_loop",
];

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
    let meta = match fs::read_to_string(dir.join("pack.toml")) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|err: toml::de::Error| {
            tracing::warn!(pack = %id, "ignoring its pack.toml: {}", err.message());
            PackToml::default()
        }),
        Err(_) => PackToml::default(),
    };
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
}

/// Finds asset files by logical name, given the selected packs.
#[derive(Debug)]
pub struct Resolver {
    packs_dir: PathBuf,
    defaults_dir: PathBuf,
    console_icons: Option<String>,
    ui_sounds: Option<String>,
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
        Self {
            packs_dir: packs_dir.into(),
            defaults_dir: defaults_dir.into(),
            console_icons: config.pack(PackCategory::ConsoleIcons).map(str::to_owned),
            ui_sounds: config.pack(PackCategory::UiSounds).map(str::to_owned),
            reported: Mutex::new(HashSet::new()),
        }
    }

    /// Selects a pack (or the defaults) for a category.
    pub fn select(&mut self, category: PackCategory, pack: Option<&str>) {
        let pack = pack.map(str::to_owned);
        match category {
            PackCategory::ConsoleIcons => self.console_icons = pack,
            PackCategory::UiSounds => self.ui_sounds = pack,
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
        if let Some(pack) = self.selected(category)
            && let Some(path) = find(&self.packs_dir.join(pack))
        {
            return Some(Resolved {
                path,
                origin: Origin::Pack(pack.to_owned()),
            });
        }
        find(&self.defaults_dir).map(|path| Resolved {
            path,
            origin: Origin::Default,
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

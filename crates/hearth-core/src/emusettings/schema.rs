//! The settings Hearth offers for an emulator, declared in `data/profiles/<emu>/settings.toml`:
//! each setting's key in the emulator's files, its friendly choices, its default and scope, and
//! the presets made of them.

use std::collections::BTreeMap;

use serde::Deserialize;

const DOLPHIN: &str = include_str!("../../../../data/profiles/dolphin/settings.toml");

/// A problem with a settings file (shipped files are checked by tests).
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("invalid settings file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("settings file: {0}")]
    Invalid(String),
}

/// Which config file of the emulator a key lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
pub enum ConfigFile {
    /// Dolphin's `Config/Dolphin.ini`.
    Main,
    /// Dolphin's `Config/GFX.ini`.
    #[serde(rename = "GFX")]
    Gfx,
    /// Dolphin's `Config/Logger.ini`.
    Logger,
    /// RetroArch's `retroarch.cfg`.
    #[serde(rename = "retroarch")]
    RetroArch,
}

/// The kind of value a setting holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// `True` / `False`.
    Bool,
    /// One of the listed choices.
    Choice,
}

/// Where a setting may be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Shared by every profile, console-wide only.
    Console,
    /// Console-wide, and overridable per game.
    PerGame,
}

/// When a changed setting takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    /// Hearth writes it before the emulator starts.
    NextLaunch,
    /// Needs the running game restarted.
    RestartGame,
}

/// One value of a setting.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Choice {
    /// What Hearth stores; defaults to `value`.
    #[serde(default)]
    pub id: String,
    /// What the setting's key holds.
    pub value: String,
    /// What the user sees.
    pub label: String,
    /// Other keys of the same section this choice sets.
    #[serde(default)]
    pub set: BTreeMap<String, String>,
}

/// One setting.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Setting {
    pub id: String,
    pub label: String,
    pub description: String,
    pub file: ConfigFile,
    pub section: String,
    pub key: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// The choice id the emulator uses when the key is missing.
    pub default: String,
    pub scope: Scope,
    pub applies: Applies,
    /// Changing it from the default would end RetroAchievements hardcore mode (stored only).
    #[serde(default)]
    pub breaks_hardcore: bool,
}

impl Setting {
    /// The choice with id `id`.
    pub fn choice(&self, id: &str) -> Option<&Choice> {
        self.choices.iter().find(|c| c.id == id)
    }

    /// The label of choice `id`, or `id` itself.
    pub fn label_of<'a>(&'a self, id: &'a str) -> &'a str {
        self.choice(id).map_or(id, |c| c.label.as_str())
    }

    /// The keys (of this setting's section) choice `id` writes, the setting's own key first.
    pub fn keys_for(&self, id: &str) -> Vec<(&str, &str)> {
        let Some(choice) = self.choice(id) else {
            return Vec::new();
        };
        let mut keys = vec![(self.key.as_str(), choice.value.as_str())];
        keys.extend(choice.set.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        keys
    }

    /// What `key` holds when the emulator uses this setting's default (the default choice's
    /// value for it).
    pub fn default_value(&self, key: &str) -> Option<&str> {
        self.keys_for(&self.default)
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    /// Every key this setting may write.
    pub fn all_keys(&self) -> Vec<&str> {
        let mut keys = vec![self.key.as_str()];
        for choice in &self.choices {
            for key in choice.set.keys() {
                if !keys.contains(&key.as_str()) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    /// The choice id matching what a file holds (`get` gives a key's value, `None` if
    /// missing; a missing key reads as the default choice's value). `None`: no choice matches.
    pub fn read(&self, get: impl Fn(&str) -> Option<String>) -> Option<&str> {
        self.read_with(get, true)
    }

    /// Like [`Setting::read`]; with `missing_is_default` false (a game INI, where a missing
    /// key means "not set here"), a missing own key reads as `None`.
    pub fn read_with(
        &self,
        get: impl Fn(&str) -> Option<String>,
        missing_is_default: bool,
    ) -> Option<&str> {
        if !missing_is_default && get(&self.key).is_none() {
            return None;
        }
        let default = self.choice(&self.default)?;
        let value_of = |key: &str| -> Option<String> {
            get(key).or_else(|| {
                if key == self.key {
                    Some(default.value.clone())
                } else {
                    default.set.get(key).cloned()
                }
            })
        };
        self.choices
            .iter()
            .find(|choice| {
                self.keys_for(&choice.id).iter().all(|(key, want)| {
                    value_of(key).is_some_and(|have| same_value(Some(self.kind), &have, want))
                })
            })
            .map(|c| c.id.as_str())
    }

    /// The choice id for a raw value of the setting's own key (single-key settings and the
    /// first choice that has it otherwise).
    pub fn choice_for_value(&self, raw: &str) -> Option<&str> {
        self.choices
            .iter()
            .find(|c| same_value(Some(self.kind), raw, &c.value))
            .map(|c| c.id.as_str())
    }
}

/// A named set of values.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Preset {
    pub id: String,
    pub label: String,
    /// Setting id → choice id.
    pub values: BTreeMap<String, String>,
}

/// A GPU → preset rule (PCI ids in hex, without `0x`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GpuRule {
    pub vendor: String,
    #[serde(default)]
    pub devices: Vec<String>,
    pub preset: String,
}

/// All settings of one emulator.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Schema {
    pub emulator: String,
    /// The emulator version every key was checked against.
    #[serde(default)]
    pub verified_with: String,
    #[serde(default)]
    pub default_preset: Option<String>,
    #[serde(default, rename = "setting")]
    pub settings: Vec<Setting>,
    #[serde(default, rename = "preset")]
    pub presets: Vec<Preset>,
    #[serde(default, rename = "gpu")]
    pub gpu_rules: Vec<GpuRule>,
}

impl Schema {
    /// Dolphin's shipped schema.
    pub fn dolphin() -> Self {
        Self::parse(DOLPHIN).expect("shipped data/profiles/dolphin/settings.toml is valid")
    }

    /// An emulator with no settings yet.
    pub fn empty(emulator: &str) -> Self {
        Self {
            emulator: emulator.to_owned(),
            verified_with: String::new(),
            default_preset: None,
            settings: Vec::new(),
            presets: Vec::new(),
            gpu_rules: Vec::new(),
        }
    }

    /// Parses and checks a settings file.
    pub fn parse(text: &str) -> Result<Self, SchemaError> {
        let mut schema: Self = toml::from_str(text)?;
        for setting in &mut schema.settings {
            if setting.kind == Kind::Bool && setting.choices.is_empty() {
                setting.choices = vec![
                    Choice {
                        id: "False".into(),
                        value: "False".into(),
                        label: "Off".into(),
                        set: BTreeMap::new(),
                    },
                    Choice {
                        id: "True".into(),
                        value: "True".into(),
                        label: "On".into(),
                        set: BTreeMap::new(),
                    },
                ];
            }
            for choice in &mut setting.choices {
                if choice.id.is_empty() {
                    choice.id = choice.value.clone();
                }
            }
        }
        schema.check()?;
        Ok(schema)
    }

    fn check(&self) -> Result<(), SchemaError> {
        let bad = |msg: String| Err(SchemaError::Invalid(msg));
        let mut ids = Vec::new();
        for s in &self.settings {
            if ids.contains(&s.id.as_str()) {
                return bad(format!("setting {} is declared twice", s.id));
            }
            ids.push(&s.id);
            if s.choices.is_empty() {
                return bad(format!("setting {} has no choices", s.id));
            }
            if s.choice(&s.default).is_none() {
                return bad(format!(
                    "setting {}: default {} isn't a choice",
                    s.id, s.default
                ));
            }
            let mut seen = Vec::new();
            for c in &s.choices {
                if seen.contains(&c.id.as_str()) {
                    return bad(format!("setting {}: choice {} is listed twice", s.id, c.id));
                }
                seen.push(&c.id);
            }
            if s.scope == Scope::PerGame && game_section(s.file, &s.section).is_none() {
                return bad(format!("setting {} can't be set per game", s.id));
            }
        }
        for p in &self.presets {
            for (id, value) in &p.values {
                let Some(setting) = self.setting(id) else {
                    return bad(format!("preset {}: no setting {id}", p.id));
                };
                if setting.choice(value).is_none() {
                    return bad(format!("preset {}: {value} isn't a choice of {id}", p.id));
                }
            }
        }
        for preset in self
            .gpu_rules
            .iter()
            .map(|r| &r.preset)
            .chain(&self.default_preset)
        {
            if self.preset(preset).is_none() {
                return bad(format!("no preset {preset}"));
            }
        }
        Ok(())
    }

    /// The setting with id `id`.
    pub fn setting(&self, id: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.id == id)
    }

    /// The preset with id `id`.
    pub fn preset(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The preset for a GPU (PCI vendor and device ids), else the default preset.
    pub fn preset_for_gpu(&self, gpu: Option<super::gpu::Gpu>) -> Option<&Preset> {
        let rule = gpu.and_then(|gpu| {
            self.gpu_rules.iter().find(|rule| {
                parse_hex(&rule.vendor) == Some(gpu.vendor)
                    && (rule.devices.is_empty()
                        || rule
                            .devices
                            .iter()
                            .any(|d| parse_hex(d) == Some(gpu.device)))
            })
        });
        rule.map(|r| r.preset.as_str())
            .or(self.default_preset.as_deref())
            .and_then(|id| self.preset(id))
    }
}

fn parse_hex(text: &str) -> Option<u16> {
    u16::from_str_radix(text.trim_start_matches("0x"), 16).ok()
}

/// The section of a Dolphin game INI (`GameSettings/<id>.ini`) holding `file`'s `section`
/// (`ConfigLoaders/GameConfigLoader.cpp`), or `None` if game INIs can't set it.
pub fn game_section(file: ConfigFile, section: &str) -> Option<String> {
    match (file, section) {
        (ConfigFile::Main, "Core") => Some("Core".into()),
        (ConfigFile::Gfx, "Hardware" | "Settings" | "Enhancements" | "Stereoscopy" | "Hacks") => {
            Some(format!("Video_{section}"))
        }
        _ => None,
    }
}

/// Whether two config values mean the same, the way the emulator reads them: `True`/`true`/`1`
/// for a bool, `2`/`02`/`2.0` for a number, otherwise the trimmed text (letters compared
/// without regard to case only for bools). `kind` is `None` for keys Hearth has no type for:
/// then bool words and numbers are still compared by meaning, `1` and `True` are not.
pub fn same_value(kind: Option<Kind>, a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a == b {
        return true;
    }
    if let (Some(x), Some(y)) = (as_bool(a, kind), as_bool(b, kind)) {
        return x == y;
    }
    if let (Ok(x), Ok(y)) = (a.parse::<f64>(), b.parse::<f64>()) {
        return x == y;
    }
    false
}

fn as_bool(text: &str, kind: Option<Kind>) -> Option<bool> {
    if text.eq_ignore_ascii_case("true") {
        Some(true)
    } else if text.eq_ignore_ascii_case("false") {
        Some(false)
    } else if kind == Some(Kind::Bool) {
        match text {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emusettings::gpu::Gpu;

    #[test]
    fn shipped_dolphin_schema_is_valid() {
        let schema = Schema::dolphin();
        assert_eq!(schema.verified_with, "Dolphin 2609");
        let ids: Vec<_> = schema.settings.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "backend",
                "internal_resolution",
                "vsync",
                "aspect_ratio",
                "widescreen_hack",
                "anti_aliasing",
                "anisotropic_filtering",
                "shader_compilation",
                "wait_for_shaders",
                "arbitrary_mipmaps",
                "texture_cache",
            ]
        );
        let wait = schema.setting("wait_for_shaders").unwrap();
        assert_eq!(wait.scope, Scope::Console);
        assert_eq!(wait.default, "False");
        assert!(
            schema
                .presets
                .iter()
                .all(|p| !p.values.contains_key("wait_for_shaders"))
        );
        assert_eq!(schema.setting("backend").unwrap().scope, Scope::Console);
    }

    #[test]
    fn gpu_rules_pick_presets() {
        let schema = Schema::dolphin();
        let pick = |vendor, device| {
            schema
                .preset_for_gpu(Some(Gpu { vendor, device }))
                .unwrap()
                .id
                .as_str()
        };
        assert_eq!(pick(0x8086, 0x9a49), "balanced"); // Iris Xe (this laptop)
        assert_eq!(pick(0x1002, 0x15bf), "quality"); // Radeon 780M
        assert_eq!(pick(0x8086, 0x5917), "performance"); // UHD 620
        assert_eq!(pick(0x10de, 0x2684), "quality");
        assert_eq!(pick(0x1234, 0x0001), "balanced"); // unknown
        assert_eq!(schema.preset_for_gpu(None).unwrap().id, "balanced");
    }

    #[test]
    fn compound_choices_read_back() {
        let schema = Schema::dolphin();
        let aa = schema.setting("anti_aliasing").unwrap();
        let file = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(aa.read(file(&[])), Some("1"));
        assert_eq!(aa.read(file(&[("MSAA", "2")])), Some("2"));
        assert_eq!(
            aa.read(file(&[("MSAA", "2"), ("SSAA", "true")])),
            Some("2-ssaa")
        );
        assert_eq!(aa.read(file(&[("MSAA", "3")])), None);
        assert_eq!(aa.keys_for("4-ssaa"), [("MSAA", "4"), ("SSAA", "True")]);
        let ir = schema.setting("internal_resolution").unwrap();
        assert_eq!(ir.read(file(&[("InternalResolution", " 03 ")])), Some("3"));
        let vsync = schema.setting("vsync").unwrap();
        assert_eq!(vsync.read(file(&[("VSync", "true")])), Some("True"));
        assert_eq!(vsync.read(file(&[("VSync", "1")])), Some("True"));
    }

    #[test]
    fn values_compare_by_meaning() {
        assert!(same_value(None, "True", "true"));
        assert!(same_value(None, "2", "2.0"));
        assert!(same_value(None, " x ", "x"));
        assert!(!same_value(None, "1", "True"));
        assert!(same_value(Some(Kind::Bool), "1", "True"));
        assert!(!same_value(None, "OGL", "ogl"));
        assert!(!same_value(None, "1", "2"));
    }

    #[test]
    fn bad_files_are_rejected() {
        let base = "emulator = \"x\"\n";
        let setting = |extra: &str| {
            format!(
                "{base}[[setting]]\nid = \"a\"\nlabel = \"A\"\ndescription = \"\"\nfile = \"GFX\"\n\
                 section = \"Settings\"\nkey = \"K\"\ntype = \"bool\"\ndefault = \"False\"\n\
                 scope = \"per_game\"\napplies = \"next_launch\"\n{extra}"
            )
        };
        assert!(Schema::parse(&setting("")).is_ok());
        assert!(
            Schema::parse(&setting(
                "[[preset]]\nid = \"p\"\nlabel = \"P\"\nvalues = { a = \"Maybe\" }\n"
            ))
            .is_err()
        );
        assert!(
            Schema::parse(&setting(
                "[[preset]]\nid = \"p\"\nlabel = \"P\"\nvalues = { b = \"True\" }\n"
            ))
            .is_err()
        );
        assert!(
            Schema::parse(&setting("").replace("\"False\"\nscope", "\"Nope\"\nscope")).is_err()
        );
        assert!(Schema::parse(&setting("").replace("Settings", "Interface")).is_err());
        assert!(Schema::parse(&format!("{base}default_preset = \"none\"\n")).is_err());
    }
}

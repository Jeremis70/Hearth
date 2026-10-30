//! Format-preserving editing of emulator config files.
//!
//! Hearth merges its keys into files the user (and the emulator) also write, so it must never
//! regenerate them: every line it doesn't change stays byte for byte, comments and unknown keys
//! included. A changed key gets its line rewritten in the file's own style; a missing key is
//! appended at the end of its section (a missing section at the end of the file).
//!
//! Two styles:
//! - [`Style::Ini`]: Dolphin's `[Section]` and `Key = Value`. Dolphin compares sections and keys
//!   without regard to case (`Common/IniFile.cpp`), so lookups here do too.
//! - [`Style::Cfg`]: RetroArch's `key = "value"`, no sections (use the section `""`).

use std::fmt::Write as _;
use std::io;
use std::path::Path;

/// The syntax of a config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Dolphin: `[Section]`, `Key = Value`, case-insensitive names.
    Ini,
    /// RetroArch: `key = "value"`, no sections.
    Cfg,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// A line kept as it was (comments, blank lines, anything unparsed).
    Raw(String),
    /// `[name]`, with its original text.
    Section { name: String, text: String },
    /// A key line; `text` is the original line, kept unless the value changes.
    Key {
        key: String,
        value: String,
        text: String,
    },
}

/// A config file as lines, editable without disturbing what Hearth doesn't touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDoc {
    style: Style,
    lines: Vec<Line>,
    /// Whether the text ended with a newline (kept on output).
    trailing_newline: bool,
}

impl ConfigDoc {
    /// An empty document.
    pub fn new(style: Style) -> Self {
        Self {
            style,
            lines: Vec::new(),
            trailing_newline: true,
        }
    }

    /// Parses `text`. Never fails: lines it can't read are kept as they are.
    pub fn parse(text: &str, style: Style) -> Self {
        let lines = text.lines().map(|line| parse_line(line, style)).collect();
        Self {
            style,
            lines,
            trailing_newline: text.is_empty() || text.ends_with('\n'),
        }
    }

    /// Reads `path`; a missing file is an empty document.
    pub fn load(path: &Path, style: Style) -> io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text, style)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::new(style)),
            Err(err) => Err(err),
        }
    }

    /// Writes the document to `path` atomically (temporary file, then rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::users::write_atomic(path, self.to_string().as_bytes())
    }

    /// The file's style.
    pub fn style(&self) -> Style {
        self.style
    }

    /// The value of `key` in `section` (the last one if the key repeats), unquoted.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.entries()
            .filter(|(s, k, _)| self.same(s, section) && self.same(k, key))
            .last()
            .map(|(_, _, v)| v)
    }

    /// Every key as (section, key, value), in file order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        let mut section = "";
        self.lines.iter().filter_map(move |line| match line {
            Line::Section { name, .. } => {
                section = name;
                None
            }
            Line::Key { key, value, .. } => Some((section, key.as_str(), value.as_str())),
            Line::Raw(_) => None,
        })
    }

    /// Sets `key` in `section` to `value`. Returns whether the text changed. An existing line
    /// keeps its place (repeats of the key are removed); a new one goes at the end of its
    /// section.
    pub fn set(&mut self, section: &str, key: &str, value: &str) -> bool {
        let at = self.key_lines(section, key);
        if let Some((&first, rest)) = at.split_first() {
            let mut changed = false;
            for &i in rest.iter().rev() {
                self.lines.remove(i);
                changed = true;
            }
            if let Line::Key { value: old, .. } = &self.lines[first]
                && old == value
            {
                return changed;
            }
            let key = match &self.lines[first] {
                Line::Key { key, .. } => key.clone(),
                _ => key.to_owned(),
            };
            self.lines[first] = self.key_line(&key, value);
            return true;
        }
        let line = self.key_line(key, value);
        match self.section_end(section) {
            Some(end) => self.lines.insert(end, line),
            None => {
                if !section.is_empty() {
                    self.lines.push(Line::Section {
                        name: section.to_owned(),
                        text: format!("[{section}]"),
                    });
                }
                self.lines.push(line);
            }
        }
        true
    }

    /// Removes `key` from `section`. Returns whether it was there. The section header stays.
    pub fn remove(&mut self, section: &str, key: &str) -> bool {
        let at = self.key_lines(section, key);
        for &i in at.iter().rev() {
            self.lines.remove(i);
        }
        !at.is_empty()
    }

    fn same(&self, a: &str, b: &str) -> bool {
        match self.style {
            Style::Ini => a.eq_ignore_ascii_case(b),
            Style::Cfg => a == b,
        }
    }

    /// Indexes of the lines holding `key` in `section`.
    fn key_lines(&self, section: &str, key: &str) -> Vec<usize> {
        let mut current = "";
        let mut found = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            match line {
                Line::Section { name, .. } => current = name,
                Line::Key { key: k, .. } if self.same(current, section) && self.same(k, key) => {
                    found.push(i);
                }
                _ => {}
            }
        }
        found
    }

    /// Where a new key of `section` goes: after the section's last key (or its header), so a
    /// comment introducing the next section stays with it. `None` if the section doesn't
    /// exist (the unnamed section always exists).
    fn section_end(&self, section: &str) -> Option<usize> {
        let mut current: Option<&str> = Some("");
        let mut end = None;
        for (i, line) in self.lines.iter().enumerate() {
            if let Line::Section { name, .. } = line {
                current = Some(name);
                if self.same(name, section) {
                    end = Some(i + 1);
                }
                continue;
            }
            if current.is_some_and(|c| self.same(c, section)) && matches!(line, Line::Key { .. }) {
                end = Some(i + 1);
            }
        }
        if section.is_empty() {
            Some(end.unwrap_or(0))
        } else {
            end
        }
    }

    fn key_line(&self, key: &str, value: &str) -> Line {
        let text = match self.style {
            Style::Ini => format!("{key} = {value}"),
            Style::Cfg => format!("{key} = \"{value}\""),
        };
        Line::Key {
            key: key.to_owned(),
            value: value.to_owned(),
            text,
        }
    }
}

impl std::fmt::Display for ConfigDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let text = match line {
                Line::Raw(text) | Line::Section { text, .. } | Line::Key { text, .. } => text,
            };
            let _ = write!(out, "{text}");
        }
        if self.trailing_newline && !self.lines.is_empty() {
            out.push('\n');
        }
        f.write_str(&out)
    }
}

fn parse_line(line: &str, style: Style) -> Line {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
        return Line::Raw(line.to_owned());
    }
    if style == Style::Ini
        && let Some(name) = trimmed.strip_prefix('[').and_then(|r| r.strip_suffix(']'))
    {
        return Line::Section {
            name: name.trim().to_owned(),
            text: line.to_owned(),
        };
    }
    let Some((key, value)) = trimmed.split_once('=') else {
        return Line::Raw(line.to_owned());
    };
    let key = key.trim();
    if key.is_empty() {
        return Line::Raw(line.to_owned());
    }
    let value = value.trim();
    let value = match style {
        Style::Cfg => value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value),
        Style::Ini => value,
    };
    Line::Key {
        key: key.to_owned(),
        value: value.to_owned(),
        text: line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::tempdir;

    const INI: &str = "# A comment\n[Display]\nFullscreen = False\n  ; odd = line\n\n[Interface]\nweird line without equals\nConfirmStop = True\n";

    #[test]
    fn untouched_text_is_identical() {
        for text in [INI, "", "no newline = 1", "a = \"b\"\n# c\n\n"] {
            assert_eq!(ConfigDoc::parse(text, Style::Ini).to_string(), text);
            assert_eq!(ConfigDoc::parse(text, Style::Cfg).to_string(), text);
        }
    }

    #[test]
    fn set_rewrites_only_its_line() {
        let mut doc = ConfigDoc::parse(INI, Style::Ini);
        assert!(doc.set("display", "fullscreen", "True"));
        assert_eq!(
            doc.to_string(),
            INI.replace("Fullscreen = False", "Fullscreen = True")
        );
        // Same value: nothing changes.
        assert!(!doc.set("Display", "Fullscreen", "True"));
    }

    #[test]
    fn new_keys_go_at_the_end_of_their_section() {
        let mut doc = ConfigDoc::parse(INI, Style::Ini);
        doc.set("Display", "RenderToMain", "False");
        doc.set("Analytics", "Enabled", "False");
        doc.set("Interface", "SkipNKitWarning", "True");
        assert_eq!(
            doc.to_string(),
            "# A comment\n[Display]\nFullscreen = False\nRenderToMain = False\n  ; odd = line\n\n\
             [Interface]\nweird line without equals\nConfirmStop = True\nSkipNKitWarning = True\n\
             [Analytics]\nEnabled = False\n"
        );
        assert_eq!(doc.get("analytics", "enabled"), Some("False"));
    }

    #[test]
    fn remove_and_repeats() {
        let mut doc = ConfigDoc::parse("[A]\nx = 1\ny = 2\nx = 3\n", Style::Ini);
        assert_eq!(doc.get("A", "x"), Some("3"));
        assert!(doc.set("A", "x", "4"));
        assert_eq!(doc.to_string(), "[A]\nx = 4\ny = 2\n");
        assert!(doc.remove("A", "y"));
        assert!(!doc.remove("A", "y"));
        assert_eq!(doc.to_string(), "[A]\nx = 4\n");
    }

    #[test]
    fn cfg_quotes_and_is_case_sensitive() {
        let mut doc = ConfigDoc::parse("# mine\nvideo_driver = \"gl\"\nFoo = bar\n", Style::Cfg);
        assert_eq!(doc.get("", "video_driver"), Some("gl"));
        assert_eq!(doc.get("", "Foo"), Some("bar"));
        assert_eq!(doc.get("", "foo"), None);
        doc.set("", "video_driver", "vulkan");
        doc.set("", "new_key", "1");
        assert_eq!(
            doc.to_string(),
            "# mine\nvideo_driver = \"vulkan\"\nFoo = bar\nnew_key = \"1\"\n"
        );
    }

    #[test]
    fn save_is_atomic_and_load_tolerates_missing_files() {
        let dir = tempdir();
        let path = dir.path().join("Config/GFX.ini");
        let mut doc = ConfigDoc::load(&path, Style::Ini).unwrap();
        doc.set("Settings", "InternalResolution", "2");
        doc.save(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[Settings]\nInternalResolution = 2\n"
        );
        assert!(!path.with_extension("ini.tmp").exists());
    }
}

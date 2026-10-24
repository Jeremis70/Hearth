//! Which music plays, from what Hearth is showing. Pure rules; the shell describes the scene
//! after every change and [`MusicDirector`] tells the engine only when the answer changes.
//!
//! - A game running and not frozen (playing, starting, or left running because its suspend
//!   policy is off): silence. The game is the one making sound.
//! - The overlay over a frozen game: the ambient, only with "Music in overlay" (it carries on
//!   where it left off; game themes belong to browsing Home).
//! - Hearth's screens: the settled focused game's theme (with "Game themes"), else the ambient.

use std::time::Duration;

use crate::{Audio, Music, SoundFile};

/// Fading in from silence.
pub const FADE_IN: Duration = Duration::from_millis(300);
/// Going silent when a game starts (it takes far longer than this to make sound).
pub const FADE_LAUNCH: Duration = Duration::from_millis(300);
/// Going silent on Resume, in parallel with the game continuing.
pub const FADE_RESUME: Duration = Duration::from_millis(150);
/// Whatever is still fading when a launched game takes the screen.
pub const FADE_FOCUS: Duration = Duration::from_millis(20);
/// Ambient ↔ theme.
pub const CROSSFADE: Duration = Duration::from_millis(800);

/// The running game, as far as music is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameScene {
    /// Nothing running.
    None,
    /// Running and audible.
    Running,
    /// Frozen (SIGSTOP), silent.
    Frozen,
}

/// What Hearth shows, for the music rules.
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    pub game: GameScene,
    /// The overlay menu is up.
    pub overlay: bool,
    /// The ambient loop (pack, else bundled), if any.
    pub ambient: Option<&'a SoundFile>,
    /// The theme of the game focused (and settled) on Home.
    pub focused_theme: Option<&'a SoundFile>,
    /// A file previewed on the Customize page: wins outside a running game and the overlay,
    /// even with game themes off (it was asked for).
    pub preview: Option<&'a SoundFile>,
    pub music_in_overlay: bool,
    pub game_themes: bool,
}

/// The music for a scene.
pub fn choose(scene: &Scene<'_>) -> Option<Music> {
    let track = |kind: &str, file: &SoundFile| Music {
        key: track_key(kind, file),
        file: file.clone(),
        resume: false,
    };
    let theme = |file: Option<&SoundFile>| {
        file.filter(|_| scene.game_themes)
            .map(|file| track("theme", file))
    };
    let ambient = || {
        scene.ambient.map(|file| Music {
            key: "ambient".into(),
            file: file.clone(),
            resume: true,
        })
    };
    if scene.game == GameScene::Running {
        return None;
    }
    if scene.overlay {
        if !scene.music_in_overlay {
            return None;
        }
        return ambient();
    }
    if let Some(preview) = scene.preview {
        return Some(track("preview", preview));
    }
    theme(scene.focused_theme).or_else(ambient)
}

/// `theme:<path>`, plus `@<ms>` for a "Start at" offset (a new offset is a new track, so it
/// restarts there).
fn track_key(kind: &str, file: &SoundFile) -> String {
    match file.auto_loop.as_ref().map(|a| a.offset_ms) {
        Some(offset) if offset > 0 => format!("{kind}:{}@{offset}", file.path.display()),
        _ => format!("{kind}:{}", file.path.display()),
    }
}

/// Sends the engine the music for each new scene, with the right fade.
#[derive(Debug, Default)]
pub struct MusicDirector {
    current: Option<Music>,
}

impl MusicDirector {
    /// Applies `scene`. `silence_fade` is how fast to go quiet if it is silent (launch or
    /// resume); fading in and crossfading use [`FADE_IN`] and [`CROSSFADE`].
    pub fn update(&mut self, audio: &Audio, scene: &Scene<'_>, silence_fade: Duration) {
        let next = choose(scene);
        if next == self.current {
            return;
        }
        let fade = match (&self.current, &next) {
            (_, None) => silence_fade,
            (None, Some(_)) => FADE_IN,
            (Some(_), Some(_)) => CROSSFADE,
        };
        tracing::debug!(from = ?self.current.as_ref().map(|m| &m.key), to = ?next.as_ref().map(|m| &m.key), ?fade, "music");
        audio.set_music(next.clone(), fade);
        self.current = next;
    }

    /// Silence asked for and still fading: finish within `fade` (a game taking the screen).
    pub fn hurry(&self, audio: &Audio, fade: Duration) {
        if self.current.is_none() {
            audio.set_music(None, fade);
        }
    }

    /// The music asked for last.
    pub fn current(&self) -> Option<&Music> {
        self.current.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> SoundFile {
        SoundFile {
            path: name.into(),
            origin: "test".into(),
            loop_spec: None,
            auto_loop: None,
            gain_db: 0,
        }
    }

    #[test]
    fn music_follows_the_rules() {
        let ambient = file("ambient.ogg");
        let oot = file("oot-theme.ogg");
        let home = Scene {
            game: GameScene::None,
            overlay: false,
            ambient: Some(&ambient),
            focused_theme: None,
            preview: None,
            music_in_overlay: true,
            game_themes: true,
        };
        let key = |s: &Scene<'_>| choose(s).map(|m| m.key);
        assert_eq!(key(&home).as_deref(), Some("ambient"));
        // A focused game with a theme.
        let focused = Scene {
            focused_theme: Some(&oot),
            ..home
        };
        assert_eq!(key(&focused).as_deref(), Some("theme:oot-theme.ogg"));
        assert_eq!(
            key(&Scene {
                game_themes: false,
                ..focused
            })
            .as_deref(),
            Some("ambient")
        );
        // A game playing (or starting): silent, whatever is focused.
        assert_eq!(
            key(&Scene {
                game: GameScene::Running,
                ..focused
            }),
            None
        );
        // Home over a frozen game: Hearth's music again.
        assert_eq!(
            key(&Scene {
                game: GameScene::Frozen,
                ..home
            })
            .as_deref(),
            Some("ambient")
        );
        // The overlay over a frozen game: the ambient, even when a game with a theme is
        // focused on Home behind it; nothing when turned off.
        let overlay = Scene {
            game: GameScene::Frozen,
            overlay: true,
            ..focused
        };
        assert_eq!(key(&overlay).as_deref(), Some("ambient"));
        assert_eq!(
            key(&Scene {
                music_in_overlay: false,
                ..overlay
            }),
            None
        );
        // The overlay over a game that keeps running (suspend off): silent.
        assert_eq!(
            key(&Scene {
                game: GameScene::Running,
                ..overlay
            }),
            None
        );
        // A preview wins on Hearth's screens, even with themes off, but never over a game or
        // in the overlay; a new start offset is a new track.
        let mut song = file("song.mp3");
        let previewing = Scene {
            preview: Some(&song),
            game_themes: false,
            ..focused
        };
        assert_eq!(key(&previewing).as_deref(), Some("preview:song.mp3"));
        assert_eq!(
            key(&Scene {
                overlay: true,
                game: GameScene::Frozen,
                ..previewing
            })
            .as_deref(),
            Some("ambient")
        );
        song.auto_loop = Some(crate::AutoLoop {
            offset_ms: 12_000,
            cache_dir: None,
        });
        assert_eq!(
            key(&Scene {
                preview: Some(&song),
                ..home
            })
            .as_deref(),
            Some("preview:song.mp3@12000")
        );
        // No ambient anywhere: silence rather than an error.
        assert_eq!(
            key(&Scene {
                ambient: None,
                ..home
            }),
            None
        );
    }
}

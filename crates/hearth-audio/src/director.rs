//! Which music plays, from what Hearth is showing. Pure rules; the shell describes the scene
//! after every change and [`MusicDirector`] tells the engine only when the answer changes.
//!
//! - A game running and not frozen (playing, starting, or left running because its suspend
//!   policy is off): silence. The game is the one making sound.
//! - The overlay over a frozen game: only with "Music in overlay"; that game's theme if it has
//!   one (and "Game themes" is on), else the ambient.
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
    /// The theme of the running game.
    pub running_theme: Option<&'a SoundFile>,
    pub music_in_overlay: bool,
    pub game_themes: bool,
}

/// The music for a scene.
pub fn choose(scene: &Scene<'_>) -> Option<Music> {
    let theme = |file: Option<&SoundFile>| {
        file.filter(|_| scene.game_themes).map(|file| Music {
            key: format!("theme:{}", file.path.display()),
            file: file.clone(),
            resume: false,
        })
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
        return theme(scene.running_theme).or_else(ambient);
    }
    theme(scene.focused_theme).or_else(ambient)
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
        }
    }

    #[test]
    fn music_follows_the_rules() {
        let ambient = file("ambient.ogg");
        let oot = file("oot-theme.ogg");
        let tp = file("tp-theme.ogg");
        let home = Scene {
            game: GameScene::None,
            overlay: false,
            ambient: Some(&ambient),
            focused_theme: None,
            running_theme: None,
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
        // The overlay over a frozen game: its theme, else the ambient; nothing when turned off.
        let overlay = Scene {
            game: GameScene::Frozen,
            overlay: true,
            running_theme: Some(&tp),
            ..focused
        };
        assert_eq!(key(&overlay).as_deref(), Some("theme:tp-theme.ogg"));
        assert_eq!(
            key(&Scene {
                running_theme: None,
                ..overlay
            })
            .as_deref(),
            Some("ambient")
        );
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

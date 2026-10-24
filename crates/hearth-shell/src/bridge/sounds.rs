//! UI sounds: the one place they are chosen. Both windows' dispatchers report every action with
//! what it did (`Feedback.input(action, NavResult)`), and this picks the sound:
//!
//! - `moved` → `move`, `edge` → `edge_bump` (at most one every [`MOVE_INTERVAL`] /
//!   [`EDGE_INTERVAL`], so a held D-pad repeating every 80 ms ticks steadily instead of
//!   machine-gunning; a new one cuts the last with a short fade in the engine);
//! - `handled` Accept → `accept`, `handled` Back → `back` (pad actions fire on release);
//! - overlay open/close come from the session ([`play`]).
//!
//! Focus set by code never comes through here, so it never sounds.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hearth_audio::{Audio, SoundFile};
use hearth_core::config::PackCategory;
use hearth_core::packs::{MUSIC_NAMES, Origin, Resolved, Resolver, SOUND_NAMES};
use hearth_platform::input::dev_socket::CommandHandler;

use crate::{NavResult, UiAction};

/// Shortest gap between two `move` sounds.
pub const MOVE_INTERVAL: Duration = Duration::from_millis(130);
/// Shortest gap between two `edge_bump` sounds.
pub const EDGE_INTERVAL: Duration = Duration::from_millis(250);
/// An input stamp older than this belongs to another action (keyboard input has none).
const STAMP_AGE: Duration = Duration::from_millis(500);

/// When the last controller action reached the UI (set by the input bridge).
static LAST_INPUT: Mutex<Option<Instant>> = Mutex::new(None);

thread_local! {
    static SOUNDS: RefCell<Option<UiSounds>> = const { RefCell::new(None) };
}

struct UiSounds {
    audio: Audio,
    resolver: Resolver,
    last_move: Option<Instant>,
    last_edge: Option<Instant>,
}

/// The controller action about to be injected arrived at `at`.
pub fn note_input(at: Instant) {
    if let Ok(mut last) = LAST_INPUT.lock() {
        *last = Some(at);
    }
}

fn input_time() -> Instant {
    let now = Instant::now();
    LAST_INPUT
        .lock()
        .ok()
        .and_then(|mut last| last.take())
        .filter(|at| now.duration_since(*at) < STAMP_AGE)
        .unwrap_or(now)
}

/// A resolved file as the engine wants it.
pub fn sound_file(resolved: Resolved) -> SoundFile {
    SoundFile {
        path: resolved.path,
        origin: match resolved.origin {
            Origin::Pack(id) => format!("pack:{id}"),
            Origin::Default => "default".into(),
        },
        loop_spec: resolved.loop_spec,
        gain_db: resolved.gain_db,
    }
}

/// The UI sounds (one-shots) of the selected pack, falling back file by file.
fn one_shots(resolver: &Resolver) -> Vec<(String, Option<SoundFile>)> {
    SOUND_NAMES
        .iter()
        .filter(|name| !MUSIC_NAMES.contains(name))
        .map(|&name| {
            let file = resolver
                .resolve_or_log(&format!("sounds/{name}"))
                .map(sound_file);
            (name.to_owned(), file)
        })
        .collect()
}

/// Starts the UI sounds over `resolver` (its sound pack selection) and decodes them.
pub fn install(audio: Audio, resolver: Resolver) {
    audio.set_sounds(one_shots(&resolver));
    SOUNDS.with(|cell| {
        *cell.borrow_mut() = Some(UiSounds {
            audio,
            resolver,
            last_move: None,
            last_edge: None,
        });
    });
}

fn with<R>(f: impl FnOnce(&mut UiSounds) -> R) -> Option<R> {
    SOUNDS.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Another sound pack (`None`: Hearth's defaults): the sounds are decoded again.
pub fn select_pack(pack: Option<&str>) {
    with(|s| {
        s.resolver.select(PackCategory::UiSounds, pack);
        s.audio.set_sounds(one_shots(&s.resolver));
    });
}

/// A music file of the selected pack (`ambient_loop`…), falling back to the default.
pub fn music_file(name: &str) -> Option<SoundFile> {
    with(|s| {
        s.resolver
            .resolve_or_log(&format!("sounds/{name}"))
            .map(sound_file)
    })
    .flatten()
}

/// Plays a sound now (overlay, dev socket, the volume preview).
pub fn play(name: &str) {
    with(|s| s.audio.play(name, None));
}

/// Dev socket commands: `sound:<name>` plays any logical name (`achievement` included),
/// `audio_state` replies with the engine's state as JSON.
pub fn dev_commands(audio: Audio) -> CommandHandler {
    Arc::new(move |word: &str| {
        if let Some(name) = word.strip_prefix("sound:") {
            audio.play(name, Some(Instant::now()));
            return Some(format!("playing {name}"));
        }
        if word == "audio_state" {
            let reply = match audio.state(Duration::from_secs(1)) {
                Some(state) => serde_json::to_string_pretty(&state)
                    .unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}")),
                None => "{\"error\": \"the audio thread didn't answer\"}".into(),
            };
            return Some(reply);
        }
        None
    })
}

/// Picks the sound for an action and what it did (see the module docs).
pub fn on_input(action: UiAction, result: NavResult) {
    let name = sound_for(action, result);
    let Some(name) = name else {
        return;
    };
    let at = input_time();
    with(|s| {
        let (last, interval) = match name {
            "move" => (&mut s.last_move, MOVE_INTERVAL),
            "edge_bump" => (&mut s.last_edge, EDGE_INTERVAL),
            _ => {
                s.audio.play(name, Some(at));
                return;
            }
        };
        if last.is_some_and(|t| at.saturating_duration_since(t) < interval) {
            return;
        }
        *last = Some(at);
        s.audio.play(name, Some(at));
    });
}

/// The sound for an action's result, if any.
pub fn sound_for(action: UiAction, result: NavResult) -> Option<&'static str> {
    match result {
        NavResult::Moved => Some("move"),
        NavResult::Edge => Some("edge_bump"),
        NavResult::Handled => match action {
            UiAction::Accept => Some("accept"),
            UiAction::Back => Some("back"),
            _ => None,
        },
        NavResult::Ignored | NavResult::Quiet => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_pick_sounds() {
        assert_eq!(sound_for(UiAction::Left, NavResult::Moved), Some("move"));
        assert_eq!(
            sound_for(UiAction::Right, NavResult::Edge),
            Some("edge_bump")
        );
        assert_eq!(
            sound_for(UiAction::Accept, NavResult::Handled),
            Some("accept")
        );
        assert_eq!(sound_for(UiAction::Back, NavResult::Handled), Some("back"));
        assert_eq!(sound_for(UiAction::Back, NavResult::Quiet), None);
        assert_eq!(sound_for(UiAction::Options, NavResult::Handled), None);
        assert_eq!(sound_for(UiAction::Accept, NavResult::Ignored), None);
    }
}

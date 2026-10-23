//! Turns controller [`Action`]s into Slint key events, so the UI only ever handles keys and the
//! keyboard behaves exactly like a controller. The key table lives in `ui/input.slint`.
//! The session controller decides which window gets them (or drops them while a game is in
//! front); the Guide button always reaches it.

use std::sync::mpsc;
use std::thread;

use anyhow::Context;
use hearth_platform::input::{Action, InputBackend};
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};

use super::session::{self, Route};

/// Starts every backend (each failure is logged and skipped) and forwards their actions to the
/// UI from one bridge thread.
pub fn start(backends: Vec<Box<dyn InputBackend>>) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut running = Vec::new();
    for mut backend in backends {
        match backend.start(tx.clone()) {
            Ok(()) => running.push(backend),
            Err(err) => tracing::warn!("input backend unavailable: {err}"),
        }
    }
    drop(tx);
    anyhow::ensure!(!running.is_empty(), "no input backend started");
    thread::Builder::new()
        .name("hearth-input-bridge".into())
        .spawn(move || {
            // The backends live as long as the bridge.
            let _backends = running;
            for action in rx {
                tracing::debug!(?action, "controller action");
                let at = std::time::Instant::now();
                if slint::invoke_from_event_loop(move || dispatch(action, at)).is_err() {
                    break;
                }
            }
        })
        .context("spawning input bridge thread")?;
    Ok(())
}

fn dispatch(action: Action, at: std::time::Instant) {
    super::sounds::note_input(at);
    // Capture is the session's alone (`route` handles it); it has no key.
    match session::route(action) {
        Route::Main(app) => inject(app.window(), action),
        Route::Overlay(overlay) => inject(overlay.window(), action),
        Route::None => {}
    }
}

/// The key each action is injected as; must be the first key for it in `ui/input.slint`.
/// `None`: the action never reaches the UI.
fn key_for(action: Action) -> Option<Key> {
    Some(match action {
        Action::Up => Key::UpArrow,
        Action::Down => Key::DownArrow,
        Action::Left => Key::LeftArrow,
        Action::Right => Key::RightArrow,
        Action::Accept => Key::Return,
        Action::Back => Key::Escape,
        Action::PrevTab => Key::PageUp,
        Action::NextTab => Key::PageDown,
        Action::Options => Key::Menu,
        Action::HomeShort => Key::F1,
        Action::HomeLong => Key::F2,
        Action::Capture => return None,
    })
}

fn inject(window: &slint::Window, action: Action) {
    let Some(key) = key_for(action) else {
        return;
    };
    let text: slint::SharedString = key.into();
    super::frames::input();
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_its_own_key() {
        let actions = [
            Action::Up,
            Action::Down,
            Action::Left,
            Action::Right,
            Action::Accept,
            Action::Back,
            Action::PrevTab,
            Action::NextTab,
            Action::Options,
            Action::HomeShort,
            Action::HomeLong,
        ];
        let mut keys: Vec<slint::SharedString> = actions
            .iter()
            .filter_map(|&a| key_for(a))
            .map(Into::into)
            .collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), actions.len());
    }
}

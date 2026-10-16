//! Turns controller [`Action`]s into Slint key events, so the UI only ever handles keys and the
//! keyboard behaves exactly like a controller. The key table lives in `ui/input.slint`.

use std::sync::mpsc;
use std::thread;

use anyhow::Context;
use hearth_platform::input::{Action, InputBackend};
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};

use crate::AppWindow;

/// Starts `backend` and forwards its actions to the window from a bridge thread.
pub fn start(app: &AppWindow, mut backend: impl InputBackend + 'static) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel();
    backend.start(tx).context("starting input backend")?;
    let weak = app.as_weak();
    thread::Builder::new()
        .name("hearth-input-bridge".into())
        .spawn(move || {
            // The backend lives as long as the bridge.
            let _backend = backend;
            for action in rx {
                tracing::debug!(?action, "controller action");
                if weak
                    .upgrade_in_event_loop(move |app| inject(&app, action))
                    .is_err()
                {
                    break;
                }
            }
        })
        .context("spawning input bridge thread")?;
    Ok(())
}

/// The key each action is injected as; must be the first key for it in `ui/input.slint`.
fn key_for(action: Action) -> Key {
    match action {
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
    }
}

fn inject(app: &AppWindow, action: Action) {
    let text: slint::SharedString = key_for(action).into();
    let window = app.window();
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
        let mut keys: Vec<slint::SharedString> =
            actions.iter().map(|&a| key_for(a).into()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), actions.len());
    }
}

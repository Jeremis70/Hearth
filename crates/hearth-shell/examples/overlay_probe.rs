//! Spike for the in-game overlay: shows the transparent `OverlayWindow` fullscreen and marks
//! it as gamescope's overlay (`STEAM_OVERLAY`). Run it against a nested gamescope's Xwayland:
//!
//! ```text
//! # gamescope must run with -e (Steam mode)
//! env -u WAYLAND_DISPLAY DISPLAY=:1 cargo run -p hearth-shell --example overlay_probe
//! ```

use std::time::Duration;

use hearth_platform::gamescope::{Compositor, Gamescope};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;

slint::include_modules!();

fn x11_id(window: &slint::Window) -> Option<u32> {
    match window.window_handle().window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .select()?;
    let gamescope = Gamescope::connect()?.ok_or("DISPLAY is not gamescope's Xwayland")?;
    let overlay = OverlayWindow::new()?;
    overlay.set_game_title("Overlay spike".into());
    overlay.window().set_fullscreen(true);
    overlay.show()?;

    let weak = overlay.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            let Some(overlay) = weak.upgrade() else {
                return;
            };
            if let Some(id) = x11_id(overlay.window()) {
                let shown = gamescope.set_overlay(id, true);
                let focused = gamescope.focused_app();
                tracing::info!(window = id, ?shown, ?focused, "overlay window");
            }
        },
    );
    slint::run_event_loop_until_quit()?;
    Ok(())
}

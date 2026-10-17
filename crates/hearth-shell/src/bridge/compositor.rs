//! Connects Slint windows to the gamescope [`Compositor`].

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use hearth_platform::gamescope::{Compositor, SHELL_APP_ID};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;

/// How often and how long to wait for winit to create a window.
const POLL: Duration = Duration::from_millis(50);
const MAX_ATTEMPTS: u32 = 100;

/// The X11 id of a Slint window, once winit has created it (None on Wayland or before that).
pub fn x11_window_id(window: &slint::Window) -> Option<u32> {
    let handle = window.window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

/// Calls `ready` with the window's X11 id as soon as it exists (winit creates windows lazily
/// once the event loop runs), or with `None` after ~5 s.
pub fn when_x11_window<C: ComponentHandle + 'static>(
    window: slint::Weak<C>,
    ready: impl Fn(Option<u32>) + 'static,
) {
    poll_window(window, Rc::new(ready), 0);
}

fn poll_window<C: ComponentHandle + 'static>(
    window: slint::Weak<C>,
    ready: Rc<dyn Fn(Option<u32>)>,
    attempt: u32,
) {
    slint::Timer::single_shot(POLL, move || {
        let Some(component) = window.upgrade() else {
            return;
        };
        match x11_window_id(component.window()) {
            Some(id) => ready(Some(id)),
            None if attempt + 1 >= MAX_ATTEMPTS => ready(None),
            None => poll_window(window, ready, attempt + 1),
        }
    });
}

/// Tags Hearth's main window as the shell app and puts it on top.
pub fn register_shell(app: &crate::AppWindow, compositor: Arc<dyn Compositor>) {
    if !compositor.is_active() {
        return;
    }
    when_x11_window(app.as_weak(), move |window| {
        let result = compositor
            .set_shell_window(window, std::process::id())
            .and_then(|()| compositor.focus(&[SHELL_APP_ID]));
        match result {
            Ok(()) => tracing::info!(?window, "shell window registered with gamescope"),
            Err(err) => tracing::warn!("registering shell window failed: {err}"),
        }
    });
}

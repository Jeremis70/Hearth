//! Startup timing: logs, once each, when the steps between launch and art on screen happen,
//! in ms since the window's first frame. A step that changes the screen is logged when the
//! next frame has been drawn, not when the data arrived.
//!
//! `RUST_LOG=hearth_shell::bridge::startup=info` (on by default at `info`).

use std::sync::Mutex;
use std::time::Instant;

use slint::ComponentHandle;

use crate::AppWindow;

#[derive(Debug, Default)]
struct Probe {
    start: Option<Instant>,
    shown: Option<Instant>,
    logged: Vec<&'static str>,
    /// Steps waiting for the next frame.
    pending: Vec<(&'static str, Instant)>,
}

static PROBE: Mutex<Probe> = Mutex::new(Probe {
    start: None,
    shown: None,
    logged: Vec::new(),
    pending: Vec::new(),
});

fn probe() -> std::sync::MutexGuard<'static, Probe> {
    PROBE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn ms(from: Option<Instant>, to: Instant) -> i64 {
    from.map_or(-1, |from| {
        i64::try_from(to.duration_since(from).as_millis()).unwrap_or(i64::MAX)
    })
}

/// Call first thing in `main`.
pub fn start() {
    probe().start = Some(Instant::now());
}

/// Hooks the window's frames: the first one is "window shown".
pub fn install(app: &AppWindow) {
    let result = app.window().set_rendering_notifier(|state, _| match state {
        slint::RenderingState::BeforeRendering => super::frames::before_rendering(),
        slint::RenderingState::AfterRendering => {
            frame_drawn();
            super::frames::after_rendering();
        }
        _ => {}
    });
    if let Err(err) = result {
        tracing::warn!("startup timing unavailable: {err:?}");
    }
}

fn frame_drawn() {
    let now = Instant::now();
    let mut p = probe();
    if p.shown.is_none() {
        p.shown = Some(now);
        tracing::info!(since_launch_ms = ms(p.start, now), "startup: window shown");
    }
    for (step, arrived) in std::mem::take(&mut p.pending) {
        tracing::info!(
            ms = ms(p.shown, now),
            data_ms = ms(p.shown, arrived),
            "startup: {step} on screen"
        );
    }
}

/// A step without anything to draw (e.g. "library loaded"); any thread.
pub fn note(step: &'static str) {
    let now = Instant::now();
    let mut p = probe();
    if p.logged.contains(&step) {
        return;
    }
    p.logged.push(step);
    tracing::info!(ms = ms(p.shown, now), "startup: {step}");
}

/// A step whose data is now in the UI; logged after the next frame (UI thread).
pub fn shown(step: &'static str, app: &AppWindow) {
    let mut p = probe();
    if p.logged.contains(&step) {
        return;
    }
    p.logged.push(step);
    p.pending.push((step, Instant::now()));
    drop(p);
    app.window().request_redraw();
}

//! Frame statistics for chasing lag, on with `HEARTH_FRAME_STATS=1`: once a second, the frames
//! drawn, how long drawing took, how long an input took to reach the screen (key injected →
//! end of the next frame drawn), and counters bumped by the code (`count`).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default)]
struct Stats {
    since: Option<Instant>,
    frame_start: Option<Instant>,
    frames: u32,
    render: Vec<Duration>,
    /// Inputs waiting for the next frame to finish.
    inputs: Vec<Instant>,
    latency: Vec<Duration>,
    counters: BTreeMap<&'static str, u32>,
}

static STATS: Mutex<Option<Stats>> = Mutex::new(None);

fn with(f: impl FnOnce(&mut Stats)) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut guard = STATS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(guard.get_or_insert_with(Stats::default));
}

/// Reads `HEARTH_FRAME_STATS`.
pub fn init() {
    if std::env::var_os("HEARTH_FRAME_STATS").is_some_and(|v| v == "1") {
        ENABLED.store(true, Ordering::Relaxed);
        tracing::info!("frame statistics on");
    }
}

/// A key was injected (a controller action).
pub fn input() {
    with(|s| s.inputs.push(Instant::now()));
}

/// Bumps a named counter (e.g. model rows updated).
pub fn count(what: &'static str, n: u32) {
    with(|s| *s.counters.entry(what).or_default() += n);
}

pub(super) fn before_rendering() {
    with(|s| s.frame_start = Some(Instant::now()));
}

pub(super) fn after_rendering() {
    with(|s| {
        let now = Instant::now();
        s.frames += 1;
        if let Some(start) = s.frame_start.take() {
            s.render.push(now - start);
        }
        for input in std::mem::take(&mut s.inputs) {
            tracing::debug!(ms = ms(now - input), "input on screen");
            s.latency.push(now - input);
        }
        let since = *s.since.get_or_insert(now);
        if now - since >= Duration::from_secs(1) {
            report(s, now - since);
            *s = Stats {
                since: Some(now),
                ..Stats::default()
            };
        }
    });
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

fn summary(values: &[Duration]) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    let total: Duration = values.iter().sum();
    let max = values.iter().max().copied().unwrap_or_default();
    #[allow(clippy::cast_possible_truncation)]
    let mean = total / values.len() as u32;
    format!("mean {} ms, max {} ms", ms(mean), ms(max))
}

fn report(s: &Stats, span: Duration) {
    let counters: Vec<String> = s.counters.iter().map(|(k, v)| format!("{k}={v}")).collect();
    tracing::info!(
        "frames: {} in {} ms; draw {}; input→screen {} ({} inputs); {}",
        s.frames,
        span.as_millis(),
        summary(&s.render),
        summary(&s.latency),
        s.latency.len(),
        counters.join(" ")
    );
}

//! Pure timing logic shared by input backends: direction merging, key repeat and Guide
//! short/long press. Every function takes the current time, so it is testable without devices.

use std::time::{Duration, Instant};

use super::{Action, Direction};

/// Delay before a held direction starts repeating.
pub const REPEAT_DELAY: Duration = Duration::from_millis(400);
/// Interval between repeats once repeating.
pub const REPEAT_RATE: Duration = Duration::from_millis(80);
/// Guide held this long is a long press.
pub const LONG_PRESS: Duration = Duration::from_millis(500);
/// Stick deflection needed to count as a direction.
pub const STICK_DEADZONE: f32 = 0.5;

/// Maps a stick position to a direction along its dominant axis.
///
/// `y` is positive upwards (gilrs convention). Returns `None` inside the deadzone.
pub fn stick_direction(x: f32, y: f32, deadzone: f32) -> Option<Direction> {
    if x.abs().max(y.abs()) < deadzone {
        return None;
    }
    Some(if x.abs() > y.abs() {
        if x > 0.0 {
            Direction::Right
        } else {
            Direction::Left
        }
    } else if y > 0.0 {
        Direction::Up
    } else {
        Direction::Down
    })
}

/// Where a held direction comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirectionSource {
    /// D-pad reported as buttons.
    DPadButton,
    /// D-pad reported as hat axes.
    DPadAxis,
    /// Left analog stick.
    Stick,
}

/// Merges D-pad and stick input; the most recently pressed direction wins.
#[derive(Debug, Clone, Default)]
pub struct HeldDirections {
    stack: Vec<(DirectionSource, Direction)>,
}

impl HeldDirections {
    /// Records a direction pressed on a button-like source.
    pub fn press(&mut self, source: DirectionSource, dir: Direction) {
        self.release(source, dir);
        self.stack.push((source, dir));
    }

    /// Records a direction released on a button-like source.
    pub fn release(&mut self, source: DirectionSource, dir: Direction) {
        self.stack.retain(|&entry| entry != (source, dir));
    }

    /// Sets the single direction of an axis-like source (stick, hat), replacing its previous one.
    pub fn set_axis(&mut self, source: DirectionSource, dir: Option<Direction>) {
        if self
            .stack
            .iter()
            .any(|&(s, d)| s == source && Some(d) == dir)
        {
            return;
        }
        self.stack.retain(|&(s, _)| s != source);
        if let Some(dir) = dir {
            self.stack.push((source, dir));
        }
    }

    /// The direction currently in effect.
    pub fn current(&self) -> Option<Direction> {
        self.stack.last().map(|&(_, d)| d)
    }
}

/// Key repeat for a held direction: fires on press, after [`REPEAT_DELAY`], then every
/// [`REPEAT_RATE`].
#[derive(Debug, Clone)]
pub struct KeyRepeat {
    delay: Duration,
    rate: Duration,
    held: Option<(Direction, Instant)>,
}

impl Default for KeyRepeat {
    fn default() -> Self {
        Self::new(REPEAT_DELAY, REPEAT_RATE)
    }
}

impl KeyRepeat {
    /// Creates a repeater with custom timings.
    pub fn new(delay: Duration, rate: Duration) -> Self {
        Self {
            delay,
            rate,
            held: None,
        }
    }

    /// Updates the held direction. Returns an action when a new direction starts.
    pub fn set(&mut self, dir: Option<Direction>, now: Instant) -> Option<Action> {
        match (dir, self.held) {
            (Some(d), Some((held, _))) if d == held => None,
            (Some(d), _) => {
                self.held = Some((d, now + self.delay));
                Some(d.into())
            }
            (None, _) => {
                self.held = None;
                None
            }
        }
    }

    /// Returns a repeat action if one is due. Late ticks never produce a burst.
    pub fn tick(&mut self, now: Instant) -> Option<Action> {
        let (dir, next) = self.held?;
        if now < next {
            return None;
        }
        let mut following = next + self.rate;
        if following <= now {
            following = now + self.rate;
        }
        self.held = Some((dir, following));
        Some(dir.into())
    }

    /// When the next repeat is due, if a direction is held.
    pub fn deadline(&self) -> Option<Instant> {
        self.held.map(|(_, next)| next)
    }
}

/// Guide button timing: short press on release before [`LONG_PRESS`], long press once held
/// that long (fired while still held, so the user gets feedback without releasing).
#[derive(Debug, Clone)]
pub struct GuidePress {
    threshold: Duration,
    pressed_at: Option<Instant>,
    long_sent: bool,
}

impl Default for GuidePress {
    fn default() -> Self {
        Self::new(LONG_PRESS)
    }
}

impl GuidePress {
    /// Creates a detector with a custom long-press threshold.
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            pressed_at: None,
            long_sent: false,
        }
    }

    /// Guide went down. Repeated presses while held are ignored.
    pub fn press(&mut self, now: Instant) {
        if self.pressed_at.is_none() {
            self.pressed_at = Some(now);
            self.long_sent = false;
        }
    }

    /// Guide went up.
    pub fn release(&mut self, now: Instant) -> Option<Action> {
        let pressed_at = self.pressed_at.take()?;
        if self.long_sent {
            None
        } else if now.duration_since(pressed_at) >= self.threshold {
            Some(Action::HomeLong)
        } else {
            Some(Action::HomeShort)
        }
    }

    /// Fires [`Action::HomeLong`] once the button has been held long enough.
    pub fn tick(&mut self, now: Instant) -> Option<Action> {
        let pressed_at = self.pressed_at?;
        if self.long_sent || now.duration_since(pressed_at) < self.threshold {
            return None;
        }
        self.long_sent = true;
        Some(Action::HomeLong)
    }

    /// When the long press would fire, if Guide is held and it hasn't fired yet.
    pub fn deadline(&self) -> Option<Instant> {
        match (self.pressed_at, self.long_sent) {
            (Some(at), false) => Some(at + self.threshold),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn stick_inside_deadzone_is_neutral() {
        assert_eq!(stick_direction(0.49, -0.3, STICK_DEADZONE), None);
        assert_eq!(stick_direction(0.0, 0.0, STICK_DEADZONE), None);
    }

    #[test]
    fn stick_uses_dominant_axis() {
        assert_eq!(
            stick_direction(0.6, 0.55, STICK_DEADZONE),
            Some(Direction::Right)
        );
        assert_eq!(
            stick_direction(-0.2, 0.9, STICK_DEADZONE),
            Some(Direction::Up)
        );
        assert_eq!(
            stick_direction(0.3, -0.5, STICK_DEADZONE),
            Some(Direction::Down)
        );
        assert_eq!(
            stick_direction(-0.7, 0.1, STICK_DEADZONE),
            Some(Direction::Left)
        );
    }

    #[test]
    fn latest_direction_wins_and_falls_back() {
        let mut held = HeldDirections::default();
        held.press(DirectionSource::DPadButton, Direction::Left);
        held.set_axis(DirectionSource::Stick, Some(Direction::Up));
        assert_eq!(held.current(), Some(Direction::Up));
        held.set_axis(DirectionSource::Stick, None);
        assert_eq!(held.current(), Some(Direction::Left));
        held.release(DirectionSource::DPadButton, Direction::Left);
        assert_eq!(held.current(), None);
    }

    #[test]
    fn unchanged_stick_direction_keeps_its_place() {
        let mut held = HeldDirections::default();
        held.set_axis(DirectionSource::Stick, Some(Direction::Up));
        held.press(DirectionSource::DPadButton, Direction::Left);
        held.set_axis(DirectionSource::Stick, Some(Direction::Up));
        assert_eq!(held.current(), Some(Direction::Left));
    }

    #[test]
    fn repeat_fires_on_press_then_after_delay_then_at_rate() {
        let t0 = Instant::now();
        let mut r = KeyRepeat::default();
        assert_eq!(r.set(Some(Direction::Right), t0), Some(Action::Right));
        assert_eq!(r.set(Some(Direction::Right), t0 + ms(10)), None);
        assert_eq!(r.tick(t0 + ms(399)), None);
        assert_eq!(r.tick(t0 + ms(400)), Some(Action::Right));
        assert_eq!(r.tick(t0 + ms(479)), None);
        assert_eq!(r.tick(t0 + ms(480)), Some(Action::Right));
        assert_eq!(r.tick(t0 + ms(560)), Some(Action::Right));
        assert_eq!(r.deadline(), Some(t0 + ms(640)));
    }

    #[test]
    fn release_stops_repeat() {
        let t0 = Instant::now();
        let mut r = KeyRepeat::default();
        r.set(Some(Direction::Down), t0);
        assert_eq!(r.set(None, t0 + ms(100)), None);
        assert_eq!(r.tick(t0 + ms(1000)), None);
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn changing_direction_restarts_delay() {
        let t0 = Instant::now();
        let mut r = KeyRepeat::default();
        r.set(Some(Direction::Down), t0);
        assert_eq!(r.set(Some(Direction::Up), t0 + ms(300)), Some(Action::Up));
        assert_eq!(r.tick(t0 + ms(400)), None);
        assert_eq!(r.tick(t0 + ms(700)), Some(Action::Up));
    }

    #[test]
    fn late_tick_does_not_burst() {
        let t0 = Instant::now();
        let mut r = KeyRepeat::default();
        r.set(Some(Direction::Left), t0);
        assert_eq!(r.tick(t0 + ms(1000)), Some(Action::Left));
        assert_eq!(r.tick(t0 + ms(1001)), None);
        assert_eq!(r.tick(t0 + ms(1080)), Some(Action::Left));
    }

    #[test]
    fn guide_quick_release_is_short() {
        let t0 = Instant::now();
        let mut g = GuidePress::default();
        g.press(t0);
        assert_eq!(g.tick(t0 + ms(300)), None);
        assert_eq!(g.release(t0 + ms(499)), Some(Action::HomeShort));
    }

    #[test]
    fn guide_held_fires_long_once_while_held() {
        let t0 = Instant::now();
        let mut g = GuidePress::default();
        g.press(t0);
        assert_eq!(g.deadline(), Some(t0 + LONG_PRESS));
        assert_eq!(g.tick(t0 + ms(500)), Some(Action::HomeLong));
        assert_eq!(g.tick(t0 + ms(900)), None);
        assert_eq!(g.release(t0 + ms(1000)), None);
        assert_eq!(g.deadline(), None);
    }

    #[test]
    fn guide_release_at_threshold_without_tick_is_long() {
        let t0 = Instant::now();
        let mut g = GuidePress::default();
        g.press(t0);
        assert_eq!(g.release(t0 + ms(500)), Some(Action::HomeLong));
    }

    #[test]
    fn guide_release_without_press_is_ignored() {
        let mut g = GuidePress::default();
        assert_eq!(g.release(Instant::now()), None);
    }

    #[test]
    fn guide_repeated_press_keeps_first_timestamp() {
        let t0 = Instant::now();
        let mut g = GuidePress::default();
        g.press(t0);
        g.press(t0 + ms(400));
        assert_eq!(g.tick(t0 + ms(500)), Some(Action::HomeLong));
    }
}

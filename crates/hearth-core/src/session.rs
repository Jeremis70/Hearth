//! The current game session and its playtime. Hearth runs one game at a time.

use std::time::{Duration, Instant};

use crate::model::{Game, GameId};
use crate::runtime::{Session, SessionId};
use crate::systems::SuspendPolicy;

/// Playtime of one session: wall time minus time spent frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayClock {
    started: Instant,
    frozen_since: Option<Instant>,
    frozen: Duration,
}

impl PlayClock {
    /// Starts counting at `now`.
    pub fn start(now: Instant) -> Self {
        Self {
            started: now,
            frozen_since: None,
            frozen: Duration::ZERO,
        }
    }

    /// Stops counting (game frozen). Repeated calls are ignored.
    pub fn freeze(&mut self, now: Instant) {
        self.frozen_since.get_or_insert(now);
    }

    /// Counts again.
    pub fn thaw(&mut self, now: Instant) {
        if let Some(since) = self.frozen_since.take() {
            self.frozen += now.saturating_duration_since(since);
        }
    }

    /// Time played so far.
    pub fn played(&self, now: Instant) -> Duration {
        let frozen_now = self
            .frozen_since
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        now.saturating_duration_since(self.started)
            .saturating_sub(self.frozen + frozen_now)
    }
}

/// Where the running game stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Started; its window isn't on screen yet.
    Starting,
    /// On screen and running.
    Playing,
    /// Hearth (Home or overlay) is in front.
    InBackground,
}

/// The one running game.
#[derive(Debug, Clone)]
pub struct ActiveSession {
    /// The game.
    pub game: Game,
    /// Runtime handle.
    pub session: Session,
    /// gamescope app id given to its windows.
    pub app_id: u32,
    /// Whether frozen with SIGSTOP right now.
    pub frozen: bool,
    /// What happens while Hearth is in front.
    pub suspend: SuspendPolicy,
    /// State.
    pub state: SessionState,
    clock: PlayClock,
}

impl ActiveSession {
    /// A session that just started.
    pub fn new(
        game: Game,
        session: Session,
        app_id: u32,
        suspend: SuspendPolicy,
        now: Instant,
    ) -> Self {
        Self {
            game,
            session,
            app_id,
            frozen: false,
            suspend,
            state: SessionState::Starting,
            clock: PlayClock::start(now),
        }
    }

    /// Records that the game was frozen at `now`.
    pub fn mark_frozen(&mut self, now: Instant) {
        self.frozen = true;
        self.clock.freeze(now);
    }

    /// Records that the game was resumed at `now`.
    pub fn mark_thawed(&mut self, now: Instant) {
        self.frozen = false;
        self.clock.thaw(now);
    }

    /// Time played so far.
    pub fn played(&self, now: Instant) -> Duration {
        self.clock.played(now)
    }

    /// Whether this is the session `id` of game `game`.
    pub fn is(&self, id: SessionId, game: &GameId) -> bool {
        self.session.id() == id && &self.game.id == game
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_time_is_not_played() {
        let t0 = Instant::now();
        let s = |n| t0 + Duration::from_secs(n);
        let mut clock = PlayClock::start(t0);
        assert_eq!(clock.played(s(10)), Duration::from_secs(10));
        clock.freeze(s(10));
        clock.freeze(s(15)); // ignored
        assert_eq!(clock.played(s(40)), Duration::from_secs(10));
        clock.thaw(s(40));
        assert_eq!(clock.played(s(50)), Duration::from_secs(20));
        clock.thaw(s(60)); // not frozen: no effect
        assert_eq!(clock.played(s(60)), Duration::from_secs(30));
    }
}

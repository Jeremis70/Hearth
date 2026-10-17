//! Window focus and overlays under gamescope, behind [`Compositor`].
//!
//! Gamescope is driven through X11 properties on its Xwayland server (`DISPLAY`). Gamescope must
//! run in Steam mode (`-e`): without it, focus control and the overlay plane are ignored.
//! Verified with gamescope 3.16.30 (see `docs/spikes/gamescope.md`):
//! - `STEAM_GAME` (CARDINAL) on a window gives it an app id. Hearth's main window uses
//!   [`SHELL_APP_ID`]; each game session gets its own id, set on the windows whose
//!   `_NET_WM_PID` is in the game's process group. Gamescope ignores a `SteamGameId` env var.
//! - `GAMESCOPECTRL_BASELAYER_APPID` (CARDINAL list) on the root window chooses what is shown,
//!   first id on top: `[game, 769]` shows the game, `[769]` shows Hearth.
//! - `STEAM_OVERLAY = 1` on a window draws it on the overlay plane above the focused app; `0`
//!   hides it. `GAMESCOPE_EXTERNAL_OVERLAY` is kept at 0: it adds a second overlay layer, and
//!   with both set gamescope drew the window twice. The window's own content is also
//!   transparent while hidden, in case gamescope keeps compositing it.
//! - `GAMESCOPE_FOCUSED_APP` on the root is gamescope's feedback (Steam mode only).
//!
//! Outside gamescope, [`NoCompositor`] makes every call a no-op.

use std::collections::BTreeSet;

use x11rb::connection::Connection;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::process::game_pids;

/// A gamescope app id (`STEAM_GAME` value).
pub type AppId = u32;

/// App id of Hearth's main window (the value gamescope-session-plus uses for a custom client).
pub const SHELL_APP_ID: AppId = 769;

/// Errors talking to the compositor.
#[derive(Debug, thiserror::Error)]
pub enum CompositorError {
    /// Could not connect to the X server.
    #[error("cannot connect to X11: {0}")]
    Connect(#[from] ConnectError),
    /// The connection failed.
    #[error("X11 connection error: {0}")]
    Connection(#[from] ConnectionError),
    /// A request failed.
    #[error("X11 request failed: {0}")]
    Reply(#[from] ReplyError),
}

/// Controls what the compositor shows. Ids are X11 window ids.
pub trait Compositor: Send + Sync {
    /// Whether a real compositor is being driven (false for [`NoCompositor`]).
    fn is_active(&self) -> bool;

    /// Whether focus switching and the overlay plane work (gamescope in Steam mode, `-e`).
    /// When false, games keep the screen once they open a window.
    fn controls_focus(&self) -> bool;

    /// Gives Hearth's main window [`SHELL_APP_ID`]. Without a known window id, windows owned
    /// by `pid` are used.
    fn set_shell_window(&self, window: Option<u32>, pid: u32) -> Result<(), CompositorError>;

    /// Tags every window whose `_NET_WM_PID` is a process of the game (its group `pgid` and
    /// all descendants) with `app`, mapped or not. Returns how many of them are mapped (0 while
    /// the game shows nothing yet). Unmapped windows don't count: toolkits create hidden ones
    /// first (Dolphin's Qt main window in batch mode), and focusing those shows nothing.
    fn assign_app(&self, pgid: u32, app: AppId) -> Result<usize, CompositorError>;

    /// Shows `apps`, first on top.
    fn focus(&self, apps: &[AppId]) -> Result<(), CompositorError>;

    /// Shows or hides `window` on the overlay plane, above whatever app is focused.
    fn set_overlay(&self, window: u32, visible: bool) -> Result<(), CompositorError>;

    /// The app gamescope reports as focused.
    fn focused_app(&self) -> Result<Option<AppId>, CompositorError>;
}

/// Used outside gamescope: nothing to control.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoCompositor;

impl Compositor for NoCompositor {
    fn is_active(&self) -> bool {
        false
    }
    fn controls_focus(&self) -> bool {
        false
    }
    fn set_shell_window(&self, _: Option<u32>, _: u32) -> Result<(), CompositorError> {
        Ok(())
    }
    fn assign_app(&self, _: u32, _: AppId) -> Result<usize, CompositorError> {
        Ok(0)
    }
    fn focus(&self, _: &[AppId]) -> Result<(), CompositorError> {
        Ok(())
    }
    fn set_overlay(&self, _: u32, _: bool) -> Result<(), CompositorError> {
        Ok(())
    }
    fn focused_app(&self) -> Result<Option<AppId>, CompositorError> {
        Ok(None)
    }
}

/// Returns a [`Gamescope`] compositor when running inside gamescope, else [`NoCompositor`].
///
/// Requires both `GAMESCOPE_WAYLAND_DISPLAY` (gamescope exports it to its children) and
/// gamescope's properties on `DISPLAY`'s root window. Root properties alone aren't enough:
/// a desktop's Xwayland can carry stale `GAMESCOPE_*` properties written by other clients
/// (seen on KDE after Steam ran).
pub fn detect() -> Box<dyn Compositor> {
    if std::env::var_os("DISPLAY").is_none()
        || std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_none()
    {
        return Box::new(NoCompositor);
    }
    match Gamescope::connect() {
        Ok(Some(gamescope)) => {
            if gamescope.steam_mode {
                tracing::info!("running inside gamescope");
            } else {
                tracing::warn!(
                    "gamescope is not in Steam mode (-e): focus switching and the overlay \
                     won't work; Guide long press will close games instead"
                );
            }
            Box::new(gamescope)
        }
        Ok(None) => Box::new(NoCompositor),
        Err(err) => {
            tracing::debug!("not using gamescope: {err}");
            Box::new(NoCompositor)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Atoms {
    steam_game: Atom,
    baselayer: Atom,
    external_overlay: Atom,
    steam_overlay: Atom,
    focused_app: Atom,
    net_wm_pid: Atom,
}

/// Gamescope driven over its Xwayland connection.
#[derive(Debug)]
pub struct Gamescope {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    steam_mode: bool,
}

impl Gamescope {
    /// Connects to `DISPLAY`. Returns `None` if the root window lacks the properties gamescope
    /// sets on its own Xwayland.
    pub fn connect() -> Result<Option<Self>, CompositorError> {
        let (conn, screen) = RustConnection::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let gamescope = Self {
            atoms: Atoms {
                steam_game: intern(&conn, "STEAM_GAME")?,
                baselayer: intern(&conn, "GAMESCOPECTRL_BASELAYER_APPID")?,
                external_overlay: intern(&conn, "GAMESCOPE_EXTERNAL_OVERLAY")?,
                steam_overlay: intern(&conn, "STEAM_OVERLAY")?,
                focused_app: intern(&conn, "GAMESCOPE_FOCUSED_APP")?,
                net_wm_pid: intern(&conn, "_NET_WM_PID")?,
            },
            conn,
            root,
            steam_mode: false,
        };
        // Set by gamescope itself (clients only ever write GAMESCOPECTRL_* and per-window
        // properties).
        let is_gamescope = gamescope
            .root_property_names()?
            .iter()
            .any(|name| name == "GAMESCOPE_FOCUSABLE_WINDOWS" || name == "GAMESCOPE_FOCUS_DISPLAY");
        let mut gamescope = gamescope;
        // GAMESCOPE_FOCUSED_APP only exists in Steam mode.
        gamescope.steam_mode = gamescope
            .root_property_names()?
            .iter()
            .any(|n| n == "GAMESCOPE_FOCUSED_APP");
        Ok(is_gamescope.then_some(gamescope))
    }

    /// Names of all properties on the root window.
    pub fn root_property_names(&self) -> Result<Vec<String>, CompositorError> {
        let atoms = self.conn.list_properties(self.root)?.reply()?.atoms;
        atoms.into_iter().map(|atom| self.atom_name(atom)).collect()
    }

    /// A CARDINAL property of the root window, by name.
    pub fn root_cardinals(&self, name: &str) -> Result<Vec<u32>, CompositorError> {
        let atom = intern(&self.conn, name)?;
        self.cardinals(self.root, atom)
    }

    /// Windows to inspect: root children and their children (gamescope has no reparenting
    /// window manager, but some toolkits nest a client window).
    pub fn windows(&self) -> Result<Vec<WindowInfo>, CompositorError> {
        let mut out = Vec::new();
        for top in self.conn.query_tree(self.root)?.reply()?.children {
            out.push(self.window_info(top)?);
            for child in self.conn.query_tree(top)?.reply()?.children {
                out.push(self.window_info(child)?);
            }
        }
        Ok(out)
    }

    fn window_info(&self, window: Window) -> Result<WindowInfo, CompositorError> {
        let geometry = self.conn.get_geometry(window)?.reply()?;
        let attributes = self.conn.get_window_attributes(window)?.reply()?;
        Ok(WindowInfo {
            window,
            size: (geometry.width, geometry.height),
            mapped: attributes.map_state == x11rb::protocol::xproto::MapState::VIEWABLE,
            pid: self
                .cardinals(window, self.atoms.net_wm_pid)?
                .first()
                .copied(),
            app: self
                .cardinals(window, self.atoms.steam_game)?
                .first()
                .copied(),
            overlay: self
                .cardinals(window, self.atoms.steam_overlay)?
                .first()
                .is_some_and(|&v| v != 0),
        })
    }

    fn atom_name(&self, atom: Atom) -> Result<String, CompositorError> {
        let reply = self.conn.get_atom_name(atom)?.reply()?;
        Ok(String::from_utf8_lossy(&reply.name).into_owned())
    }

    fn cardinals(&self, window: Window, atom: Atom) -> Result<Vec<u32>, CompositorError> {
        let reply = self
            .conn
            .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 256)?
            .reply()?;
        Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
    }

    fn set_cardinals(
        &self,
        window: Window,
        atom: Atom,
        values: &[u32],
    ) -> Result<(), CompositorError> {
        self.conn
            .change_property32(PropMode::REPLACE, window, atom, AtomEnum::CARDINAL, values)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Tags the windows of `pids`; returns how many of them are mapped.
    fn tag_windows_of(&self, pids: &BTreeSet<u32>, app: AppId) -> Result<usize, CompositorError> {
        let mut mapped = 0;
        for info in self.windows()? {
            if info.pid.is_some_and(|pid| pids.contains(&pid)) {
                if info.app != Some(app) {
                    self.set_cardinals(info.window, self.atoms.steam_game, &[app])?;
                    tracing::debug!(
                        window = info.window,
                        app,
                        mapped = info.mapped,
                        "tagged window"
                    );
                }
                mapped += usize::from(info.mapped);
            }
        }
        Ok(mapped)
    }
}

impl Compositor for Gamescope {
    fn is_active(&self) -> bool {
        true
    }

    fn controls_focus(&self) -> bool {
        self.steam_mode
    }

    fn set_shell_window(&self, window: Option<u32>, pid: u32) -> Result<(), CompositorError> {
        match window {
            Some(window) => {
                self.set_cardinals(window, self.atoms.steam_game, &[SHELL_APP_ID])?;
            }
            None => {
                let mapped = self.tag_windows_of(&BTreeSet::from([pid]), SHELL_APP_ID)?;
                tracing::debug!(mapped, "tagged shell windows by _NET_WM_PID");
            }
        }
        Ok(())
    }

    fn assign_app(&self, pgid: u32, app: AppId) -> Result<usize, CompositorError> {
        self.tag_windows_of(&game_pids(pgid), app)
    }

    fn focus(&self, apps: &[AppId]) -> Result<(), CompositorError> {
        self.set_cardinals(self.root, self.atoms.baselayer, apps)
    }

    fn set_overlay(&self, window: u32, visible: bool) -> Result<(), CompositorError> {
        // Only STEAM_OVERLAY. GAMESCOPE_EXTERNAL_OVERLAY puts the window on a second overlay
        // layer: with both set, gamescope drew it twice (visible when the output is resized),
        // and leaving it at 1 kept the window drawn while hidden.
        self.set_cardinals(window, self.atoms.external_overlay, &[0])?;
        self.set_cardinals(window, self.atoms.steam_overlay, &[u32::from(visible)])
    }

    fn focused_app(&self) -> Result<Option<AppId>, CompositorError> {
        Ok(self
            .cardinals(self.root, self.atoms.focused_app)?
            .first()
            .copied())
    }
}

/// What [`Gamescope::windows`] reports per window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowInfo {
    /// X11 window id.
    pub window: u32,
    /// Width and height in pixels.
    pub size: (u16, u16),
    /// Mapped and viewable.
    pub mapped: bool,
    /// `_NET_WM_PID`, if set.
    pub pid: Option<u32>,
    /// `STEAM_GAME`, if set.
    pub app: Option<AppId>,
    /// `STEAM_OVERLAY` is non-zero.
    pub overlay: bool,
}

fn intern(conn: &RustConnection, name: &str) -> Result<Atom, CompositorError> {
    Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_compositor_is_inert() {
        let c = NoCompositor;
        assert!(!c.is_active());
        assert!(!c.controls_focus());
        assert_eq!(c.assign_app(1234, 10_000).unwrap(), 0);
        assert_eq!(c.focused_app().unwrap(), None);
    }
}

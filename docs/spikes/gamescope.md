# Spike: gamescope focus switching and overlay (M2a phase A)

Date: 2026-10-17. gamescope 3.16.30 nested on KDE Plasma (Wayland), Intel Iris Xe,
RetroArch 1.22.2 + mupen64plus_next running Ocarina of Time. Hearth launched with
`scripts/run-gamescope.sh`; game and overlay clients started on gamescope's Xwayland (`:1`);
properties inspected with `gamescope_probe`, `xprop` and `gamescopectl focus_info`.

## Results

| Mechanism | Without `-e` | With `-e` (Steam mode) |
|---|---|---|
| Detect gamescope by `GAMESCOPE_*` root properties | works | works |
| Hearth window id from Slint raw window handle (X11) | works | works |
| `STEAM_GAME = 769` on Hearth's window | accepted, listed in `GAMESCOPE_FOCUSABLE_APPS` | same |
| `SteamGameId=<id>` env on the game process | ignored (gamescope uses the window id as app id) | ignored (window not focusable) |
| `STEAM_GAME` on windows found via `_NET_WM_PID` in the game's process group | works | works |
| `GAMESCOPECTRL_BASELAYER_APPID` focus switching | **ignored** (newest window keeps focus) | **works** both ways; `GAMESCOPE_FOCUSED_APP` confirms |
| `GAMESCOPECTRL_BASELAYER_WINDOW` | ignored | not needed |
| `GAMESCOPE_EXTERNAL_OVERLAY = 1` | drew once over the running game | **not drawn** (fresh or remapped window, running or frozen game) |
| `STEAM_OVERLAY = 1` | not tested | **works**, also over a SIGSTOPped game (see hiding below) |
| `GAMESCOPE_FOCUSED_APP` feedback | absent | present |
| SIGSTOP / SIGCONT on the game's process group | frozen frame stays on screen, resumes | same |

## Decisions

- Run gamescope with `-e`. It only changes how that gamescope instance behaves (it expects
  Steam's window conventions); nothing system-wide.
- Tag game windows ourselves via `_NET_WM_PID` + process group; don't rely on `SteamGameId`.
- The overlay window uses `STEAM_OVERLAY` plus `GAMESCOPE_EXTERNAL_OVERLAY`, both 1 to show and
  both 0 to hide, and its content is transparent while hidden (see follow-ups).
- `gamescopectl screenshot` captures the base layer only (no overlay plane), so overlay checks
  were done by eye.

## Notes

- RetroArch must be forced to X11 inside gamescope (`XDG_SESSION_TYPE=x11`,
  `video_context_driver = "x"`, `WAYLAND_DISPLAY` unset); otherwise libwayland defaults to the
  host's `wayland-0` and RetroArch opens on the desktop instead.
- Not tested: keyboard focus inside the overlay window (controller actions are injected, so
  this only matters for keyboard use under gamescope).

## Follow-ups found while building M2a

- Detection: a desktop's Xwayland can carry stale `GAMESCOPE_*` root properties (seen on KDE's
  `:0`, probably from Steam). Hearth now requires `GAMESCOPE_WAYLAND_DISPLAY` in its
  environment (gamescope exports it to children) plus `GAMESCOPE_FOCUSABLE_WINDOWS` /
  `GAMESCOPE_FOCUS_DISPLAY` on the root.
- Without `-e`, gamescope keeps the newest window in front regardless of the baselayer, so a
  frozen game would leave a black screen. Hearth treats that case like running outside
  gamescope: no freezing, no overlay, Guide long press closes the game.
- Hiding the overlay: setting only `STEAM_OVERLAY = 0` (leaving `GAMESCOPE_EXTERNAL_OVERLAY = 1`)
  kept the window drawn over the game and Home, passing input through. The spike had only
  checked `focus_info`, not the screen. Hearth now clears both properties and renders the
  overlay fully transparent unless it is open; verified by eye in Steam mode.

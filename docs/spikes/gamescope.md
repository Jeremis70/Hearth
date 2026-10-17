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
| `STEAM_OVERLAY = 1` | not tested | **works**, also over a SIGSTOPped game; `0` hides it again |
| `GAMESCOPE_FOCUSED_APP` feedback | absent | present |
| SIGSTOP / SIGCONT on the game's process group | frozen frame stays on screen, resumes | same |

## Decisions

- Run gamescope with `-e`. It only changes how that gamescope instance behaves (it expects
  Steam's window conventions); nothing system-wide.
- Tag game windows ourselves via `_NET_WM_PID` + process group; don't rely on `SteamGameId`.
- The overlay window uses `STEAM_OVERLAY` (plus `GAMESCOPE_EXTERNAL_OVERLAY`, which was set
  when it was verified) and is shown/hidden by toggling `STEAM_OVERLAY`.
- `gamescopectl screenshot` captures the base layer only (no overlay plane), so overlay checks
  were done by eye.

## Notes

- RetroArch must be forced to X11 inside gamescope (`XDG_SESSION_TYPE=x11`,
  `video_context_driver = "x"`, `WAYLAND_DISPLAY` unset); otherwise libwayland defaults to the
  host's `wayland-0` and RetroArch opens on the desktop instead.
- Not tested: keyboard focus inside the overlay window (controller actions are injected, so
  this only matters for keyboard use under gamescope).

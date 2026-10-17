#!/usr/bin/env bash
# Runs Hearth inside a nested gamescope (1920×1080), the way the appliance session will.
# WAYLAND_DISPLAY is unset so winit uses gamescope's Xwayland, where Hearth sets the X11
# properties gamescope reads (STEAM_GAME, GAMESCOPECTRL_BASELAYER_APPID, ...).
# -e (Steam mode) is required: without it gamescope ignores GAMESCOPECTRL_BASELAYER_APPID and
# doesn't draw the overlay plane. It only changes how this nested gamescope behaves.
# Extra arguments go to hearth-shell. Nothing outside the repo is changed.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p hearth-shell
exec gamescope -e -W 1920 -H 1080 -- \
    env -u WAYLAND_DISPLAY HEARTH_HOME=./.dev-data \
    target/debug/hearth-shell --fullscreen "$@"

#!/usr/bin/env bash
# Runs Hearth inside a nested gamescope, the way the appliance session will.
# WAYLAND_DISPLAY is unset so winit uses gamescope's Xwayland, where Hearth sets the X11
# properties gamescope reads (STEAM_GAME, GAMESCOPECTRL_BASELAYER_APPID, ...).
# -e (Steam mode) is required: without it gamescope ignores GAMESCOPECTRL_BASELAYER_APPID and
# doesn't draw the overlay plane. It only changes how this nested gamescope behaves.
# Extra arguments go to hearth-shell. Nothing outside the repo is changed.
#
# Environment:
#   HEARTH_GS_SIZE=WxH      gamescope output size (default: the display's current mode, so a
#                           16:10 laptop panel gets no black bars)
#   HEARTH_GS_FULLSCREEN=1  start the gamescope window fullscreen (instead of Alt+F3 later)
#   HEARTH_GS_ARGS="..."    extra gamescope flags, e.g. "--backend sdl"
#   HEARTH_GS_RELEASE=1     run the optimised build (target/release); day to day:
#                           HEARTH_GS_RELEASE=1 HEARTH_GS_FULLSCREEN=1 scripts/run-gamescope.sh
set -euo pipefail
cd "$(dirname "$0")/.."

size="${HEARTH_GS_SIZE:-}"
if [[ -z "$size" ]]; then
    size="$(xrandr --current 2>/dev/null | awk '/\*/ { print $1; exit }' || true)"
fi
if [[ ! "$size" =~ ^[0-9]+x[0-9]+$ ]]; then
    size="1920x1080"
fi
width="${size%x*}"
height="${size#*x}"

gamescope_args=(-e -W "$width" -H "$height")
if [[ "${HEARTH_GS_FULLSCREEN:-0}" == "1" ]]; then
    gamescope_args+=(-f)
fi
if [[ -n "${HEARTH_GS_ARGS:-}" ]]; then
    read -r -a extra <<< "$HEARTH_GS_ARGS"
    gamescope_args+=("${extra[@]}")
fi

profile=debug
build_args=(-p hearth-shell)
if [[ "${HEARTH_GS_RELEASE:-0}" == "1" ]]; then
    profile=release
    build_args+=(--release)
fi

cargo build "${build_args[@]}"
echo "gamescope ${gamescope_args[*]} ($profile build)" >&2
exec gamescope "${gamescope_args[@]}" -- \
    env -u WAYLAND_DISPLAY HEARTH_HOME=./.dev-data \
    "target/$profile/hearth-shell" --fullscreen "$@"

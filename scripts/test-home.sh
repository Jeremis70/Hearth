#!/usr/bin/env bash
# Runs Hearth on a throwaway copy of the dev data, for tests with fake data: fixture
# SteamGridDB art (--mock-sgdb=fixtures) and fixture downloads never touch ./.dev-data's
# cache, picks or library.
#
#   scripts/test-home.sh [--reset] [gamescope|windowed] [hearth-shell args...]
#
# The home is ./.dev-data/test-home (HEARTH_TEST_HOME overrides). It gets a copy of the
# dev config and library (made once; --reset starts over), the dev packs (linked, read
# only), an empty cache and a `.throwaway` marker, which --mock-sgdb requires.
# HEARTH_DOWNLOADS is ./.dev-data/downloads (fill it with
# `cargo run -p hearth-core --example make_fixtures`), so the real ~/Downloads is never read.
# Default mode: gamescope (scripts/run-gamescope.sh), with --mock-sgdb=fixtures and
# --games-dir ./.dev-data/test-games if it exists (HEARTH_TEST_GAMES overrides), else
# ./.dev-data/games. --reset keeps the games folder (it lives outside the home).
#
# Physical controllers are ignored (--no-pad-input): only the keyboard and the dev socket
# drive it, so a pad in use elsewhere never presses anything here.
#
# The user profile is HEARTH_TEST_PROFILE (default: default), passed as --profile.
#
# HEARTH_TEST_SGDB=real: the real SteamGridDB with the dev config's key instead of the
# fixtures, in its own home (./.dev-data/test-home-real by default), so fixture art and real
# art never share a cache.
set -euo pipefail
cd "$(dirname "$0")/.."

sgdb="${HEARTH_TEST_SGDB:-fixtures}"
case "$sgdb" in
    fixtures) default_home=./.dev-data/test-home ;;
    real) default_home=./.dev-data/test-home-real ;;
    *) echo "HEARTH_TEST_SGDB must be fixtures or real" >&2; exit 1 ;;
esac
home="${HEARTH_TEST_HOME:-$default_home}"
case "$home" in
    ./.dev-data/*) ;;
    *) echo "HEARTH_TEST_HOME must be under ./.dev-data" >&2; exit 1 ;;
esac
if [[ "${1:-}" == "--reset" ]]; then
    shift
    rm -rf -- "$home"
fi
mode=gamescope
if [[ "${1:-}" == "gamescope" || "${1:-}" == "windowed" ]]; then
    mode="$1"
    shift
fi

if [[ ! -e "$home/.throwaway" ]]; then
    mkdir -p "$home/config" "$home/data" "$home/cache"
    if [[ -f ./.dev-data/config/hearth.toml ]]; then
        cp ./.dev-data/config/hearth.toml "$home/config/"
    fi
    if [[ -f ./.dev-data/data/library.sqlite3 ]]; then
        cp ./.dev-data/data/library.sqlite3 "$home/data/"
    fi
    if [[ -d ./.dev-data/packs ]]; then
        ln -sfn "$(pwd)/.dev-data/packs" "$home/packs"
    fi
    touch "$home/.throwaway"
fi
mkdir -p ./.dev-data/downloads

export HEARTH_HOME="$home"
export HEARTH_DOWNLOADS="${HEARTH_DOWNLOADS:-./.dev-data/downloads}"
games="${HEARTH_TEST_GAMES:-./.dev-data/test-games}"
if [[ ! -d "$games" ]]; then
    games=./.dev-data/games
fi
args=(--no-pad-input --profile "${HEARTH_TEST_PROFILE:-default}" --games-dir "$games" "$@")
if [[ "$sgdb" == "fixtures" ]]; then
    args=(--mock-sgdb=fixtures "${args[@]}")
fi
if [[ "$mode" == "windowed" ]]; then
    exec cargo run -p hearth-shell -- --windowed "${args[@]}"
fi
exec scripts/run-gamescope.sh "${args[@]}"

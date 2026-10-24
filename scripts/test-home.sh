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
# --games-dir ./.dev-data/games.
set -euo pipefail
cd "$(dirname "$0")/.."

home="${HEARTH_TEST_HOME:-./.dev-data/test-home}"
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
args=(--mock-sgdb=fixtures --games-dir ./.dev-data/games "$@")
if [[ "$mode" == "windowed" ]]; then
    exec cargo run -p hearth-shell -- --windowed "${args[@]}"
fi
exec scripts/run-gamescope.sh "${args[@]}"

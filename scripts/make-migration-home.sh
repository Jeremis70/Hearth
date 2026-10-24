#!/usr/bin/env bash
# Builds a throwaway home shaped like ./.dev-data before profiles, to rehearse the move into
# profiles/default (round D) on a copy:
#
#   scripts/make-migration-home.sh [--real-saves] [home]
#
# The home (default ./.dev-data/test-migrate; it must be under ./.dev-data) is replaced. It
# gets copies of ./.dev-data's retroarch/, dolphin/, config/controls/, config/hearth.toml and
# data/library.sqlite3, the dev packs (linked) and a `.throwaway` marker. By default every save
# and state is replaced by a fake file of the same name ("fake save: <name>"); --real-saves
# keeps the real ones, to check that the emulators find them after the move.
#
# Then, for example:
#   HEARTH_TEST_HOME=./.dev-data/test-migrate scripts/test-home.sh windowed
#   HEARTH_TEST_MIGRATION_ABORT_AFTER=3 HEARTH_TEST_HOME=./.dev-data/test-migrate \
#       scripts/test-home.sh windowed     # killed after move 3; start again to resume
#   HEARTH_HOME=./.dev-data/test-migrate cargo run -p hearth-core --example migration_plan
set -euo pipefail
cd "$(dirname "$0")/.."

real=0
if [[ "${1:-}" == "--real-saves" ]]; then
    real=1
    shift
fi
home="${1:-./.dev-data/test-migrate}"
case "$home" in
    ./.dev-data/?*) ;;
    *) echo "the home must be under ./.dev-data" >&2; exit 1 ;;
esac
src=./.dev-data
for dir in retroarch dolphin; do
    if [[ ! -d "$src/$dir" ]]; then
        echo "$src/$dir is missing: ./.dev-data is migrated already?" >&2
        exit 1
    fi
done

rm -rf -- "$home"
mkdir -p "$home/config" "$home/data" "$home/cache"
cp -a "$src/retroarch" "$src/dolphin" "$home/"
if [[ -d "$src/config/controls" ]]; then
    cp -a "$src/config/controls" "$home/config/"
fi
if [[ -f "$src/config/hearth.toml" ]]; then
    cp -a "$src/config/hearth.toml" "$home/config/"
fi
cp -a "$src/data/library.sqlite3" "$home/data/"
if [[ -d "$src/packs" ]]; then
    ln -sfn "$(pwd)/.dev-data/packs" "$home/packs"
fi
touch "$home/.throwaway"

if [[ $real == 0 ]]; then
    # Saves and states: same names, fake contents.
    find "$home/retroarch/saves" "$home/retroarch/states" "$home/dolphin/StateSaves" \
        "$home/dolphin/GC" "$home/dolphin/Wii/title" \
        -type f \( -path '*/GC/*.gci' -o -path '*/GC/*.raw' -o -path '*/saves/*' \
        -o -path '*/states/*' -o -path '*/StateSaves/*' -o -path '*/Wii/title/*' \) \
        ! -name SRAM.raw 2>/dev/null |
        while IFS= read -r file; do
            printf 'fake save: %s\n' "$(basename "$file")" > "$file"
        done
fi
echo "made $home ($([[ $real == 1 ]] && echo real || echo fake) saves)"

# Hearth

An open, controller-first living-room console for Linux: one library for Steam games, emulated classics and native ports, plus the user's own media. Rust + Slint. Full architecture and milestones: `docs/PLAN.md` (read it before starting any milestone).
Read .github/instructions/rust.instructions.md and docs/STATUS.md at the start of every session.

## This is the developer's daily-driver laptop: safety rules (non-negotiable)

- Never run `sudo`, `pkexec`, `su`, `pkill`, `killall`, or any package manager (`pacman`, `paru`, `yay`, `flatpak install`). If something needs installing, stop and tell me the exact command; I'll run it.
- Never run `systemctl`, `nmcli`, `rfkill`, `bluetoothctl`, or anything that changes system, network, Bluetooth or session state.
- Never write outside this repository. App data in development goes under `./.dev-data/` (gitignored), selected via the `HEARTH_HOME` env var; production paths (XDG dirs via `directories`) are only used when `HEARTH_HOME` is unset.
- Real system integrations are opt-in and default to mocks:
  - Network: `MockNetworkBackend` unless `--real-network` is passed.
  - Power: `MockPower` unless `--real-power` is passed. Never call suspend/reboot/poweroff in tests.
  - Input: gilrs (read-only, non-exclusive) in development. Never grab devices exclusively or create uinput devices unless a milestone explicitly says so.
- Before starting Hearth (cargo run, `scripts/run-gamescope.sh`), check whether one is already running (`pgrep -x -a hearth-shell`; its input guard is a separate `hearth-guard` process and doesn't count). If one is, don't start another: ask me. Hearth also refuses to start twice (lock in `<HEARTH_HOME>/data/hearth.lock`); `--print-controls` is read-only and may run next to it.
- Only kill processes the app itself spawned (tracked by process group). Never `pkill`/`killall`.
- Don't use `rm -rf` on anything but `target/` or `./.dev-data/`.
- Personal Claude Code hooks (`.claude/settings.local.json`, gitignored) run `.claude/inhibit.sh`: each prompt starts `kde-inhibit --power --screenSaver sleep 7200` (screen stays awake, 2 h max, PID in `.claude/inhibit.pid`); Stop and SessionEnd end it. Leave it alone; it is neither Hearth nor a game.

## Workspace

- `crates/hearth-audio`: the audio engine (cpal output, symphonia decoding, mixer, music rules). No Slint.
- `crates/hearth-core`: platform-neutral model, systems, library, runtimes. No Slint, no D-Bus.
- `crates/hearth-platform`: Linux integrations behind traits (network, power, input, gamescope), each with a mock.
- `crates/hearth-shell`: Slint app (`ui/*.slint`, `src/`).

## Commands

- Build/check: `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`
- Test: `cargo test --workspace`
- Run (dev): `HEARTH_HOME=./.dev-data cargo run -p hearth-shell -- --windowed`
- Preview a .slint file live: `slint-viewer --auto-reload crates/hearth-shell/ui/app.slint`
- Run inside a nested gamescope (Steam mode): `scripts/run-gamescope.sh` (extra args go to hearth-shell; uses the display's native size, override with `HEARTH_GS_SIZE=1920x1080`; `HEARTH_GS_FULLSCREEN=1` starts it fullscreen). This is the way to test the overlay: plain `cargo run` has no gamescope, so games open their own window and Guide long press closes them.
- Drive a running Hearth from scripts: start it with `--dev-socket ./.dev-data/actions.sock`, then `scripts/send-action.sh down accept home_short`. Add `--no-pad-input` (scripts/test-home.sh does) so physical controllers never drive a test instance; `pad:connect:<dualsense|ds4|xbox|switch|generic>` / `pad:disconnect` fake them. Animation: `anim:on|off`, `reduce_motion:on|off`, `anim_state` (JSON). Real SteamGridDB in a throwaway home: `HEARTH_TEST_SGDB=real scripts/test-home.sh`. Audio: `scripts/send-action.sh sound:move audio_state` (plays a logical sound; prints the engine's state as JSON).
- Audio: `--no-audio` or `HEARTH_AUDIO=null` for no output (tests, headless). `HEARTH_AUDIO_STATS=1` logs input → first mixed sample and cpal's output delay per UI sound. Check the stream is real (read-only): `pactl list sink-inputs` (look for `alsa_playback.hearth-shell`). Never change the default sink; to measure a wired path, `PIPEWIRE_NODE=<sink name>` affects only the process it's set on. `cargo run --release -p hearth-audio --example audio_probe -- <file> 10 30 --sound <file>` plays through the real output and prints the state.
- Game running slowly? Show RetroArch's FPS counter: `HEARTH_RETROARCH_ARGS="--appendconfig=data/dev/retroarch-fps.cfg" scripts/run-gamescope.sh` (`HEARTH_RETROARCH_ARGS` adds any RetroArch arguments). `HEARTH_HOME=./.dev-data cargo run -p hearth-core --example launch_timing -- <core> <rom> 600` times frames outside the UI (the default profile's RetroArch; `HEARTH_PROFILE` picks another).
- Animated art: `cargo run --release -p hearth-core --example anim_probe -- <file>` prints what Hearth decodes per frame; `art_probe --candidates` lists animated candidates and sizes.
- Profiles: per-person data lives in `<HEARTH_HOME>/profiles/<id>/` (saves, controls, emulator folders; layout in `docs/design/README.md` §9). `--profile <id>` (default `default`; an unknown id is a fresh profile) works with `--print-controls` too; `HEARTH_TEST_PROFILE=test2 scripts/test-home.sh` passes it. `scripts/send-action.sh profile_state` prints the active profile and its folders (JSON). Before touching data from before profiles: `HEARTH_HOME=… cargo run -p hearth-core --example migration_plan` (read-only). Rehearse the move on a copy: `scripts/make-migration-home.sh [--real-saves]`; `HEARTH_TEST_MIGRATION_ABORT_AFTER=<step>` (throwaway homes only) kills Hearth after that step.
- Inspect gamescope properties: `DISPLAY=:1 cargo run -p hearth-platform --example gamescope_probe -- props|windows`

## Conventions

- Slint owns the main thread. Slow or async work runs on worker threads; send results back with `slint::Weak::upgrade_in_event_loop`. Only `Send` data crosses threads (e.g. `SharedPixelBuffer`, never `slint::Image`).
- Design for 1920×1080 with all sizes/colors in a `Theme` global; check 1280×720 too.
- Controller input is converted to an `Action` enum, then injected into Slint as key events, so the keyboard works identically during development.
- Errors: `thiserror` in libraries, `anyhow` in the binary. Logging: `tracing`.
- Keep milestones small: implement only the current milestone, finish with `cargo fmt`, clippy clean, tests passing, then commit and stop.
- Commit messages: imperative, one logical change per commit.

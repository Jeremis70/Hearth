# Hearth

An open, controller-first living-room console for Linux: one library for Steam games, emulated classics and native ports, plus the user's own media. Rust + Slint. Full architecture and milestones: `docs/PLAN.md` (read it before starting any milestone).

## This is the developer's daily-driver laptop: safety rules (non-negotiable)

- Never run `sudo`, `pkexec`, `su`, or any package manager (`pacman`, `paru`, `yay`, `flatpak install`). If something needs installing, stop and tell me the exact command; I'll run it.
- Never run `systemctl`, `nmcli`, `rfkill`, `bluetoothctl`, or anything that changes system, network, Bluetooth or session state.
- Never write outside this repository. App data in development goes under `./.dev-data/` (gitignored), selected via the `HEARTH_HOME` env var; production paths (XDG dirs via `directories`) are only used when `HEARTH_HOME` is unset.
- Real system integrations are opt-in and default to mocks:
  - Network: `MockNetworkBackend` unless `--real-network` is passed.
  - Power: `MockPower` unless `--real-power` is passed. Never call suspend/reboot/poweroff in tests.
  - Input: gilrs (read-only, non-exclusive) in development. Never grab devices exclusively or create uinput devices unless a milestone explicitly says so.
- Only kill processes the app itself spawned (tracked by process group). Never `pkill`/`killall`.
- Don't use `rm -rf` on anything but `target/` or `./.dev-data/`.

## Workspace

- `crates/hearth-core`: platform-neutral model, systems, library, runtimes. No Slint, no D-Bus.
- `crates/hearth-platform`: Linux integrations behind traits (network, power, input, gamescope), each with a mock.
- `crates/hearth-shell`: Slint app (`ui/*.slint`, `src/`).

## Commands

- Build/check: `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`
- Test: `cargo test --workspace`
- Run (dev): `HEARTH_HOME=./.dev-data cargo run -p hearth-shell -- --windowed`
- Preview a .slint file live: `slint-viewer --auto-reload crates/hearth-shell/ui/app.slint`
- Run inside a nested gamescope (Steam mode): `scripts/run-gamescope.sh` (extra args go to hearth-shell; uses the display's native size, override with `HEARTH_GS_SIZE=1920x1080`; `HEARTH_GS_FULLSCREEN=1` starts it fullscreen). This is the way to test the overlay: plain `cargo run` has no gamescope, so games open their own window and Guide long press closes them.
- Drive a running Hearth from scripts: start it with `--dev-socket ./.dev-data/actions.sock`, then `scripts/send-action.sh down accept home_short`
- Game running slowly? Show RetroArch's FPS counter: `HEARTH_RETROARCH_ARGS="--appendconfig=data/dev/retroarch-fps.cfg" scripts/run-gamescope.sh` (`HEARTH_RETROARCH_ARGS` adds any RetroArch arguments). `cargo run -p hearth-core --example launch_timing -- ./.dev-data/retroarch <core> <rom> 600` times frames outside the UI.
- Inspect gamescope properties: `DISPLAY=:1 cargo run -p hearth-platform --example gamescope_probe -- props|windows`

## Conventions

- Slint owns the main thread. Slow or async work runs on worker threads; send results back with `slint::Weak::upgrade_in_event_loop`. Only `Send` data crosses threads (e.g. `SharedPixelBuffer`, never `slint::Image`).
- Design for 1920×1080 with all sizes/colors in a `Theme` global; check 1280×720 too.
- Controller input is converted to an `Action` enum, then injected into Slint as key events, so the keyboard works identically during development.
- Errors: `thiserror` in libraries, `anyhow` in the binary. Logging: `tracing`.
- Keep milestones small: implement only the current milestone, finish with `cargo fmt`, clippy clean, tests passing, then commit and stop.
- Commit messages: imperative, one logical change per commit.

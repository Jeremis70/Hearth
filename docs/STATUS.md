# Hearth status

Last updated: 2026-10-19. Read this first, then `AGENTS.md` (rules, commands),
`.github/instructions/rust.instructions.md` and `docs/plan.md` (architecture).

## 0. Where we are, and how we work

**Done so far:**
- M2a (first game + overlay).
- Dolphin.
- The controls system: layered files and translators.
- The controls UI: a PS5-style controls page and the button test.
- The Capture button (a notice only for now).
- InputPlumber input (`docs/spikes/inputplumber.md`): menus hidden from games, Guide over D-Bus,
  live remapping through profiles, the virtual Elite pad for every emulator.
- Art and Home round (2026-10-19): per-game static art from SteamGridDB, the new Home row,
  the Options menu with Favorite and Properties, asset packs for console icons (sounds: names
  only).

Art and Home round, 2026-10-19, newest first:
- `89d85b9`: opaque Options menu.
- `6d7351a`: design docs: game artwork (§6), asset packs and logical names (§7), Home row.
- `505c38d`: `art_probe` example (what SteamGridDB would match, read-only).
- `55496c1`: Home redesign: art on tiles and background, sliding row, icon + name beside the
  focused tile, logo in the hero zone, Options menu, Properties, Display & sound pack picker,
  `--mock-network`.
- `85389e0`: an unreachable games root keeps the library (and art) as it was.
- `ebc8332`: library migration under the write lock (library and art threads open it at once).
- `ed7657f`: Hearth's own console glyphs (`assets/icons/consoles/`).
- `d8fcd4c`: asset packs and the per-file fallback resolver (`hearth-core::packs`).
- `c5559a2`, `6d1a1a0`: art worker, `art`/`art_queue` tables, favorites column.
- `0627c40`: identification, SteamGridDB provider (ureq), art cache, pre-blurred hero.
- `4eb3206`: ROM header reader shared by controls keys and art (`rom_header.rs`).
- `81750ce`: `<config>/hearth.toml` (SteamGridDB key, pack choices).
- `039ca31`: plan updated for zbus 6 / rust 1.89 and SteamGridDB over ureq.

Before that (things going away, 2026-10-18):
- `980df42`: controller notices (battery, connect), freeze when the game's controller goes,
  ghost devices stripped again on reconnect.
- `f4f5704`: emulators see only the virtual pad (SDL allowlist, RetroArch `sdl2` driver and
  Hearth's own joypad profiles).
- `7a75ad1`: InputPlumber DualSense config with the Elite pad (install in
  `/etc/inputplumber/devices.d/`, already installed on this laptop).
- `dfef039`: freeze when InputPlumber goes away, take over whenever it returns.
- `f734b38`: survive InputPlumber restarting.
- `3884145`: guard as its own `hearth-guard` binary.

InputPlumber round, 2026-10-18:
- `abcc994`: Mute and raw values in the button test; gilrs trigger axes, paddle order.
- `0bf5a95`: live apply through the game's profile, per-option "next launch" notes.
- `3b36323`: emulators on the virtual pad with a fixed layout.
- `822b70b`: profile translator, `VirtualLayout`, `same_as` on console controls.
- `170c1e1`: input guard, release on exit/panic/signals.
- `b3ffe8f`: InputPlumber backend, `--input`.

Before that:
- `b1afcb3`: stick options belong to the console stick, swap_sticks and the L1 fast-forward
  default are gone.
- `d5ed37c`: button test rebuilt on the controls page.
- `60e5359`: raw gilrs input, connection and battery.
- `c72e12c`: exact-shape highlights.

The working tree is clean.

**Working with the user (all confirmed in past sessions):**
- **Plan first:** for a `/plan` request, give a short plan (with a layout sketch for UI) and wait
  for OK. Direct instructions are done straight away.
- **Small commits:** imperative messages, one logical change each, **no Co-Authored-By trailer**,
  and every commit must build and pass tests. A file already staged (`git rm`) slips into the
  next commit, so check `git status` first.
- **Finish with a report:**
  - what was verified, with how (screenshots, logs, tests);
  - what couldn't be tested;
  - deviations from the request;
  - numbered manual checks with the controller.
- **The user edits files too** (for example `data/controls/gc.toml`). Never revert or commit
  their uncommitted edits without asking. To test without them:
  `git stash push <file>`, run the tests, then `git stash pop`.
- **Preferences:**
  - Kenney's real glyphs over text pills (pills only when no glyph exists).
  - Defaults don't bind emulator actions.
  - Assignable sticks (swap-on-conflict) instead of special options.

**Live testing safely** (this is the user's daily laptop, often with their own Hearth running):
1. `pgrep -x -a hearth-shell` (the `hearth-guard` next to it is Hearth's input guard). If anything runs, don't start another and don't send actions or take
   screenshots: `gamescopectl screenshot` would capture *their* session. Ask, or test offline.
2. Start a test instance in the background:
   `RUST_LOG=info HEARTH_GS_SIZE=1920x1080 setsid scripts/run-gamescope.sh --dev-socket ./.dev-data/actions.sock > .dev-data/test-run.log 2>&1 &`.
   Wait for the socket. Record the pid (`pgrep -x hearth-shell`) and stop only that pid with
   SIGTERM when done (its game goes with it).
3. Chain the follow-up commands with `&&` after the check, so nothing runs against someone
   else's instance.
4. Navigation:
   - Settings → Controllers: `right right right right accept`, then `down down down accept`.
   - In that pane: `right` enters it, then `down` ×0/1/2 for Test buttons / N64 / GameCube, and
     `accept`.
   - The Sticks & options tab is `next_tab`.
   - Launching TP: `accept` on Home, about 25 s to boot.
5. Screenshots: `GAMESCOPE_WAYLAND_DISPLAY=gamescope-0 gamescopectl screenshot $PWD/<file>.png`.
   They capture the base layer only (never the overlay window). Check overlay state with
   `DISPLAY=:1 cargo run -p hearth-platform --example gamescope_probe -- windows|props`.

**Next / open:**
- Art round, not verified yet (no SteamGridDB key in Claude's environment):
  - a real fetch: OoT and TP grid/hero/logo after a first run, then the same art offline from
    the cache. Check by hand with
    `HEARTH_HOME=./.dev-data cargo run -p hearth-core --example art_probe -- --rom n64 <rom>`;
  - Options menu and Favorite with the controller (done through the dev socket only).
- Art round follow-ups:
  - "Refresh artwork", "Change artwork", "Wrong game?" (shown disabled in Properties);
  - a NetworkManager connectivity signal instead of the 60 s queue check;
  - the seam between the sharp and the blurred hero is softened by a gradient, not blended;
  - UI sounds (playback) next round; the `ps5-sounds` pack has no `ambient_loop`.
- Not tested by the user yet (all done through the dev socket or unit tests). Ask how these went:
  - button test with real presses (held highlights, trigger bars, stick dots, PS short/long);
  - menu navigation with the stick and D-pad now that gilrs runs unfiltered;
  - Create = Capture in menus and in game;
  - C-stick invert following the stick when it is moved to the left stick in TP;
  - how the controls page reads from the couch.
- The Sticks & options tab after `b1afcb3` was not seen live (the user's Hearth was running).
- Open question: the user's "Create can't be assigned" was read as "defaults to Capture, never
  reaches a game, still rebindable". The user hasn't contradicted it; lock it like Guide only if
  asked.
- InputPlumber follow-ups:
  - rumble doesn't reach the DualSense over Bluetooth (not even InputPlumber's own `Rumble`);
    try USB;
  - stick options (inversion, deadzone, sensitivity), triggers and vibration still apply at the
    next launch (a profile can't express them);
  - user idea: the button test's trigger highlight could follow the pull (visual change);
  - user idea: a battery indicator when the pad is plugged in or unplugged ("charging · 45 %");
  - the reconnect overlay and the notice card are placeholders (user); Kenney's controller
    icons aren't in the repo, the card uses Lucide's gamepad.
- Later milestones (`docs/plan.md`):
  - Capture: panel and hold-to-screenshot, see `docs/design/overlay.md` §10.
  - ES-DE import, Steam, overlay dock actions, library grid, Uninstall.

## 1. What exists

**gamescope and sessions** (M2a)
- `hearth-platform/src/gamescope.rs` (`Compositor` trait, `NoCompositor` outside gamescope):
  - Hearth's window gets `STEAM_GAME=769`.
  - Each game gets an app id (10000+) on every window of its process group (`process.rs`,
    retried 15 s).
  - Focus goes through `GAMESCOPECTRL_BASELAYER_APPID`, checked through `GAMESCOPE_FOCUSED_APP`
    and retried for 3 s. If it fails, the game keeps running; it is never left frozen.
- Runtime (`runtime/process.rs`):
  - Each game gets its own process group; freezing is SIGSTOP/SIGCONT on that group.
  - Close sends SIGTERM, then SIGKILL after 3 s.
  - Games get pdeathsig and are launched from the UI thread.
  - Playtime excludes frozen time.
- Session (`hearth-shell/src/bridge/session.rs`):
  - Guide short: freeze the game and open the overlay. Guide long: Home.
  - Resume unfreezes at once.
  - Menu actions fire on button **release**; a press that began in a game is disarmed
    (`PadActivity::disarm`).
- **Single instance:** a `flock` on `<data>/hearth.lock`. `--print-controls` may run alongside.

**Overlay** (`ui/overlay/`)
- `window.slint` routes input to the active `OverlayPanel` (none, menu, controls, notice). The
  session sets it, together with `STEAM_OVERLAY`.
- `quick_menu.slint` holds the Now playing card, the dock and the close confirmation.
- The notice panel is a toast over a running game: no scrim, no freeze, and the game keeps
  focus.
- A new panel is a component with `handle(action) -> bool`, an enum value, and a branch in
  `handle` and in the drawing.

**Emulators**
- RetroArch (`profiles/retroarch.rs`, N64 via mupen64plus_next) uses an isolated config in
  `.dev-data/retroarch/`.
- Dolphin (`profiles/dolphin.rs`, gc and wii) is the native `dolphin-emu`, else the Flatpak,
  run with `-u <HEARTH_HOME>/dolphin` and `-C` overrides (reasons listed in the file).
- Dolphin needs `UseDiscordPresence=False`: otherwise it writes outside the repo.
- The window search counts mapped windows only.

**Controls files** (`hearth-core/src/controls/`)
- **Inputs:** Hearth pad inputs by position (`PadInput`, plus extras: touchpad, share,
  paddle1–4).
- **Targets:**
  - console controls, declared in `data/controls/<system>.toml` as `[[console]]` with name,
    kind, label and hint. Every stick and D-pad also offers "<name> up/down/left/right" as a
    button target;
  - `@emulator_action` (fast_forward_hold…);
  - `@capture` (a `HearthAction`, never sent to the emulator);
  - `none`.
- **Layers:**
  1. `data/controls/<system>.toml`;
  2. `data/controls/pads/<type>.toml` (PlayStation Create = Capture, touchpad = console Select;
     Xbox/Switch Share = Capture; controls the console lacks are skipped silently);
  3. `<config>/controls/systems/<system>.toml`;
  4. `<config>/controls/games/<system>/<serial>.toml`, or the region-free serial. The serial
     comes from the ROM header (`gc/GZ2E01`).
- **Stick options:** `[player1.sticks."C-stick"]` (invert_x, invert_y, deadzone, sensitivity)
  belongs to the console stick and follows it to whichever physical stick drives it.
  - There is no swap option: binding the sticks swaps them (swap-on-conflict).
  - Old `left_stick`/`right_stick` tables and `swap_sticks` are errors naming their
    replacement.
  - `layers::stick_setting(stick, field)` is the setting path used for provenance.
- **Editing:** `edit.rs` (`Editor`, `Setting`) writes only differences, with `toml_edit`
  (comments kept), swaps on conflict, and deletes a file once it is empty.
- **Translators:** `profiles/dolphin_input.rs` and `profiles/retroarch_input.rs` regenerate the
  emulator config at every launch.
  - `Capabilities::can_tune(stick)`: mupen64plus can only tune "Stick".
  - Goldens: `crates/hearth-core/tests/golden/`; accept changes with `HEARTH_UPDATE_GOLDEN=1`.
- **Device names:** `controls/device.rs` gives the pad's name for each emulator and
  `extra_inputs()`. Override in `<config>/controls/device.toml`.
- `hearth-shell --print-controls <id | key | title part>` shows bindings, options and sticks
  with the layer of each value, plus the generated files.
- **Live apply**: with InputPlumber, `InputPlumberApplier` rewrites the game's profile on every
  save and it loads on resume (`Applied`); the stick, trigger and vibration options say "Applies
  next time you start the game" on their own row. With gilrs: `EmulatorConfigApplier`
  (`NextLaunch`), page-wide note as before.

**Controls UI** (`ui/controls/`, `bridge/controls_page.rs`, `src/callouts.rs`,
`src/controller_layout.rs`)
- Settings → Controllers lists connected pads, Test buttons, and one row per console.
- The game-scoped page opens from the overlay dock (Controls) and Home → Options → Controls.
- **Drawing:** `assets/controllers/generic.svg` is generated by `scripts/controller-svg.py`
  from the lambdalighthouse gamepad. The source sits in `assets/controllers/source/`,
  gitignored.
  - Generated from it: `generic.toml` (anchors) and `generic/<id>.svg` (exact-shape highlights
    with fill, outline and glow baked in; sticks are a ring plus a cap).
  - Regenerate both: `HEARTH_WRITE_ANCHORS=1 cargo test -p hearth-shell controller_layout`.
    When adding an id, create a placeholder svg first: the Slint build needs the file.
- **Callouts:** computed in Rust at 1920×1080, then scaled.
  - Rows go in reading order (D-pad: up, left, right, down). A search picks elbows and a column
    shift so lines never cross and avoid other buttons; it falls back to shortest-length
    matching.
  - Tested at 1920×1080 and 1280×720.
  - `HEARTH_CALLOUT_PREVIEW=out.svg cargo test -p hearth-shell callout_preview` draws the
    layout.
- **Glyphs:** Kenney (`assets/glyphs/<family>/`); D-pad directions use the `_outline` variants.
  Inputs without a glyph get a pill with their name. The family comes from the first pad.
- **Button test:** `ControlsScreen` with `test-mode` (`TestPage`).
  - Held inputs light up; triggers show % with a bar; sticks show raw values and a dot.
  - PS sits below the drawing; holding Guide exits.
  - The header shows the controller, connection and battery.

**Input** (`hearth-platform/src/input/`)
- `--input=auto|inputplumber|gilrs` (`main.rs`, `select_input`). Auto uses InputPlumber when it
  runs and accepts writes; the log says which one and why.
- InputPlumber (`input/inputplumber/`): `client.rs` (zbus proxies, source pad identity),
  `mod.rs` (worker thread, modes, menu profile). The session drives it through `InputControl`
  (`Controller::set_mode` → Shell/Game; the button test → Test). `guard.rs` in the shell is the
  spawner of the `hearth-guard` child (`src/bin/hearth-guard.rs`, its own binary so
  `pgrep -x hearth-shell` only finds Hearth).
- Game profiles: `hearth-core/src/profiles/inputplumber_profile.rs` (`VirtualLayout`,
  goldens in `tests/golden/inputplumber/`), written to `<HEARTH_HOME>/runtime/inputplumber/`.
  `--print-controls` shows the profile when InputPlumber runs.
- `cargo run -p hearth-platform --example ip_probe` shows InputPlumber's state (read-only); use
  it to check the pad was handed back.
- Menu actions and Guide timing are shared in `input/pad_state.rs`.
- gilrs runs **without its deadzone and jitter filters**. Only the D-pad hat conversion is
  applied, by hand in `next_event`, so drift is visible. Navigation has its own 0.5 threshold.
- `PadActivity` carries:
  - pads (`PadInfo` with connection and battery, refreshed every 10 s);
  - pressed inputs and raw analog values;
  - the capture inputs (the session keeps them current: the game's mapping in game, the pad
    type's default in menus);
  - disarm.
- `Action::Capture` is sent on release and never injected into the UI.
- The DualSense on gilrs gives every standard button plus PS. It does not give the touchpad
  click (a separate evdev device) or the mic button. Share and Elite paddles come from raw
  evdev codes and are unverified.
- `cargo run -p hearth-platform --example pad_probe -- 10` lists pads and raw events
  (read-only).

**Art, Home and packs** (art round)
- `hearth-core::art`: `identity.rs` (art key `<system>/<serial>` or `<system>/name-<fnv64 of the
  cleaned title>`, search names, match selection), `provider.rs` (`MetadataProvider`,
  `MockMetadataProvider`), `steamgriddb.rs`, `cache.rs` (paths, local overrides, blurred hero),
  `service.rs` (the `hearth-art` worker, `remove_orphaned_art`).
- SQLite (`library.rs`, `user_version` 1): `games.favorite`, `games.art_key`, `art` (match or
  `no_match`, kinds found), `art_queue` (network/HTTP errors only, backoff 5 min doubling to 6 h;
  everything queued is retried at startup).
- Art is deleted only for games removed by a scan whose system folder still exists.
- Shell: `bridge/art.rs` decodes on the art worker into `SharedPixelBuffer`s; tiles are updated
  in place (`models::set_tiles`). Console icons: `ConsoleIcons` over `packs::Resolver`.
- `--mock-network`: an always-offline provider (cache only, games go to the queue).
- Packs: `.dev-data/packs/my-console-icons` and `.dev-data/packs/ps5-sounds` (the user's files,
  moved out of `assets/`; unmapped sounds in `sounds/unused/`). `hearth.toml` currently selects
  `my-console-icons`.
- Dev games root with dummies for the end-of-row case: `--games-dir ./.dev-data/games` (links to
  the two real ROMs plus 15 empty `Dummy … (USA).z64`). Running with it and then with `~/Games`
  removes the dummies from the library again.

## 2. Known issues and untested areas

- Untested by hand: everything in §0 "Next / open", plus:
  - presses made while a game is frozen;
  - the 15 s "no window" path;
  - the keyboard inside the overlay window;
  - Share and paddle codes.
- A game calling libwayland directly would still default to `wayland-0`. RetroArch is forced
  to X11 (`video_context_driver = "x"`).
- `retroarch.cfg` paths are relative with `HEARTH_HOME=./.dev-data`. They work, but should be
  absolute.
- N64: mupen64plus's "hold R2 for C buttons" mode is dropped; the right stick drives the C
  buttons.
- SDL names of non-DualSense pads are unverified (use `device.toml`).
- Performance: fullscreen nested gamescope runs at about 40 fps on Balanced and about 60 on
  Performance (the nested chain on an i5-1145G7, not Hearth).
- Cosmetic: at startup the tile row animates from 1280×720 to fullscreen.
- Not done: Uninstall, overlay dock actions other than Controls, ES-DE import, Steam.
- A games root that is missing at startup now leaves the library untouched (before, every game
  and its history was dropped).
- InputPlumber: physical Elite/Series paddle and Share names in profiles are unverified (no such
  pad here). Other controllers than the DualSense untested. On a box whose user isn't in
  `wheel`/`inputplumber`, polkit refuses the writes and auto falls back to gilrs.

## 3. How to run and test

- **Prerequisites:**
  - gamescope;
  - RetroArch with `/usr/lib/libretro/mupen64plus_next_libretro.so` and
    `retroarch-autoconfig-udev-git`;
  - Dolphin 2609;
  - quit Steam first.
- **Desktop, no overlay:** `HEARTH_HOME=./.dev-data cargo run -p hearth-shell -- --windowed`.
- **Real thing:** `scripts/run-gamescope.sh` (options: `HEARTH_GS_SIZE`,
  `HEARTH_GS_FULLSCREEN=1`, `HEARTH_GS_ARGS`).
- **Dev socket actions:** up, down, left, right, accept, back, prev_tab, next_tab, options,
  home_short, home_long, capture.
- **Keyboard:**
  - arrows; Enter/Space = Accept; Esc/Backspace = Back;
  - Q/E = tabs; O = Options;
  - F1/F2 = Guide short/long.
- **Logs:**
  - Hearth: stderr (`RUST_LOG=info,hearth_shell=debug`);
  - RetroArch: `.dev-data/retroarch/logs/`;
  - Dolphin: `.dev-data/dolphin/Logs/hearth-launch.log`;
  - library: `.dev-data/data/library.sqlite3`.
- **FPS counter:** `HEARTH_RETROARCH_ARGS="--appendconfig=data/dev/retroarch-fps.cfg"`.
- **Checks:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` (core 123, platform 35, shell 15 tests).
- **Artwork:** set `HEARTH_STEAMGRIDDB_KEY` or `[art] steamgriddb_key` in
  `.dev-data/config/hearth.toml` (gitignored). Without it: placeholders and one log line.

## 4. Gotchas

- **gamescope needs `-e`** (Steam mode). Without it there is no focus control and no overlay
  plane, and Hearth behaves as if outside gamescope.
- **Detecting gamescope** needs `GAMESCOPE_WAYLAND_DISPLAY` in the environment: KDE's Xwayland
  carries stale `GAMESCOPE_*` root properties.
- **Overlay = `STEAM_OVERLAY` only.** Keep `GAMESCOPE_EXTERNAL_OVERLAY` at 0, or the window is
  drawn twice. A hidden overlay must also be transparent (`panel: none`).
- **RetroArch:**
  - always `--config <generated cfg>`, never `~/.config/retroarch`;
  - every hotkey is bound to button 999 / axis +99, because `"nul"` falls back to the
    autoconfig profile and PS would open the menu;
  - X11 only inside gamescope.
- **Slint:**
  - globals are per window;
  - callbacks reach the session through `defer()`;
  - `@image-url` paths must exist at build time.
- **Clippy** rejects large `Err` types (`result_large_err`): map gilrs errors to `String`.

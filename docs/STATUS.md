# Hearth status

Last updated: 2026-10-17: M2a ("first game + overlay"), then Dolphin and the controls system
(sections 1b to 1e).
Read this first; then `AGENTS.md` (rules, commands) and `docs/plan.md` (architecture).

## 1. What M2a delivered

Launch a real N64 game from Hearth, open an overlay over it with the Guide button, go Home,
resume and close it. Verified end to end inside `scripts/run-gamescope.sh` with Ocarina of Time.

- **gamescope integration** (`crates/hearth-platform/src/gamescope.rs`, `Compositor` trait,
  `NoCompositor` outside gamescope). Hearth's window gets `STEAM_GAME=769`; each game gets its
  own app id (10000+) set on every window whose `_NET_WM_PID` is in the game's process group or
  its descendants (`process.rs`), retried for 15 s. Focus via
  `GAMESCOPECTRL_BASELAYER_APPID`, verified 0.7 s later through `GAMESCOPE_FOCUSED_APP`.
  Spike results: `docs/spikes/gamescope.md`.
- **Library** (`hearth-core`): `data/systems.toml` (N64 only, RetroArch + mupen64plus_next),
  read-only ROM scanner for `--games-dir` (default `~/Games`, layout `<dir>/n64/...`), No-Intro
  title cleanup, SQLite library (`.dev-data/data/library.sqlite3`: games, last played,
  playtime).
- **Runtime** (`runtime/process.rs`, plan.md §6.5): own process group, watcher thread,
  close = SIGCONT + SIGTERM then SIGKILL after 3 s, suspend/resume = SIGSTOP/SIGCONT on the
  group. Signals refuse pgid ≤ 1 and Hearth's own group. Games get `PR_SET_PDEATHSIG=SIGKILL`
  and an empty signal mask. Playtime excludes frozen time.
- **RetroArch profile** (`profiles/retroarch.rs`): generated, isolated config under
  `.dev-data/retroarch/`, all hotkeys disabled (see gotchas), log file enabled.
- **Session logic** (`crates/hearth-shell/src/bridge/session.rs`):
  - Home shows the real library (running game first, with a `success` dot). A or Play
    launches; Resume / Options → Close game (confirmation dialog) on the running tile.
  - Guide short: freeze the game, show the overlay (Now playing card with Resume, Go to
    Home and Close game; dock buttons are placeholders). Resume, Back or Guide again: unfreeze,
    show the game.
  - Guide long: freeze the game and show Home with focus on its tile.
  - When the game exits by itself, Hearth returns Home and saves playtime.
  - Outside gamescope (or gamescope without `-e`) there is no overlay; long press closes
    the game.
- **Safety and robustness**:
  - Games launched inside gamescope never reach the host Wayland session
    (`LaunchCommand::force_x11`).
  - Steam running → warning at startup.
  - Ctrl+C / SIGTERM / SIGHUP / closing the window close the game properly before exiting.
  - A crashed or killed Hearth takes its game with it (pdeathsig).
- **Dev tools**:
  - `scripts/run-gamescope.sh` and `scripts/send-action.sh`.
  - `--dev-socket`, `HEARTH_RETROARCH_ARGS`.
  - Examples: `gamescope_probe`, `overlay_probe`, `launch_timing`.
  - `data/dev/retroarch-fps.cfg`.

## 1b. Dolphin (GameCube, Wii)

- `gc` and `wii` systems (folders `gc`/`gamecube` and `wii`; the system comes from the folder,
  since both use .iso). The `.nkit` inner extension is dropped from titles.
- `profiles/dolphin.rs`: native `dolphin-emu` in PATH, else the Flatpak (only the native 2609
  build was tested). Batch mode, user dir `<HEARTH_HOME>/dolphin` (`-u`), every setting as a
  `-C` override (checked against the 2609 source and listed with reasons in the file).
  `Dolphin.General.UseDiscordPresence=False` is required: otherwise Dolphin writes
  `~/.local/share/applications/discord-455712169795780630.desktop` and edits
  `~/.config/mimeapps.list`.
- Window search counts **mapped** windows only: Dolphin's hidden Qt main window carries
  `_NET_WM_PID` and appears before the render window.
- Dolphin log: `.dev-data/dolphin/Logs/hearth-launch.log` (includes "Added device: SDL/0/...").

## 1c. Input: resume, focus, controls files

- Menu and overlay actions fire on button **release**. A press that began in a game is disarmed
  when Hearth takes the pad back (`PadActivity::disarm`).
- Resume unfreezes the game at once. Waiting until the pad was at rest was tried and removed:
  it got stuck (the pad never looked at rest), and players want input to reach the game as soon
  as they resume.
- A focus switch is retried for 3 s; on failure Hearth comes back and the game keeps running
  (never left frozen).
- **Controls files** (`crates/hearth-core/src/controls/`): Hearth pad by position → console
  controls (declared per system in `systems.toml`) or `@actions`, plus stick, trigger and
  vibration options. Layers: `data/controls/<system>.toml` → `<config>/controls/systems/<system>.toml`
  → `<config>/controls/games/<system>/<serial>.toml` (or the region-free serial, e.g. `GZ2.toml`).
  The serial is read from the ROM header (`gc/GZ2E01`, `n64/CZLE`).
- **Stick options belong to the console stick** (`[player1.sticks."C-stick"]`: invert_x,
  invert_y, deadzone, sensitivity) and follow it to whichever physical stick drives it. There is
  no swap option: binding `left_stick = "C-stick"` (swap on conflict) moves the stick with its
  options. The old `[player1.left_stick]`/`right_stick` tables and `swap_sticks` are errors that
  say what replaced them. Emulator actions (fast-forward…) are never bound by default; users add
  them per console or game.
- **Translators** (`profiles/dolphin_input.rs`, `profiles/retroarch_input.rs`) regenerate the
  emulator's input config at every launch: Dolphin GCPadNew.ini + Hotkeys.ini; RetroArch a core
  remap file, hotkey binds (raw buttons from the joypad autoconfig) and core options. Golden
  files in `crates/hearth-core/tests/golden/` (`HEARTH_UPDATE_GOLDEN=1` to accept changes).
- The controller's names for each emulator come from `controls/device.rs` (override:
  `<config>/controls/device.toml`, `sdl_name = "..."`).
- `hearth-shell --print-controls <game id | key | part of title>` prints the resolved mapping,
  the layer of every value and the generated files.

## 1d. Controls UI, glyphs, single instance

- **Single instance:** `flock` on `<data>/hearth.lock` (pid inside); a second Hearth exits
  naming the first. `--print-controls` may run alongside.
- **Console controls** are declared in `data/controls/<system>.toml` (`[[console]]`: name, kind,
  label, hint). Each stick and D-pad control also offers "<name> up/down/left/right" as a button
  target, so D-pad directions and L3/R3 take any button target.
- **`controls::edit`** writes a system or game file with `toml_edit`: only differences from the
  layers below, comments and order kept; taking a function from another button swaps them.
- **Glyphs:** Kenney Input Prompts in `assets/glyphs/<family>/<input>.svg`;
  `ui/components/glyphs.slint` (`ButtonGlyph`, `ControllerType`, set from the first pad's USB ids).
- **Drawing:** see 1e.
- **Settings → Controllers:** connected pads, Test buttons (hold Guide to exit), one row per
  console. **Controls page** (`ui/controls/`, `bridge/controls_page.rs`): Buttons tab with callouts
  and a choice menu, Sticks & options tab (only what the emulator supports), Reset with
  confirmation. Game-scoped from the overlay dock ("Controls") and Home → Options → Controls.
- **Live apply:** not possible today. Dolphin reads controller config only at boot
  (`Core.cpp` `Pad::LoadConfig`), RetroArch loads remaps with the content and has no network
  command for it. `MappingApplier` (`EmulatorConfigApplier` → `NextLaunch`) is where
  InputPlumber goes; the page says "Applies next time you start the game".

## 1e. Controller drawing, leader lines, Capture

- **Drawing:** `assets/controllers/generic.svg` is generated by `scripts/controller-svg.py` from
  lambdalighthouse's gamepad (source kept out of git in `assets/controllers/source/`). One color,
  themed with `colorize`. `generic.toml` (shape center and size per input) is derived from it:
  `HEARTH_WRITE_ANCHORS=1 cargo test -p hearth-shell anchors` rewrites it, the test fails when stale.
- **Callouts** (`src/callouts.rs`): columns by anchor side, rows matched to anchors by shortest
  total length with all elbows on one vertical line per side, so lines can't cross (tested at
  1920×1080 and 1280×720). `HEARTH_CALLOUT_PREVIEW=out.svg cargo test -p hearth-shell
  callout_preview` draws the layout. The focused input is highlighted on the drawing.
- **Glyphs:** Kenney's own Create/Options/View/Menu/−/+ and touchpad icons; inputs without a
  Kenney glyph (Share, paddles) show as a pill with their name. D-pad directions use Kenney's
  `_outline` variants: the pressed side is filled, so they stay distinct when tinted one color.
- **Capture** (`@capture`, `HearthAction`): a Hearth action on any console, never sent to the
  emulator. `data/controls/pads/<type>.toml` (between the console default and the user's files):
  PlayStation Create and Xbox Series / Switch Pro Share are Capture; the PlayStation touchpad
  takes the console's Select where there is one. The session keeps `PadActivity`'s capture
  buttons current (the game's mapping in game, the pad type's default in menus); the gilrs thread
  sends `Action::Capture` on release. For now a toast "Screenshots and clips are coming later";
  over a game it is the overlay's **notice** panel (no scrim, no freeze, game keeps focus).
- **Extras:** callouts below the drawing for the connected pad's touchpad, share and paddles
  (`ControllerIdentity::extra_inputs`). gilrs sees none of them on the DualSense (the touchpad is
  a separate evdev device); Share and Elite paddles are mapped from raw evdev codes, unverified.
  RetroArch can't read extras; Dolphin reads them through SDL (`Touchpad`, `Misc 1`, `Paddle n`).
- **Overlay panels** (`ui/overlay/`): the window routes input to `OverlayPanel` (none, menu,
  controls, notice); the quick menu is its own component.
- **Highlights:** `assets/controllers/generic/<id>.svg`, one per input shape (sticks: ring for
  movement, cap for the click), generated from the drawing with fill, outline and glow baked in
  (same command as the anchors). The focused input's is tinted with the accent over the drawing.
  Callout rows prefer reading order (D-pad up, left, right, down) when some elbows and a small
  column shift keep lines clear; otherwise rows are matched by length.
- **Button test** is `ControlsScreen` with `test-mode`: callouts show the input's name and live
  state (held inputs lit, several at once; trigger % with a bar; raw stick values and a dot in
  the ring), Guide below the drawing, header with controller, connection and battery.
- **Raw input:** gilrs runs without its deadzone/jitter filters (only the D-pad hat conversion),
  so drift is visible; navigation keeps its own 0.5 threshold. `PadActivity::analog()` has the
  first pad's raw sticks and triggers; `PadInfo` has `connection` (from SDL's GUID bus) and
  `power`. `cargo run -p hearth-platform --example pad_probe -- 10` lists pads and raw events.
  On the DualSense gilrs delivers every standard button and PS, not the touchpad click (separate
  evdev device) nor the mic button (not reported by hid-playstation).

## 2. Known issues and untested areas

**Untested or unverified**
- **Navigation with raw gilrs events:** stick and D-pad navigation in menus with a real pad
  after dropping gilrs's deadzone/jitter filters (only dev-socket input was tried).
- **Button test with real presses:** held highlights, trigger bars and the stick dot were only
  unit-tested; values show 0.00 until the stick first moves.
- **Capture with a real button press:** only sent through the dev socket. Create on the DualSense
  should give the toast in menus and the notice in game (and do nothing in the game).
- **Share / Elite paddle raw codes** (xpad `KEY_RECORD`, `BTN_TRIGGER_HAPPY5–8`, hid-nintendo
  `BTN_Z`): from the kernel drivers' sources, no hardware here.
- **Buttons pressed and released while a game is frozen:** whether the queued events reach the
  game after SIGCONT is still untested by hand (InputPlumber in M3 makes this moot).
- **Controls translation live:** checked by unit/golden tests and `--print-controls` only. Still
  to try by hand: the TP camera override, a fast-forward the user binds in TP, and OoT
  unchanged with the generated remap file.
- **15 s "no window" timeout:** the path that toasts, closes the game and keeps Hearth in front
  was never triggered live; RetroArch always mapped its window in about 0.25 s.
- **Keyboard inside the overlay window under gamescope:** untested. Controller actions are
  injected into it, so only keyboard use is affected.
- **Real Guide short/long timing inside gamescope:** only exercised through the dev socket;
  the user tested by hand and it worked.
- **Libwayland default socket:** `force_x11` removes `WAYLAND_DISPLAY`, but a game that calls
  libwayland directly still defaults to `wayland-0` (the host desktop). RetroArch is forced to
  X11 by its config (`video_context_driver = "x"`); SDL, GTK and Qt get X11 env hints.

**Known issues**
- **Relative profile paths:** with `HEARTH_HOME=./.dev-data`, the paths in the generated
  `retroarch.cfg` are relative (`./.dev-data/...`). They work because the game inherits Hearth's
  working directory, but should be made absolute in `DataDirs`.
- **Launching thread:** pdeathsig fires when the launching *thread* exits. Games must be launched
  from the UI thread (they are); never from a short-lived worker.
- **N64 C-button mode dropped:** mupen64plus-next's "hold R2 to turn the face buttons into C
  buttons" isn't a console control, so the generated remap unbinds R2, east and north. The right
  stick still drives the C buttons.
- **SDL names of non-DualSense pads** in `controls/device.rs` follow SDL3's HIDAPI drivers but
  are unverified; use `device.toml` if Dolphin doesn't see the pad.
- **Performance on the dev laptop:** fullscreen nested gamescope ran at about 40 fps on the
  Balanced power profile, and close to 60 on Performance (windowed: 60 on either). The cost is
  the nested chain (game → gamescope → KDE at 125% scaling) sharing the i5-1145G7/Iris Xe power
  budget, not Hearth (2.5% CPU while playing, debug build). The appliance should run on the
  Performance profile (M8).
- **Startup animation:** at startup the tile row animates from the 1280×720 preferred size to
  fullscreen (cosmetic).
- **Not done (out of M2a scope):**
  - README home row redesign (fixed focus anchor, label beside the tile, console icons);
  - Favorite, Uninstall and Properties in the game menu;
  - overlay dock actions;
  - the ES-DE import;
  - InputPlumber;
  - Steam.

## 3. How to run and test

**Prerequisites:** gamescope, RetroArch with `/usr/lib/libretro/mupen64plus_next_libretro.so`,
and joypad profiles (`retroarch-autoconfig-udev-git` from the AUR, installed to
`/usr/share/libretro/autoconfig/udev`). Quit Steam and any other Hearth instance first: they all
read the controller.

**Running Hearth**
- **Plain desktop, no overlay:** `HEARTH_HOME=./.dev-data cargo run -p hearth-shell -- --windowed`
- **Real thing:** `scripts/run-gamescope.sh`. Steam mode, the display's native size; extra
  arguments go to hearth-shell. Options:
  - `HEARTH_GS_SIZE=WxH` overrides the size.
  - `HEARTH_GS_FULLSCREEN=1` starts fullscreen.
  - `HEARTH_GS_ARGS="..."` passes extra gamescope flags.
  - For fullscreen tests use `powerprofilesctl set performance`.
  - Closest to the console: run the script from a text console (Ctrl+Alt+F3), so gamescope
    drives the display directly.

**Scripted driving**
- Start Hearth with `--dev-socket ./.dev-data/actions.sock`, then
  `scripts/send-action.sh accept home_short back home_long options down accept`.
- Action names: up, down, left, right, accept, back, prev_tab, next_tab, options, home_short,
  home_long.

**Keyboard**
- Arrows; Enter / Space = Accept; Esc / Backspace = Back; Q / E = tabs; O = Options;
  F1 = Guide short; F2 = Guide long.
- Inside gamescope the keyboard goes to whichever window is focused, so use the controller or
  the dev socket while a game is in front.

**Inspecting**
- gamescope state:
  `DISPLAY=:N cargo run -p hearth-platform --example gamescope_probe -- props|windows|tag <pgid> <app>|focus <app>...`.
  The display number is in the log ("Starting Xwayland on :N"); a second gamescope gets `:2`.
- Screenshots: `GAMESCOPE_WAYLAND_DISPLAY=gamescope-N gamescopectl screenshot $PWD/.dev-data/x.png`.
  These capture the base layer only: the overlay never shows in them, so check it by eye.
- FPS counter: `HEARTH_RETROARCH_ARGS="--appendconfig=data/dev/retroarch-fps.cfg" scripts/run-gamescope.sh`.
- Timing outside the UI:
  `cargo run -p hearth-core --example launch_timing -- ./.dev-data/retroarch <core> <rom> 600`.

**Logs**
- Hearth: stderr, with `RUST_LOG=info,hearth_shell=debug`.
- RetroArch: `.dev-data/retroarch/logs/retroarch.log` (video init, joypad autoconfig) and
  `.dev-data/retroarch/logs/hearth-launch.log` (its stdout/stderr).
- Library: `.dev-data/data/library.sqlite3`.
- Leftover games: `ps -C retroarch`. Kill only Hearth-spawned groups, and SIGCONT a frozen one
  first.

**Checks:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (core 67, platform 24, shell 6 tests).

## 4. Gotchas for a fresh session

- **gamescope needs `-e` (Steam mode).** Without it, `GAMESCOPECTRL_BASELAYER_APPID` is ignored,
  the newest window keeps the screen, the overlay plane isn't drawn and `GAMESCOPE_FOCUSED_APP`
  doesn't exist. Hearth detects this (`controls_focus() == false`) and behaves as if outside
  gamescope. The `SteamGameId` env var is ignored; tag windows yourself.
- **Overlay = `STEAM_OVERLAY` only.** 1 shows it, 0 hides it. Keep `GAMESCOPE_EXTERNAL_OVERLAY`
  at 0: with both set, gamescope draws the window on two layers (darker scrim, and two
  copies when the output is resized), and leaving it at 1 kept the window drawn while hidden.
  The overlay content is also transparent while hidden (`shown` property).
- **gamescope detection needs `GAMESCOPE_WAYLAND_DISPLAY`** in the environment (gamescope
  exports it to its children) *and* `GAMESCOPE_FOCUSABLE_WINDOWS` / `GAMESCOPE_FOCUS_DISPLAY`
  on the root window. KDE's own Xwayland (`:0`) carries stale `GAMESCOPE_*` properties
  (probably from Steam), so root properties alone give false positives.
- **RetroArch isolation:** always `retroarch --config <generated cfg>`; never read or write
  `~/.config/retroarch`. Every writable path is under `<HEARTH_HOME>/retroarch/`; only the joypad
  autoconfig folder (read-only system path) is outside.
- **Hotkey trick:** setting a RetroArch gamepad bind to `"nul"` makes it *fall back to the
  autoconfig profile*, and the DualSense profile binds the PS button to the menu
  (`input_menu_toggle_btn = "10"`). So every hotkey's `_btn` is bound to button `999` and
  `_axis` to `+99` (nonexistent), keyboard binds are `"nul"`, and `enable_hotkey` is bound to the
  nonexistent button too. This also defeats RetroArch's "autoconf menu toggle regardless of
  enable_hotkey" path (`input/input_driver.c`, v1.22.2).
- **RetroArch must use X11 inside gamescope:** `video_context_driver = "x"` plus the env rule.
  Otherwise it opens on the host desktop through `wayland-0`.
- **Two or more Hearth or gamescope instances at once** confuse testing: `:1` vs `:2`,
  `gamescope-0` vs `gamescope-1`, and every instance reacts to the controller. Check
  `ps -C hearth-shell,retroarch` before testing.
- **Slint:** globals are per window (the overlay window sets its own `Theme.scale`);
  `run_event_loop_until_quit` is used, so window close quits explicitly; Slint callbacks must
  call the session controller through `defer()` (it is in a `RefCell`), never directly.
- **Commits:** imperative, one logical change each, no Co-Authored-By trailer. Check that every
  commit builds (stash unstaged changes, run clippy and tests, then commit).

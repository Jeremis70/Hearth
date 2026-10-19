# Hearth (working name) — Architecture & Implementation Plan

> An open, controller-first living-room console for Linux. One library for your Steam games, emulated classics and native ports, with your own media alongside. No subscriptions, no ads, no lock-in.

Status: planning. Last updated 2026-10-16.

---

## 1. The goal in one paragraph

From the user's perspective there are no "consoles", only games. Every game is added through the same page, lives in the same grid, launches the same way, and is controlled with the same controller. While playing, a short press of the Home/PS button opens an overlay; a long press returns to the Hearth home screen with the game still running; pressing Options on the game's tile offers "Close game". This works identically for an N64 ROM, a GameCube disc image, a native port, or a Steam game.

## 2. Guiding rule: reuse before building

We only write our own component when no existing one does what we need. Summary of decisions:

| Need | Use (don't build) | We build |
|---|---|---|
| Fullscreen compositor, window switching, overlays | **gamescope** (Valve) | Small X11 client to set gamescope properties |
| Boot straight into our app | **gamescope-session-plus** (ChimeraOS, MIT) | One session file |
| Exclusive controller grab, virtual pad, Guide interception, overlay input | **InputPlumber** daemon (ShadowBlip, GPL-3.0, v0.69+) | D-Bus client (zbus) |
| Console definitions, file extensions, emulator detection rules for 100+ systems | **ES-DE's `es_systems.xml` + `es_find_rules.xml`** (MIT) | Converter into our `systems.toml` |
| Emulation | Standalone emulators + RetroArch/libretro cores | Per-emulator "profile" (CLI overrides) |
| Wi-Fi / Ethernet | **NetworkManager** via `nmrs` | Settings screens |
| Power (suspend/reboot/off) | **logind** via `logind-zbus` | Buttons |
| Steam library discovery | `steamlocate` | Steam `LibrarySource` + `Runtime` |
| Uniform grid art | **SteamGridDB** API v2 (via `ureq`) | `MetadataProvider`, cache |
| UI toolkit | **Slint** (winit backend) | All screens |
| Dev-time input without InputPlumber | `gilrs` | Small input adapter |

What genuinely has to be ours: the UI, the unified library model, the Runtime abstraction that hides console differences, and the glue to gamescope and InputPlumber.

Reference implementation to read (not use): **OpenGamepadUI** (ShadowBlip, Godot) already combines gamescope + InputPlumber into a gamepad launcher with an overlay. Its gamescope and InputPlumber integration code shows exactly which properties and D-Bus calls work in practice.

## 3. System architecture

```
greetd (autologin) → gamescope-session-plus  ─┬─ gamescope (compositor, display, focus, overlay planes)
                                              ├─ inputplumber.service (root daemon, owns controllers)
                                              └─ hearth (our app, CLIENTCMD)
                                                   ├─ main thread: Slint event loop (UI)
                                                   ├─ services thread: tokio runtime
                                                   │    ├─ InputPlumber client (zbus)
                                                   │    ├─ NetworkManager (nmrs)
                                                   │    └─ logind (logind-zbus)
                                                   ├─ library worker: scanning, SQLite, art
                                                   └─ session manager: game processes
                                                         ├─ steam -silent (background runtime only)
                                                         ├─ dolphin-emu / retroarch / azahar …
                                                         └─ native ports
```

Threading rule: Slint owns the main thread. Everything slow or async runs on worker threads and sends results back with `slint::Weak::upgrade_in_event_loop`. Only `Send` data crosses threads (e.g. `SharedPixelBuffer`, never `slint::Image`).

## 4. Workspace layout

```
hearth/
  Cargo.toml                    # [workspace]
  data/
    systems.toml                # generated from ES-DE data + our overrides
    profiles/                   # per-emulator override files (RetroArch appendconfig, etc.)
  tools/
    import-esde/                # converter: es_systems.xml + es_find_rules.xml → systems.toml
  crates/
    hearth-core/                # platform-neutral, no UI, no D-Bus
      model.rs                  #   Game, GameId, SystemId, LaunchTarget
      systems.rs                #   load systems.toml, resolve emulator command
      sources/{mod,roms,steam,ports}.rs   # LibrarySource trait + impls
      library.rs                #   SQLite cache, queries (recent, by system, favorites)
      runtime/{mod,process,steam}.rs      # Runtime trait + impls
      profiles/                 #   per-emulator config/CLI override logic
      config.rs
    hearth-platform/            # Linux integration behind traits
      gamescope.rs              #   x11rb: focus, overlay, app ids
      input/{mod,inputplumber,gilrs}.rs   # InputBackend trait + 2 impls
      network.rs                #   nmrs + mock
      power.rs                  #   logind-zbus
    hearth-shell/               # Slint app
      build.rs
      ui/ app.slint theme.slint home.slint library.slint overlay/
          settings/*.slint components/{tile,chip,osk,folder_picker,menu}.slint
      src/ main.rs  app_state.rs  bridge/*.rs  images.rs
```

## 5. Crates

| Crate | Version checked | Purpose |
|---|---|---|
| `slint`, `slint-build` | 1.18 | UI, `.slint` compiled at build time |
| `gilrs` | 0.11 | Controller input on the dev laptop / non-InputPlumber systems |
| `zbus` | 6.0 | D-Bus client for InputPlumber, blocking API (no tokio). 6.0 needs rustc 1.89, which is the workspace's `rust-version` |
| `nmrs` | 3.5 | NetworkManager: list/connect Wi-Fi, device state, change monitoring (async, tokio) |
| `logind-zbus` | 5.3 | Suspend / reboot / power off |
| `x11rb` | 0.14 | Setting gamescope X11 properties |
| `tokio` | 1.x | Runtime for the services thread (nmrs/zbus) |
| `steamlocate` | 2.1 | Find Steam install, libraries, installed apps |
| `ureq` | 3.x | Simple blocking HTTP (Steam Web API, SteamGridDB art; its v2 API is a few GETs, so no client crate) |
| `rusqlite` (feature `bundled`) | 0.40 | Library cache, playtime, favorites |
| `walkdir` | 2.5 | Folder scanning |
| `notify` | 8.x stable (9 is RC) | Watch game folders |
| `quick-xml` + `serde` | 0.42 | Import ES-DE XML |
| `serde`, `toml` | — | Config + systems.toml |
| `rust-ini` | 0.21 | Emulator INI edits when CLI overrides aren't enough |
| `procfs` | 0.18 | Find Steam game processes by environment |
| `nix` | 0.31 | `killpg`, SIGTERM/SIGKILL/SIGSTOP/SIGCONT |
| `image` | 0.25 | Decode/resize art off the UI thread |
| `directories` | 6.0 | XDG config/data/cache paths |
| `crossbeam-channel` | 0.5 | Worker ↔ UI messaging |
| `tracing`, `tracing-subscriber` | 0.1 | Logging |
| `anyhow`, `thiserror` | — | Errors |

## 6. Component implementation

### 6.1 Slint shell

- `build.rs`: `slint_build::compile("ui/app.slint")`; `main.rs`: `slint::include_modules!()`.
- Data flows into Slint as models: `Rc<VecModel<GameTile>>`, with `slint::FilterModel` for console filters and `slint::SortModel` for sorting, so filtering costs no Rust-side copies.
- `GameTile` struct exposed from Slint: `id`, `title`, `system_short`, `art: image`, `installed`, `running`.
- Art: worker decodes and resizes with `image` into `SharedPixelBuffer<Rgba8Pixel>`, sends to UI thread, which wraps it in `slint::Image::from_rgba8` and updates the row. Placeholder tile until loaded.
- Navigation: each screen is a `FocusScope` handling key events; focus index lives in Slint properties. Don't rely on Slint's tab focus chain for 2D grids.
- Grid virtualization: a vertical `ListView` whose rows are fixed-width rows of tiles (Slint's ListView virtualizes rows).
- Window: fullscreen via `window.set_fullscreen(true)` or `--windowed` flag for development; force winit backend (`SLINT_BACKEND=winit`), not LinuxKMS, because gamescope is the compositor.
- Two windows: main shell and overlay (a second component instance), see 6.4.

### 6.2 Input

```rust
pub enum Action { Up, Down, Left, Right, Accept, Back, PrevTab, NextTab, Options, HomeShort, HomeLong, Capture }

pub trait InputBackend: Send {
    fn start(&mut self, tx: Sender<Action>) -> Result<(), InputError>;
}
// The session keeps an `InputControl` handle: set_mode(Shell | Game | Test),
// set_game_profile(path), release(timeout). Detached (no-op) for gilrs.
```

`--input=auto|inputplumber|gilrs` (default auto: InputPlumber if it runs, is ≥ 0.69 and accepts
Hearth's first write, else gilrs, with the reason logged). Verified API, quirks and polkit
behaviour: `docs/spikes/inputplumber.md`.

**InputPlumber backend (appliance)** (`hearth-platform::input::inputplumber`, zbus 6 blocking,
own threads). InputPlumber exposes one composite device per physical controller
(`org.shadowblip.Input.CompositeDevice`) with an `InterceptMode` property (0 None, 1 Pass,
2 Always). Intercepted input arrives as `InputEvent(name, value)` signals (`ui_accept`,
`ui_guide`, …) from `org.shadowblip.Input.DBusDevice`.

Flow:
1. Start: set `ManageAllDevices` (external pads are only taken with it), give each controller one
   virtual `xbox-elite` pad, `Always` + Hearth's menu profile (identity, touchpad and Mute to
   D-Bus). Composite devices without capabilities (a 0.82 race splits a DualSense's evdev nodes
   off) get no virtual pad.
2. Game focused → load the game's profile, then `Pass`.
3. Guide → InputPlumber switches to `Always` by itself and sends `ui_guide`; Hearth times it
   (~500 ms short/long, menu buttons fire on release, presses from the game are disarmed).
4. Back to Hearth (overlay, Home) → `Always` + menu profile, so a game's remap never affects
   Hearth's menus. Outside gamescope a short press re-asserts `Pass`.
5. Exit, panic of the input thread, SIGTERM/SIGINT/SIGHUP → every controller `None` + default
   profile, `ManageAllDevices` off. SIGKILL or a crash: the `hearth-guard` child (it reads a
   close-on-exec pipe from Hearth) does the same.

**Remapping and emulators.** Emulators always read the virtual pad (Dolphin
`SDL/0/Xbox One Elite 2 Controller`; RetroArch through its `sdl2` driver and Hearth's profile
for the pad) laid out like the console's shipped default, with emulator actions on its four
paddles, Share and free buttons (`profiles::inputplumber_profile::VirtualLayout`). A device
allowlist keeps every other pad away from them (§6.6). The Elite 2 pad was chosen over the
Xbox 360 one for those five spare buttons, which SDL and RetroArch both read.

**Live vs next launch.** The user's layers become a `device_profile_v1` profile
(`runtime/inputplumber/game.yaml`) routing each physical input to the virtual input the emulator
reads; every save on a controls page rewrites it and it is loaded when the game is resumed, so
button and stick remaps, emulator actions and Capture apply live. A profile can't express stick
inversion (only both axes at once), deadzone, sensitivity, trigger mode or vibration: those stay
in the emulator configs, apply at the next launch, and each option says so.

**InputPlumber config.** `data/inputplumber/devices.d/60-ps5_ds_gamepad.yaml`, installed in
`/etc/inputplumber/devices.d/` on the box, gives the DualSense the Elite pad from the moment it
is taken (InputPlumber's own config starts with a virtual DualSense).

**Things going away.**
- InputPlumber stops or restarts: a game in front is frozen under the overlay; gilrs reads the
  controllers for Hearth meanwhile; when InputPlumber is back (any time) Hearth takes the
  controllers again; back within 2 s, the game resumes by itself. A game started meanwhile
  keeps the physical pad until it ends.
- The controller driving the game disconnects: frozen under the overlay ("Reconnect your
  controller"); on reconnect, Resume focused, so the emulator picks up the new virtual pad while
  the game can't move. Another pad: a notice only.
- Controller notices (no pause): battery ≤ 20 % and ≤ 10 % once per charge (kernel
  `power_supply`, also on gilrs), connected / disconnected.

Glyphs and the controller name come from the composite device's source devices, not the virtual
pad.

**gilrs backend (laptop / fallback).** Reads controllers directly, maps D-pad + left stick (deadzone 0.5) to actions with key repeat (400 ms delay, 80 ms rate), and times Guide for short/long press. It can't hide input from a running game, which is fine for development. Emulator configs then target the physical pad and do the remapping themselves. With InputPlumber, a gilrs observer reads the virtual pad for the button test's raw stick and trigger values.

Both backends feed the same channel; the bridge converts `Action` into Slint key events (`window.dispatch_event(WindowEvent::KeyPressed…)`), so the keyboard works too during development. Setting: Nintendo/Xbox A-B swap.

### 6.3 Gamescope integration (`hearth-platform::gamescope`)

Uses `x11rb` against gamescope's Xwayland (`DISPLAY`).

- Every window gamescope should show needs an app id via the `STEAM_GAME` window property. Hearth's main window uses `769` (the value gamescope-session-plus documents for a custom main client).
- We give each game session its own app id. Planned approach: launch the game with `SteamGameId=<id>` in its environment, which gamescope uses to associate windows with an app; fallback: find the game's windows via `_NET_WM_PID` and set `STEAM_GAME` ourselves.
- Switch what's on screen by writing the root-window property `GAMESCOPECTRL_BASELAYER_APPID` (list of app ids, first is on top): `[game_id, 769]` to show the game, `[769]` to show Hearth.
- Overlay: the overlay window gets `GAMESCOPE_EXTERNAL_OVERLAY = 1`, so gamescope draws it above the game.
- Property names come from gamescope / OpenGamepadUI usage; verify in the spike before building on them.

Development on the laptop: `gamescope -W 1920 -H 1080 -- hearth` runs everything nested inside KDE. Without gamescope (plain `cargo run`), this module becomes a no-op and launched games simply open their own fullscreen windows.

### 6.4 Overlay

- Second Slint window, transparent background, marked as external overlay.
- Contents: game title + playtime, Resume, Go to Home, Close game, quick settings (volume, Wi-Fi status), clock and battery.
- Shown on HomeShort, while InputPlumber is in `Always` so the game gets no input.

### 6.5 Runtime: the contract that hides consoles

```rust
pub trait Runtime: Send + Sync {
    fn launch(&self, game: &Game, ctx: &LaunchCtx) -> Result<Session>;
    fn is_running(&self, s: &Session) -> bool;
    fn suspend(&self, s: &Session) -> Result<()>;  // optional freeze when leaving to Home
    fn resume(&self, s: &Session) -> Result<()>;
    fn close(&self, s: &Session) -> Result<()>;
}
```

`SessionManager` holds the current session, records playtime, and reports state changes to the UI ("running" badge on the tile, Options menu shows "Close game").

**ProcessRuntime** (emulators, native ports, anything we spawn):
- `Command::new(..).process_group(0).env("SteamGameId", app_id)`; store pid = pgid.
- Watcher thread: `child.wait()` → session ended → update playtime, refocus Hearth.
- `close`: `killpg(SIGTERM)`, wait up to 3 s, then `SIGKILL`.
- `suspend/resume`: `SIGSTOP/SIGCONT` on the group, enabled per system (great for emulators; default off for anything online).

**SteamRuntime:**
- Ensure `steam -silent` is running (started once at session start).
- `launch`: run `steam steam://rungameid/<appid>`.
- Detect the game's processes: poll `procfs` every 500 ms for processes whose environment contains `SteamAppId=<appid>`; running while any exist.
- `close`: SIGTERM those process groups, then SIGKILL after timeout.
- `suspend`: off by default.
- Disable Steam's own Guide/overlay shortcut; Steam only ever sees the InputPlumber virtual pad.

### 6.6 Systems, emulators and profiles

`tools/import-esde` parses ES-DE's `es_systems.xml` (systems: name, full name, extensions, launch commands with placeholders such as `%ROM%`, `%CORE_RETROARCH%`, `%EMULATOR_…%`) and `es_find_rules.xml` (where each emulator lives: PATH names, Flatpak IDs, AppImage locations, RetroArch core folders) into our `systems.toml`. Keep ES-DE's MIT notice in `data/`. We then add our own fields on top: default emulator per system, suspend policy, profile id.

```toml
[[system]]
id = "gc"
name = "Nintendo GameCube"
short = "GameCube"
extensions = ["iso","rvz","gcz","ciso","gcm"]
folders = ["gc","gamecube"]
default_emulator = "dolphin"
suspend = "sigstop"

  [[system.emulator]]
  id = "dolphin"
  command = "{emu:dolphin} -b -e {rom}"
  profile = "dolphin"
```

Emulator detection order: PATH binary → Flatpak (`flatpak info <id>`) → AppImage folder. Missing emulator = game shown greyed with "Install Dolphin to play".

**Profiles** make every emulator behave like part of one console. Checklist for each one:
- Fullscreen, no emulator UI, pause on focus loss, saves under `~/.local/share/hearth/saves/<system>/`.
- Controller: InputPlumber's virtual Xbox Elite 2 pad, laid out like the console's default; emulator actions on its paddles, Share and free buttons, everything else unbound (no pad button reaches the emulator's menu or hotkeys).
- The emulator only ever sees the InputPlumber virtual pad (device allowlist), never raw pads: a reconnecting controller is visible for a moment before InputPlumber hides it. SDL emulators get `SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT=0x045e/0x0b00`; RetroArch uses its `sdl2` joypad driver for it, and only Hearth's own joypad profile (its autoconfig folder is Hearth's).
- Prefer non-destructive overrides: RetroArch `--appendconfig=data/profiles/retroarch.cfg`; Dolphin `-C Section.Key=Value` overrides; flags like `-fullscreen` for PCSX2/DuckStation. Only edit INI files (with `rust-ini`) when an emulator has no CLI override.
- Each profile is verified when its system is added (CLI flags change between versions).

Recommended defaults: RetroArch cores for NES/SNES/GB/GBC/GBA/Genesis/N64/PS1/PSP; standalone for NDS (melonDS), 3DS (Azahar), GameCube/Wii (Dolphin), PS2 (PCSX2), Wii U (Cemu). Install emulators as Flatpaks on the final box to keep the base OS minimal.

### 6.7 Library and sources

```rust
pub trait LibrarySource: Send {
    fn id(&self) -> SourceId;                     // "roms", "steam", "ports"
    fn scan(&self, ctx: &ScanCtx) -> Result<Vec<Game>>;
    fn install_options(&self) -> Vec<Installable>; // for the unified "Add games" page
    fn uninstall(&self, game: &Game) -> Result<()>;
}
```

- **RomSource:** walk configured roots (`walkdir`), map `<root>/<system folder>/…` to systems, fall back to unambiguous extensions. `.m3u` hides its member discs; folder-based games where the system says so. Title cleanup from No-Intro/Redump style names.
- **SteamSource:** `steamlocate` for installed apps; owned-but-not-installed via Steam Web API `GetOwnedGames` (user's API key) for the install page; art from Steam's local library cache.
- **PortSource:** a small catalog of native ports (e.g. Ship of Harkinian) offered when the matching ROM is in the library; launched via ProcessRuntime.
- SQLite tables: `games`, `sessions` (playtime), `favorites`, `art`. Startup: show cached library instantly, rescan in background, `notify` watches roots.
- Art: SteamGridDB grid images (uniform aspect ratio across consoles) cached under `~/.cache/hearth/art/`; local `<rom>.png` overrides.

### 6.8 Unified "Add games" page

One page listing installables from every source:
- Steam: owned, not installed → Install triggers `steam://install/<appid>`, which shows Steam's own small controller-navigable dialog inside gamescope.
- Import: when a USB drive or network share is mounted, game dumps found on it are listed; Import copies them into the right `Games/<system>/` folder with progress.
- Ports: available when the ROM is present.

Uninstall is one button on the game's Options menu: delete file (with confirmation), `steam://uninstall/<appid>`, or remove port.

### 6.9 Settings

- Network (`nmrs`): list networks with strength, connect with WPA password via on-screen keyboard, wired status, live updates via its change monitors. `--mock-network` uses a fake backend so laptop testing doesn't drop real Wi-Fi.
- Game folders: controller folder picker, rescan, counts per console.
- Emulators: per system, detected/missing, choose default, BIOS folder + "BIOS found/missing".
- Controllers: A/B layout, button test.
- System (`logind-zbus`): suspend, restart, power off; About screen (includes the Slint attribution, see §9).

### 6.10 Boot (final box)

- Minimal Arch/CachyOS, no desktop environment.
- `greetd` autologin → `gamescope-session-plus hearth`.
- `/usr/share/gamescope-session-plus/sessions.d/hearth` sets `CLIENTCMD=hearth`.
- `inputplumber.service` enabled.

## 7. Milestones

| # | Milestone | Done when |
|---|---|---|
| S0 | **Spike** (throwaway) | Inside nested gamescope on the laptop: a Slint window shows with `STEAM_GAME=769`; launching RetroArch with an app id and writing `GAMESCOPECTRL_BASELAYER_APPID` switches between them; an overlay window draws over the game. InputPlumber installed on the laptop: switching `InterceptMode` and receiving Guide over D-Bus works |
| M0 | Skeleton | Slint home row navigable with controller (gilrs) and keyboard |
| M1 | Settings + Wi-Fi | Join Wi-Fi with password using only the controller (mock + real); on-screen keyboard and folder picker components done |
| M2 | Library + first launch | ES-DE import tool; add `~/Games`; Ocarina of Time appears, launches via ProcessRuntime + RetroArch profile, quitting returns to Hearth, playtime recorded |
| M3 | Console experience | Gamescope focus switching, InputPlumber backend, Home short/long press, overlay, Options → Close game |
| M4 | Unified library UX | Grid, filters, recent row, SteamGridDB art, missing-emulator state |
| M5 | More consoles | 3DS, GameCube/Wii (Twilight Princess), PS1/PS2/PSP, NDS, GBA, each with a verified profile |
| M6 | Steam | SteamSource + SteamRuntime; Steam games behave identically in grid, launch, overlay, close |
| M7 | Add games / uninstall | Unified page: Steam install, USB/network import, ports |
| M8 | Appliance image | greetd + gamescope-session-plus + InputPlumber on the new hardware |
| Later | Media (Jellyfin + libmpv), embedded libretro, Windows port of the non-Linux parts | |

## 8. Risks / things to prove early (S0 spike)

1. Gamescope app-id assignment for non-Steam processes (`SteamGameId` env vs setting `STEAM_GAME` on found windows).
2. InputPlumber D-Bus event delivery and reliable short/long Guide detection.
3. Slint window → X11 window id for setting properties (winit accessor or `_NET_WM_PID` lookup via x11rb).
4. Steam running headless (`-silent`) in a non-Steam gamescope session, and Steam-launched games appearing under our app ids.
5. Some emulators may ignore pause-on-focus-loss; SIGSTOP covers those.

## 9. Licensing notes

- **Slint** is available under GPL-3.0, a royalty-free license, or a commercial license. The royalty-free license covers desktop apps but explicitly excludes embedded systems and requires showing the AboutSlint widget or a badge. Simplest path: license Hearth as **GPL-3.0-or-later** and use Slint under GPL; InputPlumber is GPL-3.0 too (though we only talk to it over D-Bus).
- ES-DE data is MIT; keep its license notice with the generated `systems.toml`.
- Users supply their own game dumps and BIOS files; Hearth never downloads ROMs.

## 10. Open decisions

1. Final name (shortlist: Hearth, Homeplay, Hearthside, Freeplay).
2. Default emulator per system (recommendation in §6.6).
3. Flatpak vs system packages for emulators on the final box (recommend Flatpak).

# Spike: InputPlumber as Hearth's controller layer

Date: 2026-10-18. InputPlumber 0.82.0 (Arch package, system service) on CachyOS, DualSense over
Bluetooth, user in `wheel`. Calls made with `busctl` / `gdbus`; virtual devices read with
`evtest` (no grab) and a small SDL3 3.4 probe; source read from the v0.82.0 tag and from
OpenGamepadUI (OGUI) master.

## Results

| Question | Result |
|---|---|
| Polkit for writes (`ManageAllDevices`, `InterceptMode`, `LoadProfilePath`, `SetTargetDevices`) | **No prompt** as a `wheel` member (shipped rules return YES for `wheel` and `inputplumber`). Every write is `auth_admin` otherwise; gamescope has no polkit agent, so non-wheel users would get an error |
| External DualSense managed by default | **No.** Only when `ManageAllDevices = true` (or a device config with `auto_manage`). The property is in memory only, false after every daemon restart |
| `ManageAllDevices = true` | `CompositeDevice0` appears within ~1 s, raw evdev nodes become `root:root 000` (hidden), targets `ds5 + mouse + keyboard` (the config's default) |
| `ManageAllDevices = false` | Composite device and virtual pads go away, the raw DualSense comes back with its ACL |
| `InterceptMode = 2` (Always) | All input goes to D-Bus. **Zero** evdev events on the virtual pad during a full button sweep |
| `InterceptMode = 1` (Pass) | Buttons reach the virtual pad. A Guide press sends `ui_guide 1`/`0` over D-Bus, never to the pad, and the device switches to Always by itself |
| Auto-switch Pass → Always | **No `PropertiesChanged` is emitted** (our own writes do emit it). Reading the property returns 2. Don't rely on the signal |
| Button held when Guide is pressed in Pass | The virtual pad gets its release at the switch (no stuck button in the game). The later physical release is **not** sent over D-Bus (the D-Bus target drops releases it never saw pressed) |
| Events in Always | `ui_accept`, `ui_back`, `ui_context`, `ui_action`, `ui_option`, `ui_select` (DualSense Create), `ui_guide`, `ui_volume_mute` (Mute), `ui_l1`/`r1`/`l2`/`r2`/`l3`/`r3`, `ui_up/down/left/right` from D-pad **and** left stick. Right stick, paddles, touchpad: dropped. No analog values |
| Custom D-Bus names in a profile (`dbus: hearth_touchpad`) | Arrive as `InputEvent('hearth_touchpad', 1.0/0.0)`, in **both** Always and Pass, and never reach the virtual pad |
| `LoadProfilePath` on a file in the repo (`.dev-data/…`) | Works (root daemon reads `/home`; 512 KiB limit). Instant |
| `SetTargetDevices([xbox-elite, keyboard, mouse])` | Works live; keyboard and mouse are kept, the gamepad is recreated as `gamepad1` |
| `SendEvent` | **Daemon bug in 0.82.0:** the async handler calls `blocking_send` and panics a tokio worker (`client.rs:471`); the call times out. The daemon survives. Don't use it |
| Rumble via virtual Elite pad (SDL `SDL_RumbleGamepad`, the path Dolphin uses) | SDL reports success; **the DualSense (Bluetooth) did not vibrate.** InputPlumber's own `ForceFeedback.Rumble` on the D-Bus target didn't either, so it's inside InputPlumber, not our config. Not yet tried over USB |

## Virtual pad: `xbox-elite`

"Microsoft X-Box One Elite 2 pad", 045e:0b00. Keys: `BTN_SOUTH…BTN_THUMBR`, `BTN_MODE`,
`BTN_TRIGGER_HAPPY1–8`, `KEY_RECORD`; ABS sticks/triggers/hat; `FF_RUMBLE`.

SDL3 names it "Xbox One Elite 2 Controller" with a built-in mapping including `misc1` and
`paddle1–4`. Profile target → SDL button → evdev → RetroArch udev button number:

| InputPlumber target | SDL | evdev | RetroArch udev |
|---|---|---|---|
| `RightPaddle1` | paddle1 | `BTN_TRIGGER_HAPPY5` | 15 |
| `LeftPaddle1` | paddle2 | `BTN_TRIGGER_HAPPY7` | 17 |
| `RightPaddle2` | paddle3 | `BTN_TRIGGER_HAPPY6` | 16 |
| `LeftPaddle2` | paddle4 | `BTN_TRIGGER_HAPPY8` | 18 |
| `Screenshot` | misc1 | `KEY_RECORD` | 19 |

RetroArch's udev numbering (`BTN_JOYSTICK…KEY_MAX` first, then the lower codes) matches SDL's
`b15–b19`; to be confirmed in a game. RetroArch ships an autoconfig for 1118:2816
("Generic X-Box pad").

Five spare buttons, all visible to both emulators, so emulator actions go there; a virtual
keyboard (needs window focus, uncertain F13+ keysyms) isn't needed.

## API names (0.82.0)

- Bus: system, `org.shadowblip.InputPlumber`; ObjectManager at `/org/shadowblip/InputPlumber`.
- Manager `/org/shadowblip/InputPlumber/Manager`, `org.shadowblip.InputManager`:
  `ManageAllDevices b` (rw), `GamepadOrder as`, `Version s`, `SupportedTargetDeviceIds as`.
- Composite `/org/shadowblip/InputPlumber/CompositeDevice{N}`,
  `org.shadowblip.Input.CompositeDevice`: `InterceptMode u` (0 None, 1 Pass, 2 Always,
  3 GamepadOnly), `LoadProfilePath(s)`, `LoadProfileFromYaml(s)`, `GetProfileYaml`,
  `SetTargetDevices(as)`, `SetInterceptActivation(as, s)`, `SendEvent(s, v)` (broken),
  `SendButtonChord(as)`, `Stop()`; properties `Name`, `PersistentId` (BT MAC here),
  `ProfileName`, `ProfilePath`, `SourceDevicePaths` (`/dev/hidraw6` + the blocked
  `/dev/input/eventN`), `Capabilities`, `TargetDevices`, `DbusDevices`.
- Events: `/org/shadowblip/InputPlumber/devices/target/dbus{N}`,
  `org.shadowblip.Input.DBusDevice.InputEvent(s name, d value)`.
- Source info: `/org/shadowblip/InputPlumber/devices/source/hidraw{N}`,
  `org.shadowblip.Input.Source.UdevDevice`: `IdVendor` / `IdProduct` (lowercase hex strings),
  `Name`, `IdBustype` (decimal string, 3 USB, 5 Bluetooth).
- There is no "unload profile": load `/usr/share/inputplumber/profiles/default.yaml` (what a new
  device gets).
- `busctl monitor` needs root (`BecomeMonitor` denied); `gdbus monitor --system --dest
  org.shadowblip.InputPlumber` works as a user. busctl warns about `direction="in"` on signal
  args in the daemon's introspection XML (harmless).

## Profile schema (device_profile_v1)

Can: button→button (the source loses its own meaning, so swaps work), button→key,
button→`dbus:` name, axis/trigger→button with a `deadzone` threshold, LeftStick↔RightStick,
button→axis direction. Unmapped inputs pass through.

Can't: per-axis invert (`invert` flips both axes), stick deadzone or sensitivity, vibration
on/off, axis→trigger. These stay in the emulator configs.

## Compared with OGUI and the docs

- OGUI: on start sets `ManageAllDevices = true`, Always, `SetInterceptActivation([Guide],
  Guide)`, and repeats it for each new device (new devices start in None). Pass while a game is
  focused. Listens to `InputEvent` on the first `DbusDevices` path. Loads the default profile
  on Guide so menus ignore game remaps. Profiles through `LoadProfilePath` (JSON). **Restores
  nothing on exit.**
- The docs and bindings XML don't list `SetTargetDevices`, `SetInterceptActivation`,
  `LoadProfileFromYaml`, the release filtering, or that a client that disappears leaves the
  device in its last mode (the daemon doesn't watch clients).
- The D-Bus policy has a commented TODO to deny receiving signals later; that would break
  `InputEvent` listening.

## Decisions

- Hearth sets `ManageAllDevices = true` while it runs, and false again on exit, panic, signals
  and through a guard process (SIGKILL). Startup also resets any composite device it finds.
- Always while Hearth is visible, Pass while a game has focus.
- Two profiles: a fixed menu profile (identity plus the touchpad and Mute to D-Bus) and a
  per-game profile generated from the controls layers (Capture to D-Bus `hearth_capture`).
- Virtual pad: `xbox-elite` alone (no touchpad-as-mouse, no keyboard needed); emulator actions
  on the five spare buttons.
- Never call `SendEvent`.

## Found while building the backend

- **Split DualSense.** Turning `ManageAllDevices` on sometimes gives one DualSense two to four
  composite devices: the hidraw one with all 33 capabilities, and others holding only the blocked
  evdev nodes (no capabilities, same `PersistentId`), each with its own virtual pad. Not seen in
  the first spike run, then on every later one. Hearth removes their targets
  (`SetTargetDevices([])` works) and ignores them. The hidraw-only composite has no evdev name,
  so the controller's name falls back to the composite `Name`
  ("Sony Interactive Entertainment DualSense Wireless Controller").
- **Signal flood.** InputPlumber adds and removes source and target objects constantly
  (`InterfacesAdded`/`Removed` under `devices/`), and its device manager retries virtual nodes
  for 4 s each ("Unable to create initialized UdevDevice … after 80 attempts"). Listing devices
  on every signal kept Hearth's input thread busy for seconds; only `CompositeDevice*` paths are
  watched now, debounced.
- **Face button names (corrected 2026-10-19).** InputPlumber names the left and top face
  buttons by evdev code, not by position: its DualSense driver reports Square as
  `GamepadButton::North` and Triangle as `West`, and its Xbox pads write `North` as code 307
  (`BTN_NORTH`, which is xpad's `BTN_X`, the left button) and `West` as 308. evtest on the
  virtual pad: Square 307, Triangle 308. SDL 2 and 3 map the pad `x:b2,y:b3` (307 is X, left),
  so emulators read it right. Hearth first used InputPlumber's names as positions: identity
  mappings came out right by two cancelling mistakes, but anything bound to west or north hit
  the other button (N64 B, on west, landed on Triangle) and the D-Bus menus read `ui_action`
  as west. The earlier note here said the codes were fine and swapped them back in gilrs; that
  hid it. Fixed where Hearth names InputPlumber's buttons (`inputplumber_profile::button_name`,
  `position_of`); gilrs gets SDL's mapping for the pad instead of a swap. Verified with an SDL2
  probe on the generated GameCube and N64 profiles and by hand in both emulators.
- **Triggers in gilrs.** Without an SDL mapping gilrs reports the virtual pad's `ABS_Z`/`ABS_RZ`
  as axes `LeftZ`/`RightZ` from -1 (released) to 1. Hearth now gives gilrs the pad's mapping
  (`gilrs::ELITE_2_MAPPING`); the axis fallback stays for other unmapped pads.
- **Profiles.** `Mute` is accepted as a button name although the JSON schema doesn't list it.
  A mapping with `target_events: []` drops the input. All generated profiles load without
  warnings.
- **Live remap.** `LoadProfilePath` while the game is frozen, then `Pass` on resume: the new
  mapping is active at once in Dolphin and RetroArch (verified by hand with an A/B swap).
- **Per-axis inversion** is impossible: stick values are `Vector2 { x: Option, y: Option }`,
  `invert` negates both, and a source can't select x or y events.
- **Exit.** SIGTERM, SIGKILL during a game (guard), and InputPlumber stopped (auto falls back to
  gilrs) all verified. The guard (`hearth-guard`) logs to stdout like Hearth.

## Found while handling things going away (2026-10-18)

- **Daemon restart.** Its bus name changes owner; signals keep matching when the match rule is
  by path (`path_namespace`), not by sender. `ManageAllDevices` is false again after a restart.
  There is no D-Bus activation file, so calls while it is down fail instead of starting it.
- **Controller off.** The composite device is stopped and the virtual pad disappears from under
  the game ("Error uploading effect … Failed to get effect ID" if a rumble was playing). On
  reconnect it is recreated at the same path, often with the input-less duplicates again.
- **Raw pad on reconnect.** The kernel's DualSense evdev nodes are readable for a moment
  before InputPlumber hides them: RetroArch (udev driver) opened them as player 1, and the
  shipped DualSense autoconfig's PS button opened its menu.
- **RetroArch player reservation** (`input_player1_device_reservation_type = 2`,
  `input_player1_reserved_device = "045e:0b00 …"`, RetroArch 1.22.2) didn't keep the raw pad
  out of port 1. Dropped.
- **SDL device allowlist** (`SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT=0x045e/0x0b00`) with
  RetroArch's `sdl2` joypad driver: across a reconnect with the game in front RetroArch only
  ever saw "Xbox One Elite 2 Controller (1118/2816)". RetroArch's sdl2 driver numbers buttons
  as SDL GameController (Misc1 15, paddles 16–19); fast-forward on a paddle and live remap
  verified by hand.
- **Device config override.** A file of the same name in `/etc/inputplumber/devices.d` wins
  over the shipped one (`data/inputplumber/devices.d/60-ps5_ds_gamepad.yaml`): the composite
  device then starts with the Elite pad, no virtual DualSense in between.
- **Battery.** No InputPlumber API. The kernel's `power_supply` under the HID device has it
  (UPower reads the same, as a gaming-input device). The DualSense reports 5, 15, 25… %.

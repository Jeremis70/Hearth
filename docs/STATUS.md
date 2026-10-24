# Hearth status

Last updated: 2026-10-24 (round D, profile-ready storage). Read this first, then `AGENTS.md` (rules, commands),
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
- Round B, audio (2026-10-23): UI sounds from packs, the ambient loop, per-game themes,
  volumes in Settings. Design: `docs/design/README.md` §7 "Sound".
- Round C.1, per-game customization (2026-10-23): the Customize page (artwork from
  SteamGridDB candidates or a file, hero position, Wrong game?, theme music with Start at),
  picks in `<data>/games/<art key>/`, themes that loop by themselves. Design: README §4
  "Customize", §6 "Picks", §7 "Game themes loop by themselves".

- Round C.2, animated art + polish (2026-10-24): animated SteamGridDB heroes and logos on
  Home and the Customize page, Animated artwork and Reduce motion, Kenney controller icons,
  the controller-disconnected card. Design: README §6 "Animated art", §4 "Customize",
  "In-game overlay".

- Round D, profile-ready storage (2026-10-24): per-person data under
  `<home>/profiles/<id>/`, today's data moved into `profiles/default/` by a backed-up,
  resumable migration. No profile UI. Design: README §9 "Profiles and storage".

Round D (profile-ready storage), 2026-10-24, oldest first:
- `f725497`: `hearth-core::users` (`ProfileId`, `ProfileDirs`, `profile.toml`); `DataDirs`
  gains `profiles/`, `emulators/` (shared), `backup/`; the old emulator roots are
  `legacy_retroarch()`/`legacy_dolphin()`.
- `eb54509`: library v4: `profile_games` (playtime, last played, favorite) and `play_sessions`
  per profile; v3 history goes to `default`; moved games take every profile's rows.
- `648292b`: `controls::UserControls` (one profile's files): resolver and editor keyed by
  (profile, system, game); `device.toml` stays in `config/controls/`.
- `0422e5e`: RetroArch saves in `saves/<system>/` (`sort_savefiles_enable = false`), shared
  `emulators/retroarch/system`, absolute paths (the relative-path known issue is gone);
  Dolphin `-u` per profile plus `-C` GCI folder A/B, raw card A/B and NAND root in the
  profile's `saves/` (checked in Dolphin's `MainSettings.cpp`, `UICommon.cpp`, `FileUtil.cpp`).
- `8772091`, `0e99f65`: `users::migrate` (plan, space and filesystem checks, verified backup
  with `SHA256SUMS`, marker-tracked renames, resume, rollback, stale path scan),
  `examples/migration_plan` (read-only).
- `20a4738`: `--profile`, the migration at startup (after the lock), the notice and no launches
  when it fails, `profile_state`; `HEARTH_TEST_MIGRATION_ABORT_AFTER` (throwaway homes only).
- `ee52ef6`: `test-home.sh --profile` (`HEARTH_TEST_PROFILE`), `make-migration-home.sh`.
- `f237419`: `Library::open` refuses a pre-profile library holding games
  (`NeedsProfileUpgrade`); only the migration upgrades it (found live: after a failed
  migration the session had upgraded the db without a backup).
- The user's TP save is a GCI folder (`GC/USA/Card A/01-GZ2E-gczelda2.gci`); raw cards are
  handled too (unit test only).
- Verified live (debug, nested gamescope 1920×1080, `--no-pad-input`, `--no-audio`, dev
  socket, copies made by `make-migration-home.sh`):
  - fake saves: killed (SIGABRT) after move 4 of 8, the next start resumed (`migration:
    resumed`), `sha256sum -c` OK (28 files), saves identical to the backup, history in
    `profile_games` for `default`; a second start: `nothing to do`, 42 hashes and the history
    rows unchanged;
  - real saves: Dolphin opened `profiles/default/saves/gc/Card A/USA/01-GZ2E-gczelda2.gci`
    and wrote the NAND's `fst.bin` there (inotify); RetroArch logged "Redirecting save file to
    …/profiles/default/saves/n64/…srm" and saved there; TP reached its health screen, OoT its
    title; no old folder came back; both saves byte-identical to the backup afterwards;
  - `--profile test2`: shipped controls only (`--print-controls gc/GZ2E01`), a GameCube option
    changed in Settings went to `profiles/test2/controls/systems/gc.toml`, OoT got a new srm
    in `profiles/test2/saves/n64/`, a closed session recorded 25 s for `test2` only;
    `default`'s files (30 hashes) and rows unchanged;
  - failure (an existing `profiles/default/emulators/retroarch`): refused before any backup,
    notice on screen, launch refused with a toast, old files unchanged, db still version 3.
- Found, not fixed (pre-existing): quitting Hearth while a game runs closes the game without
  recording that session's playtime (`Controller::shutdown`).
- Not done yet: the user's real `.dev-data` is still in the old layout, waiting for the OK
  (`migration_plan`: 8 moves, backup 28 files / 1,147,806 bytes, BIOS 2.0 MiB and Dolphin cache
  7.2 MiB moved unchanged). Starting a Hearth built from this round on `.dev-data` runs the
  migration.

Round C.2 (animated art + polish), 2026-10-24, oldest first:
- `b9336cb`: `.dev-data/games` links the moved ROMs and their `theme.mp3` (see below).
- `990a911`, `2807f4e`, `cd08468`: `hearth-core::art::animation` (content probe, frames,
  `.anim` v2 files of QOI frames, stills, `FrameStream`, disk trim by last played); WebP
  frames disposed to background are cleared (image-webp only does it with a background
  color set: OoT's logo glow built up and dropped at each loop); display copies named after
  their source; an animated source is decoded once.
- `9e6549a`, `2746dc0`: animated requests (`types=animated`, `image/png,image/webp`: the API
  refuses GIF there), `hero-animated.<ext>` / `logo-animated.<ext>`, automatic up to 30 MiB;
  Customize lists them (previews up to 10 MB).
- `af73832`: `[display] animated_art, reduce_motion`; Reduce motion sets `Theme.motion-scale`.
- `490c869`, `b2e98ac`, `9039776`: `bridge/anim.rs` (tracks, sheets, windows, streams) and
  `bridge/session/anim.rs` (slots: Home hero/logo, Customize strip/background); while
  animation may play, the still shown is the animated asset's own.
- `6a19537`: the row reorders only once a launched game is on screen; focus changes while a
  game is in front are ignored (a launched game's theme no longer lost to the game that
  took its cell).
- `30c89a2`, `34ce455`, `c761a17`: `--no-pad-input`, `HEARTH_TEST_SGDB=real`, dev words
  (`anim:`, `reduce_motion:`, `anim_state`, `pad:connect:`/`pad:disconnect`), animated
  fixtures, `anim_probe`; Kenney controller icons (`assets/icons/controllers/`).
- `15a994c`: the controller-disconnected card and the 2 s auto-resume.
- Measured (release, nested gamescope 1920×1080, on AC, performance profile; % of one
  core; GPU busy = 1 − RC6 residency, includes the desktop): idle Welcome 0.7 % / 19 %;
  OoT animated logo (74 frames 260×260, sheet 20 MB) + hero (48 frames 1920×620, window
  38 MB) 6.4 % / 28 %, RSS 298 MB; TP's 27 MB hero (469 frames, `.anim` 311 MB, window)
  19.1 % / 29 %, RSS 337 MB (it was 50.8 % streamed from the WebP before the per-file cap
  went). The cost is mostly Slint redrawing the whole window at each flip (draw 0.7–1.1 ms,
  ~38/s); sheets removed the uploads but barely the CPU. Focus moves: input→screen 6–28 ms,
  longest draw 14.5 ms (a sheet's one-time upload when an animation starts), none over
  16.7 ms. `.anim` sizes: OoT logo 8.8 MB, OoT hero 10.9 MB, TP automatic hero 39 MB.
  TP has 3 animated heroes (0.6, 1.1, 27.2 MB) and no animated logo; OoT one of each.
- Verified live (release and debug, `HEARTH_TEST_SGDB=real`, `--no-pad-input`, dev socket,
  screenshots): OoT's logo and hero animate on Home, stop on a focus move, show their
  stills with Reduce motion or Animated artwork off, never play in game or in the overlay,
  play again over a frozen game; a cached replay starts in 5–80 ms. Customize: badges, the
  focused candidate plays in its tile, the background and position mode play the hero in
  use; the 27 MB hero picked (no preview, size shown) survived a restart; a GIF and an
  animated WebP named `.png` from Downloads play as logos. Kenney icons in the top bar,
  the widget and Controllers rows (fake DualSense and Xbox). Fake controller pulled in
  game: back after 1.2 s resumed by itself, after 4 s stayed frozen.
- Not verified: the card itself in the overlay window (gamescope screenshots capture the
  base layer only; seen in Hearth's own window); a real unplug; listening (all runs
  `--no-audio`; the theme after Guide long press checked by `audio_state`); 720p for the
  27 MB hero (the run was interrupted by a stray controller press, before `--no-pad-input`).
- Follow-ups: another controller taking over a lost one (multiplayer round); cheaper flips
  would need fewer redraws (Slint's GPU renderer has no partial redraw).

Round C.1 (per-game customization), 2026-10-23, oldest first:
- `3575472`: `hearth-core::custom` (`CustomStore`, `custom.toml`, copies of chosen files);
  `Picked` goes first in the extras chain; a source can block the rest (`Lookup::Blocked`).
- `cff707f`: `MetadataProvider::assets` (all images of a kind, with thumbs); library v3
  `art_candidates` (the matcher's games); `--mock-sgdb=fixtures` (only with a HEARTH_HOME
  holding `.throwaway`); `scripts/test-home.sh`.
- `716d9df`: the art worker serves the page (candidates, apply, pick file, reset, re-match,
  position, theme start/remove; all writes there); its own backoff after network errors.
- `11a82ba`, `4a27666`: `hearth-audio::analyze` (first sound at −50 dBFS, loop region by a
  loudness envelope, fades walked out), `LoopSpec::intro_ms`, late loop points re-take the
  head; preview slot in the director; theme keys carry the Start at.
- `43e29b0`: `hearth-core::files` (read-only list, newest first), `HEARTH_DOWNLOADS`,
  `examples/make_fixtures`. Themes may be `.wav`, art `.jpeg`.
- `4bfffc2`: Home's hero cropped at `hero-x`; `hero-rev` so a same-size hero still swaps.
- `0ed926c`, `087f411`: the Customize page (`ui/customize/`, `components/file_list.slint`,
  `bridge/session/customize.rs`), Options items and Properties rows.
- `803b70a`: dev socket words (`customize:`, `pick:`, `rematch:`, `pick_file:`, `position:`,
  `reset:`, `theme_file:`, `theme_start:`, `theme_remove`, `custom_state`).
- `9fecb2e`: `art_probe --candidates`.
- Verified live (debug, nested gamescope 1920×1080 and 1280×720, `.dev-data/test-home` with
  `--mock-sgdb=fixtures` or `--mock-network`, `--no-audio`, dev socket + screenshots):
  - TP: hero #3 applied (check mark moves, status "Picked", backdrop and Home change at once),
    position 0.2 (bare hero over the page, then Home cropped the same), logo from
    `.dev-data/downloads/Logo transparent.png` (copied in; the original stays). All three
    were there after a restart.
  - Dummy Bowling (no match): "Wrong game?" listed 4 fixture games with thumbs; re-matched to
    "Super Dummy Bowling": its art arrived and shows on Home. Cover and hero picked, then
    `reset:hero` put only the hero back to automatic.
  - OoT: Theme section previews the folder `theme.ogg` (`audio_state` current
    `preview:…`); the file list (durations shown) previews the focused file after 500 ms;
    picking `Short jingle.wav` replaced the theme; Remove fell back to the folder theme; a
    second Remove wrote `source = "none"` (folder file untouched). With "Theme leading
    silence.wav" picked and Start at 5: Home plays `theme:…/theme.wav@5000`; its cached
    analysis says onset 3000 ms.
  - Offline (`--mock-network`): hero/cover/logo show saved candidates with "Offline · saved
    results"; icon (never listed) says "Can't reach SteamGridDB (trying again in 5 min). Use
    a file meanwhile."
  - Seams (`loop_seam … auto`, release): "Theme with fades.wav" (3 s fade-out) holds
    −16 dB through the seam (−13 at most in the crossfade; before the fade walk it dipped to
    −19); unit tests check onset within 20 ms and no window > 3 dB under the body, also when
    loop points arrive after the loop start.
  - Live SteamGridDB (user's key, read-only `art_probe --candidates`): TP's "Wrong game?"
    list (10 games, match first) and 24 heroes, all 1920×620 or 3840×1240 with thumbs.
- Not verified: listening (all runs were `--no-audio`; previews checked by `audio_state`
  only), the controller (dev socket only), the real SteamGridDB inside Hearth (only through
  `art_probe`), the Game themes-off warning line (code path only), a long title without a
  logo in the header after the fix.
- `.dev-data/games` (2026-10-24): the OoT and TP folders hold links to the ROMs and the real
  `theme.mp3` in `~/Games/<system>/<title>/`, next to the 15 dummies; `theme_probe` finds both
  games and both folder themes. The 8 s test theme is now `.dev-data/downloads/OoT test
  theme.ogg`. The test runs use `.dev-data/test-games` (links to the new places, empty
  dummies, a copy of the test `theme.ogg`).

Round B (audio), 2026-10-23, oldest first:
- `3efe5e6`: `pack.toml` `[sounds]` (logical name → file inside the pack) and
  `[loops.<name>]` (start, end, crossfade_ms); `[audio]` in `hearth.toml`; reserved names
  `achievement`, `mastery`, `ambient_profiles`, `startup`.
- `738f560`: `crates/hearth-audio`: cpal 0.18 (ALSA `default` = PipeWire), symphonia 0.6
  (MPL-2.0; ogg/vorbis, wav, flac, mp3), rubato 5 (FFT, to 48 kHz), rtrb queues into the
  callback. UI sounds decoded once; music streamed by one thread per track and looped by the
  looper in `stream.rs` (head kept, tail held back, equal-power crossfade, reopen + skip).
- `ba6bf7a`: bundled CC0 sounds from `scripts/make-default-sounds.py` (numpy + ffmpeg only to
  regenerate); the ambient is 32 s and loops with no loop points (checked by a test).
- `30675f3`: every `handle()` returns `NavResult`; `Feedback.input` → `bridge/sounds.rs`.
- `45cc7e6`: `hearth_audio::director` (pure rules) + `Controller::sync_music`, called from
  `set_mode` (after freeze/thaw).
- `cd691de`: `hearth-core::themes` (`ThemeLookup`, `LocalThemeFile`); 500 ms settle timer.
- `bc207b4`: Display & sound rows (volumes, Mute all, Music in overlay, Game themes).
- `222e084`: dev socket `sound:<name>` and `audio_state` (JSON reply).
- `010a519`: `examples/loop_seam.rs`. `569324c`: reopen test. `eb917e4`: music waits for 50 ms
  of decoded audio before it plays; a fade still running ends in 20 ms when a launched game
  takes the screen (RetroArch's window came 254 ms after launch, inside the 300 ms fade).
- The user's `ps5-sounds/pack.toml` maps `ambient_loop` = `themes/003. Home Menu.mp3`
  (start 1.2, end 884.0, crossfade 3000 ms: it fades in over ~1.1 s and out over ~3.3 s),
  `ambient_profiles` = `002. Select User.mp3`, `startup` = `001. First Time Startup.mp3`.
  Suggested for `themes/unused/` (the user moves it): `004. 3D Spatial Audio Test.mp3`.
- Verified live (release, nested gamescope 1920×1080, dev socket; laptop speakers):
  - real stream: `pactl list sink-inputs` shows `alsa_playback.hearth-shell`, float32 2 ch
    48 kHz, not corked; cpal buffer 256 frames, output delay 16.4 ms;
  - input → first mixed sample → output (`HEARTH_AUDIO_STATS=1`, 57 sounds): median 20.7 ms,
    p95 43.7 ms, max 52 ms (the worst when the same input opens a screen). Over the user's
    Bluetooth headset (AAC) cpal reports ~360 ms output delay: that's the headset;
  - CPU on idle Home with the mp3 ambient: 1.3 % of one core in total (PipeWire's ALSA thread,
    cpal's callback and the decode thread ~0.4 % each);
  - move/edge_bump/accept/back from the pack; held moves throttled; switching to the pack
    re-decoded the sounds (33 ms) and crossfaded the ambient to the pack's;
  - ambient fades out on launch; the overlay over frozen OoT plays OoT's theme (its own
    `-theme.ogg`), else the ambient; Resume SIGCONTs at once with a 150 ms fade; with N64's
    suspend set to none (temporarily), the overlay stayed silent;
  - test theme `.dev-data/games/n64/…Ocarina of Time…-theme.ogg` (an 8 s arpeggio, made
    here): crossfades in 500 ms after focusing OoT, back to the same ambient stream on TP;
    sweeping past OoT starts nothing (needs `--games-dir ./.dev-data/games`);
  - volumes, Mute all and both toggles apply live and survived two restarts;
  - per-file fallback: with ps5-sounds, `achievement`/`mastery` come from Hearth's defaults.
  - the seam of the pack's ambient (`loop_seam`): 100 ms levels stay −12…−18 dB through it,
    largest sample step 0.031 (normal content 0.022 at p99.9); without loop points it dips to
    −59 dB.
- After the user listened: UI sounds were barely heard over the music. Music now sits 6 dB
  under the UI sounds, Music defaults to 50, and packs can correct their mastering with
  `[gain] ui_db / music_db`; ps5-sounds uses +10 / −4 (its UI files peak at −27…−15 dBFS,
  Home Menu is −15 dBFS RMS). The user moved `004` out of `themes/` and replaced edge_bump.
- Extras by fixed names in a game's own folder (replacing `<rom>-theme.*`, `<rom>.png`,
  `<rom>-hero.png`, `<rom>-logo.png`): `hearth-core::extras` (`Extras` chain, `GameFolder`;
  Round C's stored choice goes first in `Extras::standard`), `Game::folder` set by the scan
  (a folder whose subtree holds one game), library `user_version` 2 (`games.folder`), and a
  sync that moves a vanished game's row to a new id with the same system and file name.
  - Why the themes didn't play: TP's ROM is `….nkit.iso`, so the old rule wanted
    `….nkit-theme.mp3`; OoT's file was `…[!].mp3` without `-theme`.
  - Verified on the dev library (backed up and restored afterwards): moving OoT and TP into
    `.dev-data/games/<system>/<title>/` kept playtime (3738 s, 2725 s), last played, a
    favorite, art keys and SteamGridDB ids (21202, 34645); no art deleted; Home showed the
    cached art with `--mock-network`; OoT's `theme.ogg` played from its folder.
  - Only the test theme in `.dev-data/games` and the user's TP `-theme.mp3` used the old
    names; no local art files existed.
- The overlay plays the ambient, never the running game's theme (user's call: it's brief,
  and a theme would restart from the top at every Guide press).
- Not verified (the user's to do): hearing it (nothing here was listened to); real
  controller input (dev socket only, so the pad's own path isn't in the latency numbers);
  turning the TV/HDMI off and on or switching the output device (not allowed here: only
  simulated in `a_failed_or_hung_output_is_reopened_and_the_music_goes_on`).

Round A.3 follow-up (controls page), 2026-10-20:
- Controls diagram on one canvas: drawing, highlights, lines, dots and callouts all in page
  units of the 1920×1080 layout, scaled by one factor `u` and centered (`Diagram` in
  `ui/controls/screen.slint`). Lines had drifted beside the rail because they used
  `Theme.scale` while boxes used the page's own scale. Slint's `Path` fits its viewbox into
  `size − stroke`, so the paths are grown and shifted by the stroke width.
- Lag cause: the 18 highlight SVGs (each the size of the whole drawing) were re-rasterized by
  Slint whenever one became visible. Same at the commit before A.3 (not a regression).
  `src/diagram.rs` now rasterizes the drawing and cropped highlights once per scale with resvg
  on a worker (6 s after start); callout placement is memoized (`callouts::place_generic`).
  Measured with `HEARTH_FRAME_STATS=1` (per-input lines at `hearth_shell=debug`):

  | | Before (release) | After (release) | Before (debug) | After (debug) |
  |---|---|---|---|---|
  | focus move, input→screen | ~50 ms (35 ms frames) | 17–20 ms (1 frame) | ~360 ms | ~24 ms |
  | open a controls page | ~70 ms | ~45–63 ms | ~460 ms | ~80 ms |

  Debug also gets opt-level 3 for resvg and the text/render crates (`Cargo.toml`).
- Button test: L2/R2 highlights follow the pressure (0–100 %, no animation, no step).
- The rail collapse animates on the way in (it was eaten by the slow first frame).
- Dialogs: switch "Close <running>?" / "<new> will start. Unsaved progress will be lost." /
  [Close and play] [Cancel]; Close game: "Unsaved progress will be lost."; fixed 720 width.
- Not checked live: trigger pressure (needs real presses), stick dots while moving.

Round A.3 (Home fixes, game switching, faster art, Settings), 2026-10-20, newest first:
- `64fd194`: design README: two-pane Settings, switch dialog, display art, notices.
- `9b7dcdf`: controllers show "Checking…" for the first 5 s (InputPlumber takes the pad ~3.5 s
  after launch; before, it read "None connected").
- `327bf8b`: Settings opens on two panes (no top-level page). Category list with status as a
  second line, right pane fades between categories, wide pages (console controls, button test)
  beside an icon rail. Controllers: real pads (name, USB/Bluetooth, battery), a controller page
  with Button test, console rows under "Controls". Welcome widget and top bar battery are real.
- `e408e18`: switch dialog ("Close <running>?" → Close and play / Cancel, Cancel focused).
- `ca44cb3`: `hearth.toml` parse errors: notice with the line, defaults kept, values never
  printed (parser messages are redacted).
- `0bd7f02`: art decode pool (3 threads, focused cell first), Home reports focus.
- `8e7d5d3`: display-sized QOI copies of art per screen class (`art/display.rs`).
- `473eb4c`: image decoders at opt-level 3 in debug builds.
- `f12408b`: `HEARTH_GS_RELEASE=1` for `run-gamescope.sh`.
- `9ab13e0`: startup timing log (`startup: … on screen ms=…`).
- `70d9cf6`: cold start on Welcome (label "Welcome"); Guide short with nothing running → Welcome.

Startup timing (window shown → on screen, 2 real games with cached art, 3 runs each):

| Build | Before: first game's hero | Before: row art | After: hero | After: row art |
|---|---|---|---|---|
| debug | 4.8–5.5 s | 7.8–8.7 s | 0.37–0.39 s | 0–0.38 s |
| release | 61–73 ms | 246–269 ms | in the first frame | in the first frame |

Most of the debug gain is the image decoders' opt-level; the rest of the debug time is the
first frame that draws the hero (~340 ms, unoptimised Slint/femtovg; making femtovg or
i-slint-core opt-level 3 didn't change it). Only 2 games here: the pool and the small copies
matter more with a bigger library.

Round A.2 (PS5-style Home), 2026-10-20, newest first:
- `cc107e3`: design README, Home section rewritten.
- `d5e90a4`: Home layout: second-slot anchor, label beside every cell, full-bleed hero,
  640×260 logo box, playtime pill, Welcome with greeting and widgets, Down state.
- `3b4587b`: row = Welcome, games, Game Library (Media and Settings only in the top bar);
  greeting from the clock; focus on the first game after the first library load.
- `a462eb6`: Hearth glyph (`assets/icons/hearth.svg`) and Lucide's clock.
- `b7b5f8b`: heroes fetched at 3840×1240 when available, else the largest.

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
   - Settings from Welcome: `up right right right accept` (focus lands on the last category
     visited, the first one at startup). Controllers: `down down down`, then `right`.
   - In that pane: `down` ×0/1/2 for the DualSense / N64 / GameCube and `accept`; on the
     controller page `accept` is Button test (`home_long` ends it).
   - Home starts on Welcome: `right` is the first game.
   - The Sticks & options tab is `next_tab`.
   - Launching TP: `accept` on Home, about 25 s to boot.
5. Screenshots: `GAMESCOPE_WAYLAND_DISPLAY=gamescope-0 gamescopectl screenshot $PWD/<file>.png`.
   They capture the base layer only (never the overlay window). Check overlay state with
   `DISPLAY=:1 cargo run -p hearth-platform --example gamescope_probe -- windows|props`.

**Next / open:**
- Real art verified 2026-10-20: OoT and TP matched (SteamGridDB 21202 and 34645), grid,
  hero 3840×1240, logo and icon cached; a `--mock-network` restart showed them from the cache
  with no provider call. Wi-Fi itself was never turned off (not allowed here).
- Day-to-day run: `HEARTH_GS_RELEASE=1 HEARTH_GS_FULLSCREEN=1 scripts/run-gamescope.sh`.
- Frame and input timings: `HEARTH_FRAME_STATS=1` (once a second at info).
- Not tried with the controller yet (round A.3, dev socket only): two-pane Settings, the
  controller page, the switch dialog, unplugging a pad while its page is open.
- Running at another size changes the display class (`.dev-data/data/display-class`) and remakes
  the art copies once (seen: 720p run, then 1080p again).
- Not tried with the controller yet: the Down state, Options menu and Favorite (dev socket only).
- `hearth.toml` must quote the key (`steamgriddb_key = "…"`); an unquoted value makes the
  whole file invalid: Hearth runs on defaults and shows "Couldn't read hearth.toml (line N)".
- Round C.1 follow-ups: typed search for "Wrong game?" and renaming (on-screen keyboard);
  the Downloads inbox; the art icon isn't shown anywhere but the Customize page; Delete game
  should remove `<data>/games/<key>/`; "Leftover customizations" in Storage.
- Art round follow-ups:
  - "Refresh artwork";
  - a NetworkManager connectivity signal instead of the 60 s queue check;
  - the seam between the sharp and the blurred hero is softened by a gradient, not blended;
  - UI sounds: done in round B.
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
  - the reconnect overlay and the notice card: the card is done (round C.2, Kenney icons).
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
  the profile's `emulators/retroarch/`; saves in `saves/<system>/`, BIOS in the shared
  `emulators/retroarch/system/`.
- Dolphin (`profiles/dolphin.rs`, gc and wii) is the native `dolphin-emu`, else the Flatpak,
  run with `-u <profile>/emulators/dolphin` and `-C` overrides (reasons listed in the file;
  memory cards and NAND in the profile's `saves/`).
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
  3. `<profile>/controls/systems/<system>.toml`;
  4. `<profile>/controls/games/<system>/<serial>.toml`, or the region-free serial. The serial
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
- Settings → Controllers lists connected pads (each opens its page with Button test) and one row
  per console; console pages and the button test open inside Settings beside the icon rail
  (`ControlsScreen` with `embedded`, scaled to its width).
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

**Audio** (round B, `crates/hearth-audio`, `bridge/sounds.rs`, `Controller::sync_music`)
- `Audio` is a handle; requests go over a channel to the `hearth-audio` thread (device,
  decoding UI sounds, music streams). The mixer runs in cpal's callback behind a mutex only it
  takes while a stream is open; it never allocates or frees (garbage goes back by queue).
- Device errors (other than xruns) or no callback for 3 s: the stream is dropped and the
  default device reopened (0.5 s doubling to 5 s); a rate change rebuilds the mixer.
- `--no-audio` / `HEARTH_AUDIO=null`: the null output runs the mixer in real time.
- Sounds per action: `NavResult` from every `handle()` (see the design README §7).
- Music: `director::choose` (pure, tested); fades 300 ms in / 300 ms launch / 150 ms resume /
  800 ms crossfade; the ambient is parked and resumed where it was, themes restart.
- Themes and a game's own art: `hearth-core::extras` (game folders, fixed names; see the
  design README §6).

**Customize** (round C.1): `hearth-core::custom` (picks), `art/service/customize.rs` (the
worker's side), `hearth-core::files`, `hearth-audio::analyze`, `ui/customize/`,
`bridge/session/customize.rs`. README §4 "Customize".

**Art, Home and packs** (art round)
- `hearth-core::art`: `identity.rs` (art key `<system>/<serial>` or `<system>/name-<fnv64 of the
  cleaned title>`, search names, match selection), `provider.rs` (`MetadataProvider`,
  `MockMetadataProvider`), `steamgriddb.rs`, `cache.rs` (paths, local overrides, blurred hero),
  `service.rs` (the `hearth-art` worker, `remove_orphaned_art`).
- SQLite (`library.rs`, `user_version` 1): `games.favorite`, `games.art_key`, `art` (match or
  `no_match`, kinds found), `art_queue` (network/HTTP errors only, backoff 5 min doubling to 6 h;
  everything queued is retried at startup).
- Art is deleted only for games removed by a scan whose system folder still exists.
- Shell: `bridge/art.rs` `ArtLoader` makes/decodes display copies on 3 threads into
  `SharedPixelBuffer`s, most visible first; tiles are updated in place (`models::set_tiles`).
  Console icons: `ConsoleIcons` over `packs::Resolver`. `bridge/startup.rs` logs startup timing.
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
  `HEARTH_GS_FULLSCREEN=1`, `HEARTH_GS_ARGS`, `HEARTH_GS_RELEASE=1`).
- **Dev socket actions** (add `--no-pad-input` so a real controller never drives a test
  instance): up, down, left, right, accept, back, prev_tab, next_tab, options,
  home_short, home_long, capture. Commands: `sound:<name>`, `audio_state`, and the Customize
  page's words (see `scripts/send-action.sh`; `custom_state` gives JSON). Replies are printed.
- **Throwaway test home:** `scripts/test-home.sh [--reset] [gamescope|windowed] [args]`
  (`.dev-data/test-home`, `--mock-sgdb=fixtures`, `HEARTH_DOWNLOADS=.dev-data/downloads`,
  `--games-dir .dev-data/test-games`). Its dev socket: `HEARTH_DEV_SOCKET=./.dev-data/test-home/actions.sock`.
- **Audio:** `HEARTH_AUDIO_STATS=1` (latency per UI sound), `--no-audio`. Check the stream:
  `pactl list sink-inputs`. `cargo run --release -p hearth-audio --example audio_probe` and
  `--example loop_seam` (see `AGENTS.md`, design README §7).
- **Keyboard:**
  - arrows; Enter/Space = Accept; Esc/Backspace = Back;
  - Q/E = tabs; O = Options;
  - F1/F2 = Guide short/long.
- **Logs:**
  - Hearth: stderr (`RUST_LOG=info,hearth_shell=debug`);
  - RetroArch: `.dev-data/profiles/default/emulators/retroarch/logs/`;
  - Dolphin: `.dev-data/profiles/default/emulators/dolphin/Logs/hearth-launch.log`;
  - library: `.dev-data/data/library.sqlite3`.
- **FPS counter:** `HEARTH_RETROARCH_ARGS="--appendconfig=data/dev/retroarch-fps.cfg"`.
- **Checks:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` (audio 16, core 198, platform 35, shell 28 tests).
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

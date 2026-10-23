# Hearth design system

Source specs (analysis of PS5 reference screenshots, kept for layout ideas only):

| Spec | Summary | Becomes in Hearth |
|---|---|---|
| `home.md` | Top bar, horizontal app row with enlarged focused tile, status pills, widget grid | **Home** |
| `overlay.md` | Dimmed full-screen overlay: card carousel + bottom dock of round shortcuts | **In-game overlay** (Home short press) |
| `search.md` | Search bar above horizontal rows of square and wide cards, L1/R1 tabs | **Media tab** layout + **global Search** |
| `settings.md` | Centered single-column list of categories with icon, label, focus frame | **Settings** category list (left pane) |

**This file wins over the individual specs when they disagree.** The specs describe Sony's screens; this file describes Hearth.

---

## 1. Principles

1. **10-foot UI first.** Everything is read from a couch 2–3 m away. Text and focus must be readable at that distance, which is why sizes below are larger than the specs measured.
2. **Focus is always obvious.** Exactly one focused element per screen, shown with the same focus treatment everywhere.
3. **Games are equal.** No screen treats a console differently; consoles appear only as small badges.
4. **Inspired, not copied.** No Sony logos, button symbols, fonts or proprietary icons. Hearth has its own warm palette.

## 2. Corrections to the specs

| Spec says | Hearth does | Why |
|---|---|---|
| Body text 13–16 px | Minimum **18 px**, body 20–24 px | Specs measured a 1080p screenshot of UI built for 4K TVs; small text is unreadable from the couch |
| Backdrop blur behind cards (home, overlay) | **Pre-blurred background image** + translucent surfaces | Slint has no backdrop-blur effect; blur the hero art once on a worker thread (`image` crate) and fade between blurred images |
| Overlay blurs the running game | Overlay **dims only** (`#000` at 65%) | The overlay is a separate gamescope overlay window and can't read the game's pixels |
| Three different suggested fonts | One font: **Manrope** (variable, SIL OFL), bundled in the repo | Consistency; Slint can import a `.ttf` directly |
| Tailwind-like blues as accent | Warm **ember** accent (see palette) | Identity; matches the name |
| Icons described generically | **Lucide** icon set (ISC license) as SVG | Free, consistent 2 px stroke style that matches the specs |
| PlayStation button glyphs in hints | **Kenney Input Prompts** (CC0), chosen per connected controller type (Xbox / PlayStation / Nintendo / generic) | Correct glyphs for any controller without shipping Sony artwork |
| Home widgets: trophies, store, news | Running now, Continue playing, Storage, Controllers, Recently added media | Hearth content; no store or ads |
| Settings and search specs place content without margins in places | **Title-safe margins**: 96 px left/right, 54 px top/bottom | TVs may crop edges (overscan) |

## 3. Theme tokens (`Theme` global)

All values at 1920×1080. Scale everything by `window-width / 1920` so 1280×720 and 4K work from the same numbers.

### Colors

| Token | Value | Use |
|---|---|---|
| `bg-base` | `#0C0B0A` | Screen background base |
| `bg-glow` | `#2A2019` | Soft top-left radial glow on plain screens |
| `surface` | `#1C1A18` @ 72% | Cards, widgets, dock |
| `surface-raised` | `#26231F` @ 88% | Focused cards, menus, keyboard |
| `divider` | `#FFFFFF` @ 6% | List separators |
| `scrim` | `#000000` @ 65% | Overlay dimming |
| `hero-vignette` | `#000000` @ 45% → 0% | Gradient over hero art (top and bottom) |
| `text-primary` | `#F7F3EE` | Main text |
| `text-secondary` | `#B4AAA0` | Metadata, hints |
| `text-disabled` | `#6E6660` | Unavailable items |
| `accent` | `#F2994A` | Ember: primary buttons, selected tab underline, progress |
| `accent-soft` | `#F2994A` @ 18% | Accent backgrounds (selected chip) |
| `success` | `#34D399` | Ready, connected, running |
| `warning` | `#FBBF24` | Missing BIOS, low battery |
| `danger` | `#EF6B5B` | Errors, Close game confirm |
| `focus-ring` | `#FFFFFF` | Focus border |
| `focus-glow` | `#F2994A` @ 35% | Outer glow around focus ring |

### Typography (Manrope)

| Token | Size / weight | Use |
|---|---|---|
| `display` | 48 / SemiBold | Screen titles ("Settings"), hero game title |
| `title` | 32 / SemiBold | Overlay game title, dialog titles |
| `tab` | 28 / SemiBold, +0.5 px tracking | Top tabs, clock |
| `heading` | 26 / SemiBold | Section headers, widget titles |
| `body-lg` | 24 / Medium | Settings rows, card titles, buttons |
| `body` | 20 / Regular | Descriptions, secondary lines |
| `caption` | 18 / Medium | Badges, metadata, button hints (minimum size anywhere) |

### Spacing, sizes, radii

| Token | Value |
|---|---|
| Spacing scale | 4, 8, 12, 16, 24, 32, 48, 64, 96 |
| `safe-x` / `safe-y` | 96 / 54 |
| `topbar-height` | 112 |
| `tile` / `tile-focused` | 128 / 192 (home row, square) |
| `tile-gap` | 20 |
| `card-square` | 300 × 300 (media rows) |
| `card-wide` | 520 × 220 |
| `grid-cover` | 220 × 300 (library grid, portrait) |
| `row-height` | 80 (settings rows), gap 8 |
| `button-height` | 64 |
| `dock-height` | 96, dock buttons 72 |
| `radius-sm` / `radius-md` / `radius-lg` / `radius-pill` | 8 / 14 / 22 / 999 |
| `focus-ring-width` | 3 |
| `focus-glow-radius` | 18 |

### Motion

| Token | Value | Use |
|---|---|---|
| `fast` | 120 ms, ease-out | Focus ring moving between list rows, button press |
| `base` | 200 ms, ease-out | Tile scale, card scale, scrolling to keep focus visible |
| `slow` | 400 ms, ease-in-out | Hero background crossfade, tab page slide |
| `overlay-in` | 250 ms, `cubic-bezier(0.16, 1, 0.3, 1)` | Overlay slides up, scrim fades in |
| `overlay-out` | 150 ms, ease-in | Overlay dismiss |

Respect a future "Reduce motion" setting: all durations multiply by a `Theme.motion-scale` property (0 disables animation).

### Focus treatment (everywhere)

- Focused tile or card: scale up (tiles grow to `tile-focused`, cards ×1.04), `focus-ring` border, `focus-glow` outside, title label appears below tiles.
- Focused list row or button: `surface-raised` fill, `focus-ring` border, `radius-md`.
- Unfocused tiles: 85% opacity.
- Focus moves with `base` timing; never jumps without animation unless motion is off.

## 4. Screens

### Home (from `home.md`, laid out like the PS5's game hover / game down screens)

```
┌ [Games] [Media]                                   search  settings  🎮   5:26 PM ┐
│ ▢⌂  [■ focused]  ▢ ▢ ▢ ▢ ▢ ▢ ▢ ▢ ▢ … ▢📚            hero art, full-bleed →     │
│        🎮 The Legend of Zelda: Ocarina of Time                                     │
│                                                                                   │
│ ┌──────────────┐                                                                  │
│ │ LOGO 640×260 │  (or TITLE in display, 2 lines max)                              │
│ └──────────────┘                                                                  │
│ N64 · last played today                                                           │
│ [ ▶ Play ] [ ⋯ ]  (running: Resume · Close · ⋯)                  ( 🕒 Played 12 h ) │
└────────────────────────────────────────────────────────────── Ⓐ Select  ☰ Options ┘
```

- **Row contents:** a Welcome cell first (Hearth glyph, label "Welcome"), then the games (running game first, then most recently played), then Game Library last. Media and Settings are in the top bar, not in the row. Running game gets a `success` dot.
- **Focus anchor (second slot):** the anchor is `safe-x + tile + tile-gap`; `row-offset = max(0, index − 1) × (tile + tile-gap)`.
  - A cold start focuses Welcome (at `safe-x`, under the "Games" tab). Coming back from a running game (Guide long press, Go to Home) or after it ends focuses that game, right next to Welcome. Guide short with nothing running goes Home with Welcome focused, from any screen.
  - Welcome focused (index 0) sits at `safe-x`; the row doesn't move.
  - From index 1 on, the focused cell stays at the anchor all the way to the last cell. The previous cell stays fully visible on its left, and older cells run off the screen edge (never clipped at `safe-x`). Empty space after the last cell is fine.
  - Left/Right slide the row by one cell (`base`).
- **Label beside the focused cell, for every kind of cell:** icon (24 px) then name (`body-lg`), left edge at the focused cell's right edge + `tile-gap`, bottom-aligned with it, in the band left free below the smaller cells. Icons: the console icon for games, the Hearth glyph for Welcome, the library icon for Game Library. Cells carry no console badge.
- **Console icons:** come from the selected console-icon pack (§7), else Hearth's original monochrome glyphs in Lucide's style (2 px stroke, tinted `text-primary`) in `assets/icons/consoles/`. Hearth's own glyphs never use console makers' logos or trademarks. When no icon exists, a short text monogram ("N64", "GC") in a `radius-sm` outline. Pack icons are drawn as they are (they may be in colour).
- Cells show the game's square grid art; until it arrives (or without any), a gradient with the title.
- **Game focused:**
  - The hero is full-bleed behind the whole screen (cover-fit, centered crop), with the top bar and row drawn over it. Heroes are fetched at 3840×1240 when SteamGridDB has one.
  - Readability: a left-to-right gradient (black at 88 % at the left edge, 65 % at 27 % of the width, gone by 60 %), a bottom vignette (`hero-vignette`) and a light top vignette for the tabs. Two layers crossfade (`slow`) when the focused cell's hero changes.
  - The pre-blurred hero stays cached but isn't drawn on Home.
  - Without a hero: `bg-base` with the `bg-glow` top-left glow.
  - **Logo:** fits inside 640×260, aspect kept, bottom-left aligned, its bottom at 62 % of the height. Without a logo: the title in `display`, 2 lines at most.
  - Under it: console · last played (`body`, `text-secondary`). Then Play + ⋯ (Resume + Close + ⋯ while running).
  - On the same line, right-aligned at the safe edge: the playtime pill (`button-height`, `surface`, clock icon + "Played 12 h").
  - No widgets under games.
- **Welcome focused:** `bg-base` + `bg-glow`, a greeting by time of day ("Good morning" 5–12 h, "Good afternoon" 12–18 h, "Good evening" otherwise) in `display` where the logo goes, and the status widgets (Running now, Storage, Controllers, Recently added media) below it. The widgets appear only on Welcome. Controllers is real: "1 connected" and "DualSense · 85 %" ("Checking…" for the first seconds while the input backend finds the pads).
- **Game Library focused:** title, description and an Open button.
- **Down (the PS5's game down):**
  - Down from the row (or Accept on Welcome) slides the page up (`slow`).
  - The tabs and the row fade and slide out. A compact header fades in at the top left (`safe-x`, `safe-y`): the cell's art (64 px, `radius-md`), then its icon and name.
  - The logo/greeting block, meta line, buttons and pill move up by 260. Focus goes to Play, or to the first widget on Welcome.
  - Up or Back restores the row.
- Navigation: Left/Right in the row; Up from the row → top bar; L1/R1 switch Games/Media; Options on a game cell (or ⋯) opens the game menu.
- **Game menu:** opens to the right of the focused cell, or under the buttons when opened from them. Items: Play or Close game (whichever applies), Controls for this game, Favorite (toggle with a check mark; the menu stays open), Properties. Back, Options or Left closes it. Uninstall comes later.
- **Starting a game while another runs** (Play on Home, Options → Play, later the Library): a `ConfirmDialog` (720 wide; titles wrap, two lines at most) "Close <running game>?", "<new game> will start. Unsaved progress will be lost.", [Close and play] (danger) and [Cancel] (focused). The plain Close game confirmation says only "Unsaved progress will be lost." Confirm closes the running game the usual way (a frozen game is continued first so it can quit), waits for its session to end, then launches the new one. Cancel or Back leaves everything as it was.
- **Problems Hearth works around** (e.g. `hearth.toml` unreadable): a one-line notice at the bottom center, warning icon + "Couldn't read hearth.toml (line 3) · Using default settings", for 20 s. It never quotes the file (it may hold the API key).
- **Properties:** a modal card with title, console, file path, size and the SteamGridDB match; "Change artwork" and "Wrong game?" are shown disabled ("Coming later"). Back closes it.

### Media tab and Search (from `search.md`)

- Media tab: vertical stack of horizontal rows: Continue watching (wide cards with progress bar), Recently added movies (portrait), Shows, Music. Edge-cut cards hint at more content.
- Search (from the top bar): search field at top, on-screen keyboard below it, results grouped into rows: Games, Movies, Shows. Typing filters live.
- Navigation: Up/Down between rows, Left/Right within a row, row remembers its horizontal position.

### Game Library (no reference screenshot)

Portrait cover grid (`grid-cover`), filter chips under the top bar (All · Favorites · one chip per console present), L1/R1 cycle chips, Options opens sort. Same focus treatment as cards.

### Settings (from `settings.md`)

```
Settings
┌ categories (520) ─────┐     ┌ right pane ──────────────────────────────────┐
│ ◉ Network             │     │ Connected controllers                         │
│   Wi-Fi · Home-5G     │     │ 🎮 DualSense          Bluetooth · 🔋 85 %  ›  │
│ ▣ Controllers   focus │     │ Controls                                      │
│   1 connected         │     │ ⚙ Nintendo 64                    Default  ›  │
│ 🔊 Display & sound    │     │ ⚙ Nintendo GameCube              Default  ›  │
└───────────────────────┘     └───────────────────────────────────────────────┘
```

- Settings opens straight into two panes; there is no separate top-level page. Left: the category list (icon, label, status as a second line: "Wi-Fi · Home-5G", "1 connected", "Hearth 0.1.0"), 520 px, rows `row-height` + 16. Right: the highlighted category's content, starting 48 px after the list.
- Focus starts in the list, on the category visited last while Hearth runs (else the first). The right pane follows the list as focus moves: the old content fades out and the new one fades in (`fast` in all), so two lists are never drawn over each other.
- Right or Accept moves focus into the right pane (the category keeps a `accent-soft` fill); Left at its edge or Back returns to the list; Back from the list leaves Settings.
- **Deeper pages open in the right pane.** A narrow one (a controller's page) replaces the category content. A wide one (a console's controls drawing, the button test) needs the width: the list collapses (`base`) into an icon rail (icons only, the current category marked, not focusable), the "Settings" title and hints fade, and the page fills the rest, scaled to it. Back returns to where it was opened from and the list expands again. There is only ever one category list (the rail is the same list, collapsed).
- Categories: Network, Game folders, Emulators, Controllers, Display & sound, Storage, System.
- **Controllers:**
  - "Connected controllers": one row per physical controller from the input backend: controller glyph, name ("DualSense", "Xbox controller"…), connection (USB/Bluetooth), battery (icon + %, charging icon while charging). Empty: "No controller connected". Rows appear and go as controllers connect and disconnect; battery levels refresh every few seconds.
  - A controller's row opens its page (right pane): glyph, name, connection and battery, then its actions: "Button test" now; player number and Identify later.
  - "Controls": one row per console with controls ("Default" / "Customized"), opening that console's controls page (wide). These are shared by everyone, not tied to one controller.
- Display & sound: "Console icons" and "UI sounds", each showing the selected pack and opening a list of "Hearth default" plus the installed packs with files for it (name, "by <author>"); a check marks the selection. Choosing saves it in `hearth.toml`; icons and sounds change at once. Below them: UI sounds volume and Music volume (Left/Right in steps of 10, a level bar and the number), Mute all, Music in overlay, Game themes (Accept toggles, "On" in accent).

### In-game overlay (from `overlay.md`)

```
                                                          5:26 PM  🎮 85%
 ┌───────────────┐ ┌──────────┐ ┌──────────┐ ┌──
 │ NOW PLAYING   │ │ Controls │ │ Display  │ │
 │ Twilight Pr.  │ │ remap    │ │ & sound  │ │
 │ GameCube·1h12 │ └──────────┘ └──────────┘ └──
 │ [Resume]      │
 │ [Home][Close] │
 └───────────────┘
        ( Home ) ( Library ) ( Media ) ( Volume ) ( Controllers ) ( Power )
```

- Scrim only, no blur. Cards slide up with `overlay-in`.
- First card is always **Now playing**: title, console badge, session time, buttons Resume (focused on open), Go to Home, Close game (asks for confirmation).
- Further cards (later milestones): controller remap, display/sound, save states (RetroArch first, via its network command interface; others when an emulator supports it).
- Dock: Home, Library, Media, Volume, Controllers, Power.
- Home press or Back: dismiss and resume the game immediately.

## 5. Shared components

`Tile`, `Card` (square / wide / portrait variants), `TopBar`, `TabStrip`, `Chip`, `Button` (primary / secondary / danger), `ListRow` (icon, label, value), `Widget`, `DockButton`, `SectionHeader`, `HintBar` (controller-aware glyphs), `OnScreenKeyboard`, `FolderPicker`, `ConfirmDialog`, `Toast`.

## 6. Game artwork

- Per game, once: identify (ROM header first: GameCube game id + internal name, N64 game code + internal name; else the file name cleaned of No-Intro/Redump tags), search SteamGridDB, keep the match (its id is stored permanently), then fetch the top-voted static, safe (`nsfw`, `humor`, `epilepsy` off) grid (square, else closest to square), hero, logo and icon.
- Cache: `<cache>/art/<art key>/` (`~/.cache/hearth/art/`, or `$HEARTH_HOME/cache/art/`), where the art key is `<system>/<serial>` or `<system>/name-<hash of the cleaned title>`. Never re-fetched unless files are missing (or a later "Refresh artwork").
- Display copies: Home never decodes the originals. Next to them, `sized/<class>/{hero,logo,grid}.qoi` are made for the screen's class (its height rounded up to 720/1080/1440/2160): hero as tall as the screen, logo inside 2× the 640×260 box, grid 2× `tile-focused`, all scaled with the class and never enlarged. Made on first use (already-cached art included), remade when the original is newer or the screen class changes (the class is remembered in `<data>/display-class`). Originals stay for "Change artwork" and other screens.
- Loading order: the focused cell's hero and logo, the grids in view, the neighbours' heroes and logos, then the rest, on up to 3 threads.
- Local files next to the ROM always win: `<rom stem>.png` (grid), `<rom stem>-hero.png`, `<rom stem>-logo.png`.
- No match is remembered and not retried; network or HTTP errors put the game in a retry queue (startup, then with backoff while running).
- API key: `HEARTH_STEAMGRIDDB_KEY`, else `[art] steamgriddb_key` in `hearth.toml`. Without one, only local and cached art is shown.

## 7. Asset packs

A pack replaces some of Hearth's console icons or UI sounds. It is a folder:

```
<packs>/<id>/                  # $HEARTH_HOME/packs/ in development, ~/.local/share/hearth/packs/ installed
  pack.toml                    # optional
  icons/consoles/<system>.svg  # or .png
  sounds/<name>.ogg            # or .wav, .flac, .mp3
```

`pack.toml` (every field optional; the name defaults to the folder name):

```toml
name = "My console icons"
author = "Someone"
license = "CC-BY-4.0"
notes = "Anything else"
```

Assets are looked up by **logical name**, in the pack selected for that category, then in Hearth's bundled defaults (`assets/`, same layout). Each file falls back on its own, so a partial pack works. A category is offered for a pack only when the pack has its folder.

| Category | Logical names | Files |
|---|---|---|
| Console icons | `consoles/<system id>`: `gc`, `n64`, `wii`, `wiiu`, `psx`, `ps2`, `ps3`, … (ES-DE system ids) | `icons/consoles/<system>.svg\|.png` |
| UI sounds | `sounds/move`, `sounds/accept`, `sounds/back`, `sounds/overlay_open`, `sounds/overlay_close`, `sounds/edge_bump`, `sounds/ambient_loop`; reserved (they resolve and the dev socket plays them, nothing else yet): `sounds/achievement`, `sounds/mastery`, `sounds/startup`, `sounds/ambient_profiles` | `sounds/<name>.ogg\|.wav\|.flac\|.mp3` |

A sound pack's `pack.toml` may point names at other files inside the pack (paths with `..` or absolute paths are ignored), and give loop points for music. Files in the default locations need no mapping:

```toml
[sounds]
ambient_loop = "themes/003. Home Menu.mp3"

[loops.ambient_loop]
start = 1.2           # seconds; the loop restarts here (the first play starts at 0)
end = 884.0           # seconds; default: the end of the file
crossfade_ms = 3000   # the end fades into the start (equal power)
```

Without loop points the whole file loops with no crossfade (gapless: made for files that loop by construction, like Hearth's own). `cargo run --release -p hearth-audio --example loop_seam -- <file> <start> <end> <ms> out.wav` renders the 10 s around the seam with levels, to check loop points.

Hearth's bundled sounds (`assets/sounds/`, CC0, generated by `scripts/make-default-sounds.py`) are placeholders. A missing sound is logged once and stays silent. The selection lives in `hearth.toml`:

```toml
[packs]
console_icons = "my-console-icons"   # missing: Hearth default
ui_sounds = "ps5-sounds"
```

### Sound

- **UI sounds** come from what an action did, never from focus changing by itself (cold start, returning from a game, list reloads stay silent). Every screen's `handle()` returns `moved`, `edge`, `handled`, `quiet` or `ignored`:
  - `moved` → `move`, at most one per 130 ms (a held D-pad repeats every 80 ms, so it ticks every other repeat); a new move cuts the last with a 6 ms fade;
  - `edge` (a move at the end of a row or list) → `edge_bump`, at most one per 250 ms;
  - `handled` Accept / Back → `accept` / `back` (controller actions fire on release);
  - `quiet`: used, sound comes from elsewhere (Back or Resume in the overlay play `overlay_close`).
- **Music:** the ambient loop on Hearth's screens, fading in over 300 ms. Silent whenever a game runs unfrozen: launching fades it out over 300 ms while the game starts; Resume unfreezes at once and fades it out over 150 ms. In the overlay over a frozen game it plays only with "Music in overlay" (continuing where it left off); over a game whose suspend policy is off it stays silent.
- **Game themes:** `<rom stem>-theme.ogg` (or `.mp3`, `.flac`) next to the ROM, like the local art. When a game with a theme stays focused on Home's Games row for 500 ms, the ambient crossfades to it (800 ms), and back once focus settles elsewhere or Home is left. The overlay plays the running game's theme instead of the ambient. Themes extracted from dumps will be a second source behind the file next to the ROM.
- **Volumes** (Settings → Display & sound): UI sounds and Music, 0–100 in steps of 10 on a dB scale (100 = 0 dB, each step −5 dB, 0 = silent); Mute all; Music in overlay; Game themes. Changes apply at once (the UI volume plays a move at its new level) and are saved under `[audio]` in `hearth.toml`. System volume (PipeWire, the overlay's Volume button) is separate and later.

## 8. Assets and licenses

- Font: Manrope, SIL Open Font License, in `assets/fonts/` with its license file.
- Icons: Lucide (ISC), in `assets/icons/` as SVG. Console glyphs in `assets/icons/consoles/` are Hearth's own.
- Button glyphs: Kenney Input Prompts (CC0), in `assets/glyphs/`.
- Sounds: Hearth's own, synthesized (CC0), in `assets/sounds/`.
- PS5 screenshots stay in `docs/reference/` (gitignored) and are never shipped.

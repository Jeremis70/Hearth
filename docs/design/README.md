# Hearth design system

Source specs (analysis of PS5 reference screenshots, kept for layout ideas only):

| Spec | Summary | Becomes in Hearth |
|---|---|---|
| `home.md` | Top bar, horizontal app row with enlarged focused tile, status pills, widget grid | **Home** |
| `overlay.md` | Dimmed full-screen overlay: card carousel + bottom dock of round shortcuts | **In-game overlay** (Home short press) |
| `search.md` | Search bar above horizontal rows of square and wide cards, L1/R1 tabs | **Media tab** layout + **global Search** |
| `settings.md` | Centered single-column list of categories with icon, label, focus frame | **Settings** top level |

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

### Home (from `home.md`)

```
┌ top bar: [Games] [Media]                      search  settings  🎮 85%  5:26 PM ┐
│ tile row: ▢ ▢ [■ focused] ▢ ▢ ▢ ▢ ▢ … Library  Media  Settings                 │
│                                                                                 │
│ hero zone (focused game):  LOGO (or TITLE in display)                           │
│                            GameCube · 12 h played · last played yesterday       │
│                            [ ▶ Play ]  [ ⋯ ]   (running: Resume · Close · ⋯)    │
│                                                                                 │
│ widget row: Running now · Storage · Controllers · Recently added media          │
└──────────────────────────────────────────────────────────── Ⓐ Select  ☰ Options ┘
```

- Background: the focused game's hero. The sharp hero spans the top of the screen at its own aspect ratio (behind the top bar, row and hero zone) and fades into the pre-blurred, downscaled hero stretched behind the widgets, darkened with `hero-vignette`. Two layers crossfade (`slow`) when the focused game or its art changes. Games without art show their placeholder gradient.
- Tiles show the game's square grid art; until it arrives (or without any), a gradient with the title.
- Hero zone: the game's logo (max 140 px tall, 640 px wide) instead of the title when one is cached; the metadata line is plain text ("N64 · 59 min played · last played today").
- Tile row: recently played across all consoles (most recent first), then fixed tiles Game Library, Media, Settings. Running game gets a `success` dot.
- **Fixed focus anchor (sliding window):** the focused tile always sits at the same x position (the first slot, at `safe-x`). Pressing Right/Left slides the whole row by one tile (`base` timing) instead of moving focus across the screen. Exception: near the end of the row, the row stops sliding when its last tile is fully visible, and focus moves the rest of the way. Never leave empty space to the right of the last tile.
- **Label tucked beside the focused tile:** because the focused tile (`tile-focused`, 192) is taller than the others (`tile`, 128), the 64 px band below the following tiles is free. The focused item's name (`body-lg`) and console icon (24 px) sit there: left edge at focused tile's right edge + `tile-gap`, vertically aligned with the focused tile's bottom. Tiles carry no console badge themselves.
- **Console icons:** come from the selected console-icon pack (§7), else Hearth's original monochrome glyphs in Lucide's style (2 px stroke, tinted `text-primary`) in `assets/icons/consoles/`. Hearth's own glyphs never use console makers' logos or trademarks; when no icon exists, a short text monogram ("N64", "GC") in a `radius-sm` outline. Pack icons are drawn as they are (they may be in colour).
- When the focused tile is too close to the right edge for its label (less than 320 px), the label goes under the tile, right-aligned to it.
- The hero zone is missing from `home.md` because the screenshot focused a system tile; it is the core of the PS5 feel and must be included.
- Navigation: Left/Right in the tile row; Down → Play/⋯ buttons → widget row; Up from row → top bar; L1/R1 switch Games/Media; Options on a tile (or ⋯) opens the game menu.
- **Game menu:** opens to the right of the focused tile (to its left at the end of the row): Play or Close game (whichever applies), Controls for this game, Favorite (toggle with a check mark; the menu stays open), Properties. Back, Options or Left closes it. Uninstall comes later.
- **Properties:** a modal card with title, console, file path, size and the SteamGridDB match; "Change artwork" and "Wrong game?" are shown disabled ("Coming later"). Back closes it.
- Focus starts on the most recent game, or on the running game if one exists.

### Media tab and Search (from `search.md`)

- Media tab: vertical stack of horizontal rows: Continue watching (wide cards with progress bar), Recently added movies (portrait), Shows, Music. Edge-cut cards hint at more content.
- Search (from the top bar): search field at top, on-screen keyboard below it, results grouped into rows: Games, Movies, Shows. Typing filters live.
- Navigation: Up/Down between rows, Left/Right within a row, row remembers its horizontal position.

### Game Library (no reference screenshot)

Portrait cover grid (`grid-cover`), filter chips under the top bar (All · Favorites · one chip per console present), L1/R1 cycle chips, Options opens sort. Same focus treatment as cards.

### Settings (from `settings.md`)

- Top level: centered list (1320 px wide), `row-height` rows with Lucide icon, label, and a **right-aligned status value** (Network: "Wi-Fi · Home-5G", Storage: "450 GB free", Controllers: "2 connected").
- Categories: Network, Game folders, Emulators, Controllers, Display & sound, Storage, System.
- Display & sound: "Console icons" and "UI sounds", each showing the selected pack and opening a list of "Hearth default" plus the installed packs with files for it (name, "by <author>"); a check marks the selection. Choosing saves it in `hearth.toml`; icons change at once.
- Entering a category slides in a two-pane page from the right (`slow`): sub-items list on the left, details/actions on the right. Back returns.

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
| UI sounds | `sounds/move`, `sounds/accept`, `sounds/back`, `sounds/overlay_open`, `sounds/overlay_close`, `sounds/edge_bump`, `sounds/ambient_loop` | `sounds/<name>.ogg\|.wav\|.flac\|.mp3` |

Hearth ships no sounds yet: a missing sound is logged once and stays silent. Playback arrives in a later round. The selection lives in `hearth.toml`:

```toml
[packs]
console_icons = "my-console-icons"   # missing: Hearth default
ui_sounds = "ps5-sounds"
```

## 8. Assets and licenses

- Font: Manrope, SIL Open Font License, in `assets/fonts/` with its license file.
- Icons: Lucide (ISC), in `assets/icons/` as SVG. Console glyphs in `assets/icons/consoles/` are Hearth's own.
- Button glyphs: Kenney Input Prompts (CC0), in `assets/glyphs/`.
- PS5 screenshots stay in `docs/reference/` (gitignored) and are never shipped.

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
│ hero zone (focused game):  TITLE (display)                                      │
│                            [GameCube] · 12 h played · last played yesterday     │
│                            [ ▶ Play ]  [ ⋯ ]                                    │
│                                                                                 │
│ widget row: Running now · Storage · Controllers · Recently added media          │
└──────────────────────────────────────────────────────────── Ⓐ Select  ☰ Options ┘
```

- Background: focused game's art, pre-blurred variant behind widgets, `hero-vignette` on top, crossfade `slow`.
- Tile row: recently played across all consoles (most recent first), then fixed tiles Game Library, Media, Settings. Console badge (caption) in the tile's bottom-left; running game gets a `success` dot.
- The hero zone is missing from `home.md` because the screenshot focused a system tile; it is the core of the PS5 feel and must be included.
- Navigation: Left/Right in the tile row; Down → Play/⋯ buttons → widget row; Up from row → top bar; L1/R1 switch Games/Media; Options on a tile opens the game menu (Close game, Favorite, Uninstall, Properties).
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

## 6. Assets and licenses

- Font: Manrope, SIL Open Font License, in `assets/fonts/` with its license file.
- Icons: Lucide (ISC), in `assets/icons/` as SVG.
- Button glyphs: Kenney Input Prompts (CC0), in `assets/glyphs/`.
- PS5 screenshots stay in `docs/reference/` (gitignored) and are never shipped.

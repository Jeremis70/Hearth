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

Reduce motion (Settings → Display & sound): all durations multiply by `Theme.motion-scale` (0 disables animation), and animated art shows its stills.

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
- **Game menu:** opens to the right of the focused cell, or under the buttons when opened from them. Items: Play or Close game (whichever applies), Controls for this game, Favorite (toggle with a check mark; the menu stays open), Artwork, Theme music, Wrong game? (each opens the Customize page on that section), Game settings (Dolphin games), Properties. Back, Options or Left closes it. Uninstall comes later.
- **Starting a game while another runs** (Play on Home, Options → Play, later the Library): a `ConfirmDialog` (720 wide; titles wrap, two lines at most) "Close <running game>?", "<new game> will start. Unsaved progress will be lost.", [Close and play] (danger) and [Cancel] (focused). The plain Close game confirmation says only "Unsaved progress will be lost." Confirm closes the running game the usual way (a frozen game is continued first so it can quit), waits for its session to end, then launches the new one. Cancel or Back leaves everything as it was.
- **Problems Hearth works around** (e.g. `hearth.toml` unreadable): a one-line notice at the bottom center, warning icon + "Couldn't read hearth.toml (line 3) · Using default settings", for 20 s. It never quotes the file (it may hold the API key).
- **Properties:** a modal card with title, console, file path, size and the SteamGridDB match, then focusable rows: Artwork, Theme music, Wrong game? (Up/Down, Accept opens the Customize page; Back from there returns here), and Game settings for Dolphin games. Back closes it.
- **Game settings** (Dolphin games): an opaque card over Home (1280 wide, the screen's height inside the safe area) titled "<game> · Game settings", listing the settings that may differ per game (§9 "Emulator settings"), first a Widescreen row (Off/On) for games Dolphin ships a widescreen code for (§9 "Widescreen"); while it's on, Aspect ratio and Widescreen hack show "Force 16:9" / "Off" with "Set by Widescreen" and can't be changed (no ‹ ›, edge sound). A row shows the game's value: its own (accent dot, "Custom for this game"), else what Dolphin's shipped game settings set ("Set by Dolphin for this game"), else the console's. Left/Right step it (Accept steps forward) and give the game its own value; Options resets that row; the last row, "Reset to console defaults", asks first and removes them all. The focused row's description and "Applies next launch" (or "Restart <game> to apply" while a Dolphin game runs) sit under the list. Back closes it.

### Customize (a game's artwork, match and theme)

```
[logo] Customize
┌ sections (480) ─────┐   ┌ Hero ─────────────────────────────── [preview of the one in use] ┐
│ ▣ Cover  Automatic  │   │ Now: picked from SteamGridDB                                       │
│ ▣ Hero   Picked     │   │ [thumb][thumb ✓][thumb][thumb][thu…      ← strip, Left/Right       │
│ ▣ Logo   Your file  │   │ ↔ Position                                            20 %         │
│ ▣ Icon   Automatic  │   │ 🗋 Use a file…                                                     │
│ ♪ Theme  Game folder│   │ ↺ Reset to automatic                                               │
│ ? Wrong game?  TP   │   └────────────────────────────────────────────────────────────────────┘
└─────────────────────┘                                              Ⓐ Use this  Ⓑ Back
```

- Full screen over the game's hero (as Home crops it), dimmed. Two panes like Settings: Right/Accept enter the pane, Left at its edge or Back return to the sections, Back from the sections leaves (to Home, or to Properties when opened from there). Each section's second line says where it comes from now: Automatic, Picked (SteamGridDB), Your file, Game folder or None; Wrong game? shows the match.
- **Cover, Hero, Logo, Icon:** a strip of the matched game's SteamGridDB images (safe; the best first, square covers first), fetched when the section is focused and cached, so they show offline ("Offline · saved results"; "Can't reach SteamGridDB" without saved ones). Heroes and logos list animated images too, first while Animated artwork is on, each with a play badge in its bottom-left corner; when focus rests on one for 300 ms it plays in its tile. An animated image over 10 MB has no preview (its size shows instead) and downloads when picked. The one in use has an accent check; Accept uses the focused one and Home changes at once. Then Use a file… (the file list), Reset to automatic, and for the hero Position.
- The page's background is the hero in use as Home shows it, animated too.
- **Position:** heroes (~3.1:1) are cropped to the screen. Accept fades the page to the bare hero exactly as Home draws it (playing, if animated); Left/Right move the crop by 5 % (live on Home too), Accept saves, Back cancels. 0 % shows the hero's left edge, 100 % its right edge, Center by default.
- **Theme music:** Now (Your file / Game folder / None), a warning line when Game themes is off in Settings, then Choose a file…, Start at (Left/Right in 1 s steps, restarting the preview there) and Remove theme (a picked file is deleted and the game folder's theme plays again; a game-folder theme is ignored, never deleted). While focus is in this pane the theme previews (crossfading from the ambient), even with Game themes off.
- **Wrong game?:** Keep current match (focused first), then up to 8 games the matcher's searches returned (header name, then file name), each with its best cover's thumbnail and the search that found it. Choosing one matches the game to it: its automatic art is fetched again, SteamGridDB picks made from the old match go (picks from files stay), the hero is centered and the old candidates are dropped.
- **File list** (`components/file_list.slint`, the Downloads inbox later): wide, the sections collapse to a rail. Images or music files from Downloads and the game's own folder, newest first: a thumbnail for images, the duration for audio, "Downloads · 2 h ago". Focusing a music file for 500 ms previews it. Accept copies the file into the game's data (the original is only read); Back returns.

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
- **Emulators:** one row per emulator ("Dolphin · Balanced", "RetroArch"; the category's status is "Dolphin · <preset>"). Accept opens its page in the right pane (narrow): the title, then for Dolphin a Preset row (Performance, Balanced, Quality; "Custom" once a preset row differs; its description names the preset recommended for this GPU), the settings of `data/profiles/dolphin/settings.toml` and "Reset to defaults" (confirmation). Rows show their value; the focused one shows ‹ value › and steps with Left/Right (Accept steps forward). Under the list: the focused row's description and "Applies next launch", or "Restart <game> to apply" while a game of that emulator runs or is frozen. The list scrolls by whole rows. RetroArch's page says "More settings coming soon". Back returns to the list.
- Display & sound: "Console icons" and "UI sounds", each showing the selected pack and opening a list of "Hearth default" plus the installed packs with files for it (name, "by <author>"); a check marks the selection. Choosing saves it in `hearth.toml`; icons and sounds change at once. Below them: UI sounds volume and Music volume (Left/Right in steps of 10, a level bar and the number), Mute all, Music in overlay, Game themes, Animated artwork (on by default; "Off · Reduce motion" while that is on) and Reduce motion (Accept toggles, "On" in accent). The last two are saved under `[display]`.

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
- **The game's controller disconnects:** the game freezes under the overlay (the quick menu stays usable with another controller) and a card stays at the bottom right, like the other controller notices: Kenney's disconnected controller (warning color), "Controller disconnected", the controller's name, "Reconnect it to continue". Back within 2 s, the game resumes by itself ("Controller connected" for a few seconds); later, Resume is focused and the card says "Controller connected · Resume when you're ready". Pressing a button on another controller to take over needs telling pads apart in game (InputPlumber merges them into one virtual pad): for the multiplayer round.

## 5. Shared components

`Tile`, `Card` (square / wide / portrait variants), `TopBar`, `TabStrip`, `Chip`, `Button` (primary / secondary / danger), `ListRow` (icon, label, value), `Widget`, `DockButton`, `SectionHeader`, `HintBar` (controller-aware glyphs), `OnScreenKeyboard`, `FolderPicker`, `ConfirmDialog`, `Toast`.

## 6. Game artwork

- Per game, once: identify (ROM header first: GameCube game id + internal name, N64 game code + internal name; else the file name cleaned of No-Intro/Redump tags), search SteamGridDB, keep the match (its id is stored permanently), then fetch the top-voted static, safe (`nsfw`, `humor`, `epilepsy` off) grid (square, else closest to square), hero, logo and icon.
- Cache: `<cache>/art/<art key>/` (`~/.cache/hearth/art/`, or `$HEARTH_HOME/cache/art/`), where the art key is `<system>/<serial>` or `<system>/name-<hash of the cleaned title>`. Never re-fetched unless files are missing (or a later "Refresh artwork").
- Display copies: Home never decodes the originals. Next to them, `sized/<class>/<name>.qoi` (named after the source: `hero.qoi`, `hero-animated-still.qoi`, `local-hero.qoi` for a local file) are made for the screen's class (its height rounded up to 720/1080/1440/2160): hero as tall as the screen, logo inside 2× the 640×260 box, grid 2× `tile-focused`, all scaled with the class and never enlarged. Made on first use (already-cached art included), remade when the original is newer or the screen class changes (the class is remembered in `<data>/display-class`). Originals stay for "Change artwork" and other screens.
- **Animated art** (Settings → Display & sound → Animated artwork, on by default; off under Reduce motion):
  - Fetching: with it on, a matched game also gets SteamGridDB's best animated hero and logo (`types=animated`: animated WebP or APNG; GIF and WebM aren't served there) beside the static ones, `hero-animated.<ext>` and `logo-animated.<ext>`, up to 30 MiB each (SteamGridDB's download cap). Turning it off or Reduce motion needs no refetch: the static ones stay. Same queue and backoff as the rest; a kind looked up once isn't looked up again (`<kind>-animated.none` when none was taken).
  - Detection by content, never the extension: animated WebP (VP8X animation flag), APNG (`acTL` before the image data), GIF (more than one image). Picks and game-folder files get the same treatment (SteamGridDB serves animated WebP named `.png`).
  - What shows: while it may play, a hero or logo with an animated version shows that version's own still (the first frame of a hero, the most opaque of a logo's), so playback starts from the image already there; otherwise the static one.
  - Playback: only Home's focused game, once focus has rested on it for 500 ms (like its theme), and the Customize page's background and focused candidate. A focus move, leaving Home, the overlay or a running game stops it at once. Frames are decoded off the UI thread at the display class's size (logos 1× Home's box) and kept in `sized/<class>/<name>.anim` (QOI frames, decoded once: the still comes from the same file), so later plays start within 0.1 s. Each frame keeps its own delay (under 20 ms means 100 ms, as browsers do). WebP frames disposed to background are cleared to transparent, as browsers do.
  - Budget: an animation whose frames fit its cap (hero 96 MB, logo 48 MB decoded; 160 MB for everything at once) and one 8192 px texture plays from a sprite sheet uploaded once; a bigger one keeps 8 frames decoded ahead of the one shown (window); one that doesn't fit even that shows its still. A frame not decoded in time never holds anything up; a track with more than 25 % of its frames late over 2 s gives up and shows its still. The `.anim` files take 2 GB at most, the least recently played games' going first.
  - Cost on this laptop (release, i5-1145G7 / Iris Xe, 1080p): OoT's animated logo + hero 6.4 % of a core over 0.7 % idle; a 27 MB animated hero (469 frames, 311 MB `.anim`) 19 %. Most of it is Slint's GPU renderer redrawing the whole window at each flip.
- Loading order: the focused cell's hero and logo, the grids in view, the neighbours' heroes and logos, then the rest, on up to 3 threads.
- A game's own art wins over the cache: `grid`, `hero`, `logo`, `icon` (`.png`, `.jpg`, `.jpeg` or `.webp`) in its game folder (see "Game folders" below). The user's picks win over both.
- **Picks** (the Customize page) are the user's data, in `<data>/games/<art key>/` (`.dev-data/data/games/gc/GZ2E01/`): copies of the chosen files (`hero.png`, `theme.mp3`…) and `custom.toml` (per kind: `source = "sgdb"` with its `asset` id, or `"file"` with the `from` path for information; the hero's `position`; the theme's `start`, or `source = "none"` to play none). Keyed by art key like the cache, so dumps of one game share them, moves keep them, and every profile sees them. They are never deleted with the library's clean-up of vanished games (only by deleting the game from Hearth, later); orphans stay (Storage can offer "Leftover customizations" later). **A game without a header serial is keyed by its cleaned file name** (`<system>/name-<hash>`): renaming its file (beyond No-Intro/Redump tags) loses its picks, match and cached art.
- Candidates for the Customize page come through the art worker (one request at a time, like the art fetch). After a network error its requests back off like the art queue (5 min doubling to 6 h; a success ends it) and answer from the cache: `<cache>/art/<key>/candidates/<kind>.json` and thumbnails beside it; the matcher's games are kept in the library (`art_candidates`). `cargo run -p hearth-core --example art_probe -- --candidates <system> <rom>` lists them read-only.
- `--mock-sgdb=fixtures` serves generated candidates (look-alike games, six images per kind, wide heroes with bars that show the crop). It only runs with a throwaway `HEARTH_HOME` (holding `.throwaway`): `scripts/test-home.sh` makes `.dev-data/test-home` (a copy of the dev config and library, an empty cache) and points `HEARTH_DOWNLOADS` at `.dev-data/downloads` (fixtures from `cargo run -p hearth-core --example make_fixtures`). `--mock-network` is still the always-offline provider.
- **Game folders.** A game in its own folder under the system folder (a folder whose whole contents are that one game: one ROM; later an `.m3u` with its discs, or a folder-based game) keeps its extras there under fixed names:

  ```
  <games>/gc/Twilight Princess/
    Legend of Zelda, The - Twilight Princess (USA).nkit.iso
    theme.mp3                     # or .ogg, .flac
    hero.png  logo.png  grid.png  icon.png    # or .jpg, .webp
  ```

  A folder holding several games is only a grouping folder (no extras are read from it), and games laid out flat in the system folder simply have none. Nothing is matched by the ROM's file name. Lookups go through one chain (`hearth-core::extras`): the user's pick (`hearth-core::custom`, see "Picks") > the file in the game folder > the pack or Hearth's default (the ambient) or the art cache. A pick of "none" stops the chain (a removed theme). `cargo run -p hearth-core --example theme_probe -- ~/Games [title]` prints each game's folder and the files tried; the same paths are logged at debug (`hearth_core::extras`).
- **Moving a game keeps it.** A game's id is its path under the system folder, so moving a ROM into its own folder changes it; the next scan sees a game gone and one new with the same system and file name, and moves the row (playtime, last played, favorite, art key; the SteamGridDB match and cached art are keyed by the art key) to the new id. Only an unambiguous pair moves; renaming the file itself still starts it over.
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

[gain]                # corrects the pack's own mastering, in dB (±24)
ui_db = 10            # UI sounds
music_db = -4         # music
```

Without loop points the whole file loops with no crossfade (gapless: made for files that loop by construction, like Hearth's own). `cargo run --release -p hearth-audio --example loop_seam -- <file> <start> <end> <ms> out.wav` renders the 10 s around the seam with levels, to check loop points (`<file> auto out.wav`: the points a theme gets automatically).

**Game themes loop by themselves** (`hearth-audio::analyze`, cached per file in `<cache>/audio/`):
- the first play starts at the first sample above −50 dBFS (leading silence skipped, intros kept), plus the game's "Start at";
- the loop runs between the first and last 50 ms windows within 10 dB of the track's median level, then moves inward past the rest of a fade (while the level, smoothed over 500 ms, keeps rising toward the middle and is more than 1 dB under the median), so fade-ins and fade-outs stay out and no pass dips;
- the end crossfades into the start over 2 s (equal power).
The first play starts at once from a quick scan for the first sound while the analysis runs; loop points that arrive after the loop start went by make the looper decode the head again at the first loop end.

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
- **Music:** the ambient loop on Hearth's screens, fading in over 300 ms. Silent whenever a game runs unfrozen: launching fades it out over 300 ms while the game starts; Resume unfreezes at once and fades it out over 150 ms. In the overlay over a frozen game it plays only with "Music in overlay" (off by default: you pause to talk to someone) (continuing where it left off; never the game's theme); over a game whose suspend policy is off it stays silent.
- **Game themes:** they also play on the game's Customize page (previews take over while a file or the Theme section is focused). The theme picked on the Customize page, else `theme.ogg` (or `.mp3`, `.flac`, `.wav`) in the game's own folder (§6 "Game folders"). When a game with a theme stays focused on Home's Games row for 500 ms, the ambient crossfades to it (800 ms), and back once focus settles elsewhere or Home is left. Themes are for browsing Home: the overlay always plays the ambient. Themes extracted from dumps will be another source in the same chain.
- **Volumes** (Settings → Display & sound): UI sounds (default 80) and Music (default 70), 0–100 in steps of 10 on a dB scale (100 = 0 dB, each step −5 dB, 0 = silent); music always sits a further 6 dB under the UI sounds, so it stays a backdrop; Mute all; Music in overlay; Game themes. Changes apply at once (the UI volume plays a move at its new level) and are saved under `[audio]` in `hearth.toml`. System volume (PipeWire, the overlay's Volume button) is separate and later.

## 8. Assets and licenses

- Font: Manrope, SIL Open Font License, in `assets/fonts/` with its license file.
- Icons: Lucide (ISC), in `assets/icons/` as SVG. Console glyphs in `assets/icons/consoles/` are Hearth's own.
- Button glyphs: Kenney Input Prompts (CC0), in `assets/glyphs/`.
- Controller icons (top bar, Controllers widget and page, controller cards): Kenney Input Prompts (CC0), in `assets/icons/controllers/` (DualSense, DualShock 4, Xbox, Switch Pro, generic, disconnected).
- Sounds: Hearth's own, synthesized (CC0), in `assets/sounds/`.
- PS5 screenshots stay in `docs/reference/` (gitignored) and are never shipped.

## 9. Profiles and storage

Round D made the storage profile-ready: everything that belongs to a person lives under
`<home>/profiles/<id>/`, so adding profiles (picker, guests, per-controller sign-in) never moves
data again. There is no profile UI yet; one profile is active per run, `default` unless
`--profile <id>` says otherwise (an unknown id creates a fresh profile: no saves, the shipped
controls). Code: `hearth-core::users` (`ProfileId`, `ProfileDirs`, `UserProfile`, `migrate`).

### Layout

```
<home>/                              (HEARTH_HOME, or the XDG dirs: config, cache, data for the rest)
  config/hearth.toml                 shared (see "Settings, later" below)
  config/controls/device.toml        shared: names of this box's pads
  data/library.sqlite3               shared library; play history per profile (profile_id)
  data/games/<art key>/              Customize picks and emulator.toml (Game settings), shared
  data/display-class, hearth.lock    shared
  cache/ packs/ runtime/             shared (art, audio analyses, packs, InputPlumber profiles)
  emulators/retroarch/system/        shared: BIOS and core system files
  emulators/dolphin/shared.toml      shared: Dolphin's console settings (preset, changed rows)
  profiles/
    migration.toml                   progress of the move from before profiles
    <id>/
      profile.toml                   name, created
      saves/n64/<rom>.srm            RetroArch battery saves, one flat folder per system
      saves/gc/Card A/<USA|JPN|EUR>/ GameCube GCI folders (Card B likewise)
      saves/gc/MemoryCardA.<region>.raw   GameCube raw memory cards (if Dolphin uses them)
      saves/wii/nand/                the Wii NAND (Wii saves live in it)
      controls/systems/<system>.toml
      controls/games/<system>/<serial>.toml
      emulators/retroarch/           retroarch.cfg (merged at launch), hearth-applied.toml, states/, config/,
                                     remaps/, logs/, playlists/, screenshots/
      emulators/dolphin/             Dolphin's user dir: Config/, GameSettings/, hearth-applied.toml, StateSaves/, GC/SRAM.raw,
                                     Cache/, Load/, Logs/, …
  backup/<YYYYmmdd-HHMMSS>/          copies made before moving data, with SHA256SUMS
```

`saves/<system>/` holds what the *game* saves (battery saves, memory cards, the NAND); emulator
save states stay in the emulator's folder (they are emulator-specific).

- **Per profile:** `profile.toml`, saves, controls (systems and games), the RetroArch and
  Dolphin folders, and in the library: playtime, last played (recents), favorites and play
  sessions (`profile_games`, `play_sessions`, keyed by profile id).
- **Shared:** the library itself (games, art status, queue, candidates), the art cache, Customize
  picks, packs, the SteamGridDB key, network and system settings, `device.toml`, BIOS files,
  runtime files.
- **Dolphin's cache is per profile.** Dolphin has no setting for its cache folder: `D_CACHE_IDX`
  follows the user directory (`-u`), and only `XDG_CACHE_HOME` moves it when there is no `-u`
  (`UICommon::SetUserDirectory`). The shader cache is rebuilt per profile; it is small (7 MB for
  TP). Sharing it (it depends on the GPU, driver, backend and graphics settings, which belong to
  the console) is deferred to the profiles round: the only way is a directory symlink, and that
  round decides whether regenerable caches may be an exception to "path keys, never symlinks"
  below.
- **How emulators find the saves:** RetroArch gets `savefile_directory = saves/<system>` with
  `sort_savefiles_enable = false` and the shared `system_directory`, all absolute, in the config
  Hearth writes at every launch. Dolphin gets `-u emulators/dolphin` and `-C` overrides
  `Core.GCIFolderAPath/BPath` (`saves/gc/Card A|B`; Dolphin appends the region, Modern style:
  `JPN`, not `JAP`), `Core.MemcardAPath/BPath` (`saves/gc/MemoryCardA.raw`; Dolphin inserts
  `.<region>`) and `General.NANDRootPath`. Both card kinds are always given: the profile's own
  Dolphin.ini (`Core.SlotA`) decides which one is used. The Flatpak gets the profile's folder.
- **Controls** resolve per (profile, system, game): `controls::UserControls` is one profile's
  files; `device.toml` stays shared.

### Settings, later (not moved yet)

| `hearth.toml` | Becomes | Why |
|---|---|---|
| `[packs] console_icons`, `ui_sounds` | per profile | a theme is personal (the PS5 does it per user) |
| `[audio] music_in_overlay`, `game_themes` | per profile | taste |
| `[audio] ui_volume`, `music_volume`, `mute` | shared | the room's speakers |
| `[display] animated_art`, `reduce_motion` | per profile | accessibility and taste |
| `[art] steamgriddb_key` | shared | one account for the box |

### The move from before profiles

At startup, after the single-instance lock and before anything opens the library or an emulator
folder, `users::migrate` moves an install from before profiles into `profiles/default/`:

1. **Plan** from what exists: battery saves of known RetroArch cores (`Mupen64Plus-Next` → `n64`;
   other cores' saves stay in the profile's RetroArch folder, logged), Dolphin's GCI folders and
   raw cards, its Wii NAND, RetroArch's `system/` to the shared folder, then both emulator folders
   and `config/controls/{systems,games}`. Refused before anything happens if a destination exists,
   a move would cross filesystems (it must be a rename), or the free space is under twice the
   backup.
2. **Backup** of what can't be regenerated: saves, states, controls, emulator config files and
   the library database (with `-wal`/`-shm`). Not caches (Dolphin `Cache/`, RetroArch `cache/`,
   `thumbnails/`), logs, dumps, RetroArch's `autoconfig/` (rewritten at launch) or BIOS files:
   they are moved unchanged and listed in the summary. Copies keep their modification time,
   symlinks are copied as links. `SHA256SUMS` (the `sha256sum -c` format) is written, and every
   copy is re-read and compared (size and hash) with its original.
3. **Moves**, one `rename` at a time, each recorded in `profiles/migration.toml` (written
   atomically). A killed run resumes at the next start: a move whose source is gone and whose
   destination exists counts as done; a backup interrupted mid-way is thrown away and redone.
4. **Finish:** `profile.toml`, the library upgrade (version 4: the history moves to `default`),
   and a scan of the moved emulator configs for paths naming the old folders (`retroarch.cfg`
   is rewritten at every launch, so it is only noted).

Any error while moving undoes the moves made, newest first, and records `rolled_back`; Hearth
then starts with the notice "Couldn't move your saves into your profile · Nothing was changed.
Games won't start until this is fixed (see the log)", no game launches, and the next start
tries again. `Library::open` refuses a library from before profiles that holds games
(`NeedsProfileUpgrade`), so after a failure nothing upgrades it without the backup; only the
migration's `open_upgrading` does. A finished migration records `done` and later starts do
nothing. `cargo run -p hearth-core --example migration_plan` prints the plan and the backup
size without changing anything.

### Emulator settings

Users aren't stuck with Hearth's defaults: Hearth owns only the keys it needs, and every other key
is the user's. Code: `hearth-core::emusettings` (`doc` the format-preserving editor, `schema`,
`store`, `merge`, `gpu`, `backup`), `profiles/dolphin.rs`, `profiles/retroarch.rs`.

- **Locked keys** (Hearth's, re-applied at every launch, a hand edit is restored and logged):
  fullscreen, input and the virtual pad, hotkeys off, pop-ups and on-screen messages off, paths
  and saves, exit behavior, update checks, analytics, Discord; later the RetroAchievements token
  and hardcore. Dolphin's `GCPadNew.ini` and `Hotkeys.ini` are wholly Hearth's and still
  regenerated. RetroArch's locked keys are exactly what Hearth wrote before (its settings, paths,
  hotkey binds, the input translation's keys).
- **User-owned keys** are everything else; Hearth never writes them. The settings Hearth offers
  are user-owned too: Hearth writes them from its layers but keeps a value changed in the
  emulator itself.
- **Merged, never regenerated.** Only the lines of the keys Hearth manages change; unknown keys,
  comments and blank lines stay byte for byte; a missing key goes at the end of its section;
  files are written atomically and only when their text changes. Dolphin drops comments itself
  (measured below).
- **Layers**, lowest first: the emulator's defaults → the console base shared by every profile,
  `<home>/emulators/<emu>/shared.toml` (the preset and the rows changed in Settings) → the profile
  layer (`profiles/<id>/emulators/<emu>/`, supported, empty in E.1) → the game's overrides,
  `<data>/games/<art key>/emulator.toml` (beside its Customize picks, shared by every profile,
  deleted only with the game) → locked keys. Graphics belong to the console, so they live in the
  shared base.
- **How they reach Dolphin.** Its layers (`Common/Config/Enums.h`, lowest first): Base (the `-u`
  ini files) < CommandLine (`-C`) < GlobalGame (`Sys/GameSettings`, shipped game fixes) <
  LocalGame (`<-u>/GameSettings/<id>.ini`). Console settings go into the Base files
  (`Dolphin.ini`, `GFX.ini`), never `-C`: the command line beats Base, so a value changed in
  Dolphin's own UI would be ignored. A game's overrides go into `GameSettings/<id>.ini`, which
  also beats Dolphin's shipped fixes (TP's `Sys/GameSettings/GZ2.ini` turns arbitrary mipmap
  detection on: only a per-game value changes that). Locked keys go into `Dolphin.ini` /
  `Logger.ini` and stay `-C`; locked `[Core]` keys found in a game INI are removed. Game INI
  sections: `GFX.Settings` → `[Video_Settings]` (likewise Hardware, Enhancements, Hacks),
  `Main.Core` → `[Core]` (`ConfigLoaders/GameConfigLoader.cpp`).
- **How they reach RetroArch.** `retroarch.cfg` (`--config`) is merged in place; no
  `--appendconfig` (appended files override the main one and its "Save configuration" would bake
  them in). No curated settings yet (E.2).
- **What Hearth applied** (`hearth-applied.toml` in each profile's emulator folder): every key it
  wrote, per file and section. At launch a managed setting whose file value no longer means what
  Hearth applied was changed outside Hearth: the file keeps it, and it is adopted into the layer
  it came from (Settings shows it; logged "kept a setting changed outside Hearth"). A value
  Hearth has no choice for (InternalResolution = 5) is left to the user until it is changed in
  Hearth again (`yielded`). A key Hearth wrote before and no longer manages goes only if
  unchanged. "Means the same" compares values as the emulator reads them (`True`/`true`, `2`/
  `2.0`), ignores line order, and in a main file treats a missing key as the default (a game INI
  missing a key means "not set here").
- **What Dolphin 2609 does to its files** (measured 2026-10-30, TP started and quit with
  SIGTERM, 0.5 s): at startup it rewrites `Dolphin.ini` and `GFX.ini`, dropping every comment and
  adding `[Analytics] ID`; values, spellings, order, unknown keys and keys at their defaults are
  kept. On exit it appends defaults of its own (`[DSP]`, `[NetPlay]`, `[SDL_Hints]`; every log
  type and `WriteTo*` in `Logger.ini`); `GFX.ini` is unchanged. None of it is taken as a change by
  the user (golden test `dolphin_rewrite`).
- **The first merge** copies the files it will touch to
  `backup/<stamp>-emulator-settings/<profile>/<emu>/` with `SHA256SUMS`, once. A `retroarch.cfg`
  generated before (its old header, no applied record) is converted: all its keys count as
  Hearth's, its header changes, nothing else moves.
- **Schema** (`data/profiles/<emu>/settings.toml`, checked by tests): per setting id, label,
  description, file/section/key, type (bool or choice; a choice may set other keys of its
  section, e.g. anti-aliasing sets `MSAA` and `SSAA`), friendly choices, default, scope (console
  or per game), applies (next launch / restart game) and `breaks_hardcore` (stored only). Every
  Dolphin key was checked against Dolphin 2609's source (`verified_with`).
- **Presets** (Performance, Balanced, Quality) set only the speed-versus-quality rows (internal
  resolution, anti-aliasing, anisotropic filtering, shader compilation, arbitrary mipmaps); a
  preset row changed afterwards shows "Custom", other rows never do. The preset is recommended
  from the GPU games run on (`/sys/class/drm/card*/device`: display class, the `boot_vga` one
  first; rules by PCI vendor/device in the schema: Iris Xe → Balanced, older Intel →
  Performance, Radeon 680M–890M, Arc and discrete GPUs → Quality, unknown → Balanced) until the
  user picks one.
- **Testing:** dev socket words `emu_state`, `emu_set`, `emu_game_set`, `emu_game_reset`,
  `emu_effective` (`scripts/send-action.sh`).

**Widescreen** (round E.1b, Dolphin): a game's Widescreen row turns on the widescreen Gecko
code Dolphin ships for it, never a downloaded one. `emusettings::gecko` reads the game's shipped
files (`Sys/GameSettings`: `<id[0]>`, `<id[0..3]>`, `<id>`, `<id>r<rev>`, merged like Dolphin's
`LoadDefaultGameIni`; the disc revision comes from the header) and takes the first `[Gecko]`
code whose name says widescreen or 16:9 and isn't an add-on (108 games in 2609; for games with
HUD variants, the first, "Normal HUD"). No code: no row. It is a layer in the merge, above the
game's overrides and under the locked keys, for the game INI only: `[Gecko_Enabled]
$<code>`, `[Core] EnableCheats = True` (Dolphin reads it through the game layer, so cheats are
on for that game only, never globally; codes the user enabled for the game run too),
`[Video_Settings] AspectRatio = 1` (Force 16:9) and `wideScreenHack = False`. Off, the normal
layers supply those keys again (the game's own Stretch comes back); Hearth removes the code
line only if it added it. A value changed outside Hearth while it's on is kept and not adopted
(Widescreen would cover the lower layer) until it's turned off. Stored as `widescreen = "True"`
in the game's `emulator.toml`; Reset to console defaults turns it off. `breaks_hardcore` comes
from `Sys/ApprovedInis.json`: the code's name under the game's INI names with the SHA-1 Dolphin
computes (line count u64, then address and data u32, little-endian); TP's "16:9 Widescreen v2"
is approved. Stored and shown by `emu_state` only.

**Rejected: shared files linked into each profile's folder.** Emulators save settings by
writing a temporary file and renaming it over the original, which replaces a symlink with a
plain file (the profile silently forks); Dolphin rewrites its whole `Dolphin.ini` and `GFX.ini`;
hard links break the same way; and the Flatpak sandbox needs every link target granted. A merge
at launch has none of these problems and keeps Hearth the only writer of shared settings.

Shared assets (texture packs, resource packs) will go by path keys (`General.LoadPath`,
`General.ResourcePackPath` → `<home>/emulators/dolphin/…`), not copies; moving a texture pack
folder there later is a move of user files and needs round D's backup-and-rename care.

# PS5 Home Dashboard

Source: `docs/reference/ps5/home.jpg`

## 1. Purpose

This is the primary home dashboard screen displayed when the console boots or returns from a game. It provides quick launching of recent games/apps, immediate status widgets (storage, controller battery, trophies), and content cards.

## 2. Layout

* **Overall structure:**
* **Top Navigation Zone:** Y: 0–120px (0%–11.1%), X: 0–1920px (0%–100%).
* **Recent Apps Bar Zone:** Y: 120–380px (11.1%–35.2%), X: 0–1920px (0%–100%).
* **Status Pill Row Zone:** Y: 420–490px (38.9%–45.4%), X: 0–1920px (0%–100%).
* **Widget Grid Zone:** Y: 510–1030px (47.2%–95.4%), X: 0–1920px (0%–100%).


* **Every visible element:**
* `Top Tabs ("Games", "Media")`: x: ~90px, y: ~40px, w: ~300px, h: ~40px. Left-aligned.


* `Header Icons (Search, Settings, Profile)`: x: ~1350px, y: ~40px, w: ~220px, h: ~40px. Right-aligned, gap ~30px.


* `Clock ("5:26 PM")`: x: ~1690px, y: ~40px, w: ~140px, h: ~40px. Right-aligned.


* `Focused Tile Title ("Welcome")`: x: ~350px, y: ~245px, w: ~300px, h: ~35px. Left-aligned beneath focused tile.


* `Status Pills`: x: ~180px, y: ~430px, h: ~46px, pill gap ~20px.




* **Grid or row structure:**
* **App Bar:** Single row. Focused tile size: ~160x160px. Unfocused tile size: ~105x105px. Gap: ~18px. Visible tiles: ~13 tiles total across the screen width.


* **Widget Grid:** 3 main columns + right overflow.
* Column 1 (x: ~180px): Trophies (~490x320px), Controller status (~490x150px).


* Column 2 (x: ~700px): Storage (~490x150px), News banner (~490x320px).


* Column 3 (x: ~1220px): Store promo tile (~490x320px), Game catalog strip (~490x150px).


* Column 4 (x: ~1740px): Accessibility widget (~490x480px, cut off right edge at ~180px visible width).







## 3. Visual style

* **Colors:**
* Background: Fullscreen art image background overlaid with a subtle dark vignette gradient (`#000000` with alpha 0.4 at top/bottom).


* Surfaces: Dark translucent rounded rectangles (`#12161A` at 65% opacity / `#1C222A` at 80% opacity).


* Text: Primary `#FFFFFF`, Secondary `#9CA3AF`, Accents `#60A5FA` (Blue) and `#FBBF24` (Yellow).


* Focus Highlight: Bright glowing white border (`#FFFFFF` with outer aura glow).




* **Typography:**
* Top Tabs: ~28px, Bold / Medium, letter-spacing +0.5px.
* Widget Headers: ~20px, Semi-Bold, sentence case.
* Body Text / Subtitles: ~15px, Regular.
* Free alternative font suggestion: **Manrope** or **Outfit**.


* **Corner Radii, Borders & Effects:**
* App tiles: Corner radius ~20px.


* Widgets / Cards: Corner radius ~16px.


* Status pills: Fully rounded / capsule (~23px radius).


* Background blur behind surfaces: ~20px background blur.




* **Icons:**
* Linear/stroke icons (~2px stroke weight) for system settings, search, trophies, and controller. Size ~24x24px to ~32x32px.





## 4. Focus and states

* **Focused State:** Focused top bar app tile scales up to ~150% (~160x160px vs ~105x105px), displays an outer white stroke halo, and renders a title label directly below it.


* **Unfocused State:** Standard ~105x105px size, 85% opacity.


* **Progress / Badges:** Linear progress bar in storage widget (blue/purple fills), controller battery indicator, level percentage bar.



## 5. Controller navigation

* **Focus Start:** Default focus starts on the most recently used app tile in the App Bar (e.g., "Welcome").


* **Navigation:**
* Left/Right in App Bar: Scrolls horizontally through app tiles.
* Up from App Bar: Jumps to Top Bar (Games, Media, Search, Settings, Profile).


* Down from App Bar: Focuses down into the Status Pills / Widget Grid area.


* Edges: Leftmost edge stops; Rightmost edge scrolls new app tiles/widgets into view.


* **Buttons:**
* Confirm (Cross): Launch selected app/widget action.
* Back (Circle): Return focus to top app bar.
* Options: Open context menu for selected app.
* L1/R1: Switch between main tabs ("Games" / "Media").





## 6. Motion (inferred)

* **Focus Transition:** Smooth spring/bezier scale animation (~200ms `ease-out`) when switching focus between app tiles.
* **Background Crossfade:** Background image crossfades (~400ms `ease-in-out`) when changing focused game tile.
* **Horizontal Scroll:** Smooth slide offset (~250ms) as off-screen tiles are brought into view.

## 7. Content and data

* **Data Required:**
* `apps`: List of installed/recent titles (id, title, icon_url, hero_bg_url).
* `trophies_summary`: Total count, level, completion %, trophy breakdown (bronze, silver, gold, platinum).


* `storage_info`: Used space, free space (GB), system usage breakdown.


* `connected_controllers`: Battery level, slot number, device title.


* `news_feed`: Latest news card (headline, image, timestamp).





## 8. Adaptation for Hearth

* **Keep:** The two-tier vertical separation (top quick-launch strip + bottom contextual dashboard widgets).


* **Change:** Use a unified dark slate background with Hearth's custom accent colors (warm amber/emerald) instead of PlayStation blue. Remove all proprietary logos and store promotional cards.
* **Hearth Mapping:** Maps to **Home Screen**.
* Add Hearth-specific elements: Console badge on app tiles (e.g., "SNES", "PS2"), emulator status indicator ("Missing BIOS", "Ready"), and "Currently Running" status pill.



## 9. Slint implementation notes

* **Reusable Components:** `AppTile`, `WidgetCard`, `PillBadge`, `TopBarHeader`, `ProgressBar`.
* **Theme Globals:** `Theme.surface-bg`, `Theme.focus-border-color`, `Theme.radius-lg`, `Theme.font-family`.
* **Tricky Aspects:** Smooth horizontal shifting of the app bar while keeping the active item centered/expanded without layout jitter.

---

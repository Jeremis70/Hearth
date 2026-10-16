# PS5 Quick Menu / Action Center Overlay

Source: `docs/reference/ps5/overlay.jpg`

## 1. Purpose

This is a bottom overlay/quick bar displayed when pressing the system guide button over a dimmed background. It gives immediate access to recent screen captures, game news, system cards, and bottom utility shortcuts without leaving the current context.

## 2. Layout

* **Overall structure:**
* **Dimmed Background Zone:** Y: 0–1080px (0%–100%), X: 0–1920px (0%–100%) with ambient blur.


* **Cards Carousel Zone:** Y: 360–820px (33.3%–75.9%), X: 0–1920px (0%–100%).


* **Bottom Navigation Dock Zone:** Y: 940–1040px (87.0%–96.3%), X: ~380–1540px (~20%–80%).




* **Every visible element:**
* `Clock ("5:26 PM")`: x: ~1690px, y: ~40px, w: ~140px, h: ~40px. Top-right.


* `Quick Cards Carousel`: x: ~80px, y: ~360px, h: ~460px.


* `Bottom Dock Bar`: x: ~360px, y: ~960px, w: ~1200px, h: ~70px, centered horizontally.




* **Grid or row structure:**
* **Cards Row:** Horizontal row of cards.


* Card 1 ("New screenshot"): x: ~80px, w: ~420px, h: ~460px.


* Card 2 ("Official news"): x: ~520px, w: ~360px, h: ~460px.


* Card 3 ("Featured Tips"): x: ~900px, w: ~360px, h: ~460px. Focused/Expanded.


* Card 4 (Overflow): Partially visible on right edge.


* Card Gap: ~20px.




* **Bottom Dock Row:** ~10 round icon buttons in a single horizontal row, gap ~28px.





## 3. Visual style

* **Colors:**
* Background: Full screen background dimming (`#000000` at 70% opacity) with Gaussian blur.


* Card Surfaces: Dark translucent panels (`#1A202C` at 85% opacity); Focused card uses dynamic accent fill (e.g., Vibrant Blue `#0070D1`).


* Dock Surface: Continuous pill background or floating transparent bar.


* Text: Primary `#FFFFFF`, Secondary `#A0AEC0`.




* **Typography:**
* Card Category Title: ~14px, Medium, upper/sentence case.
* Card Main Title: ~22px, Bold.
* Metadata / Timestamp: ~13px, Regular, `#A0AEC0`.


* Free alternative font suggestion: **Inter** or **Outfit**.


* **Corner Radii & Borders:**
* Cards: Corner radius ~20px.


* Dock active icon highlight: Rounded rectangle (~12px radius) or circular container.


* Card stroke: Subtle 1px translucent border (`#FFFFFF` at 15% opacity).


* **Icons:**
* Minimalist white stroke icons (~24x24px) for dock shortcuts (Home, Media, Notifications, Music, Sound, Mic, Controller, Power).





## 4. Focus and states

* **Focused Card:** Scales up slightly (~103%), border glows white or takes a filled brand accent background, showing additional action hints.


* **Focused Dock Button:** Surrounded by a solid rounded blue/white highlight container.


* **Badges:** Notification badge dots (e.g., green dot with number '3' below notifications icon).



## 5. Controller navigation

* **Focus Start:** Focus starts on the last active Card in the carousel or the central dock shortcut.


* **Navigation:**
* Up/Down: Switches focus between the Cards Carousel row and the Bottom Dock Bar row.


* Left/Right (in Cards): Scrolls cards horizontally.


* Left/Right (in Dock): Moves focus across dock icons.




* **Buttons:**
* Confirm (Cross): Open the focused card detail / execute dock shortcut.
* Back / PS Button: Dismiss the overlay and return to active game/app immediately.





## 6. Motion (inferred)

* **Overlay Entrance:** Background blurs and darkens (~150ms), cards and dock slide up from bottom with subtle damping (~250ms `cubic-bezier(0.16, 1, 0.3, 1)`).
* **Overlay Exit:** Dock and cards slide down, background unblurs (~150ms).

## 7. Content and data

* **Data Required:**
* `recent_captures`: Latest screenshots/clips (thumbnail_url, timestamp, type).


* `quick_news`: Activity cards, tips, or achievement guides.


* `system_status`: Active notifications count, connected accessories, audio mute state.





## 8. Adaptation for Hearth

* **Keep:** The overlay concept—a lightweight bottom action bar and contextual cards overlay accessible at any moment.
* **Change:** Use a glassmorphic dark container for the bottom dock with rounded squircle geometry. Replace PlayStation-specific tips with emulator performance toggles, quick save state slots, and controller remap shortcuts.
* **Hearth Mapping:** Maps to **In-Game Overlay / Quick Menu**.

## 9. Slint implementation notes

* **Reusable Components:** `OverlayDock`, `DockButton`, `ActivityCard`, `DimmedBackground`.
* **Theme Globals:** `Theme.overlay-backdrop-color`, `Theme.dock-height`.
* **Tricky Aspects:** Handling modal key capture so all background input is suppressed while the overlay is active.

---

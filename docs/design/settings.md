# PS5 Settings Menu

Source: `docs/reference/ps5/settings.jpg`

## 1. Purpose

A full-screen vertical system settings list. Users access this screen to configure hardware, network, accounts, accessibility, storage, and audio parameters.

## 2. Layout

* **Overall structure:**
* **Header Zone:** Y: 0–180px (0%–16.7%), X: 0–1920px (0%–100%).


* **Vertical Options List Zone:** Y: 180–1020px (16.7%–94.4%), X: 0–1920px (0%–100%).




* **Every visible element:**
* `Screen Title ("Settings")`: x: ~95px, y: ~85px, w: ~300px, h: ~50px.


* `List Container`: x: ~300px, y: ~180px, w: ~1320px, h: ~840px, centered horizontally.


* `Setting Items (Row 1 to 9)`: Each row width ~1320px, height ~76px, vertical gap ~12px.


* Item icon: x: ~330px.


* Item text: x: ~400px.






* **Grid or row structure:**
* Single column vertical stack.
* Visible row count: ~9 rows visible simultaneously on 1080p screen.


* Rows extend past bottom edge for long lists.





## 3. Visual style

* **Colors:**
* Background: Dark atmospheric radial backdrop (`#0E1116` to `#050608`) with subtle top-left spotlight glow (`#232833`).


* Row Surfaces: Unfocused rows are transparent with thin bottom dividers (`#FFFFFF` at 6% opacity).


* Focused Row Surface: Dark translucent fill (`#1A222D` at 80% opacity) with bright white stroke border (`#FFFFFF`).


* Text: Primary `#FFFFFF`, Secondary `#8E98A8`.




* **Typography:**
* Title ("Settings"): ~40px, Light / Regular, `#FFFFFF`.


* Row Label: ~22px, Regular / Medium, `#E2E8F0`.


* Free alternative font suggestion: **Outfit** or **Inter**.


* **Corner Radii & Borders:**
* Focused Item Box: Corner radius ~10px.


* Divider lines: 1px height, full row width.




* **Icons:**
* White outline stroke icons (~28x28px, ~2px stroke width) aligned to the left of each text item (e.g., globe for Network, speaker for Sound, display for Screen and Video).





## 4. Focus and states

* **Focused State:** The entire active row gets enclosed in a clean rounded rectangle border with a subtle inner blue-grey highlight background.


* **Unfocused State:** Transparent row background with simple text, icon, and subtle bottom separator line.



## 5. Controller navigation

* **Focus Start:** Focus begins on the top row item ("Guide & Tips...") or the last active setting.


* **Navigation:**
* Up/Down: Moves highlight sequentially up and down through the vertical list.


* Left/Right: No effect on main list (or switches categories if sub-tabs exist).
* List auto-scrolls down when focus moves past the 6th row.


* **Buttons:**
* Confirm (Cross): Enter selected setting category menu.
* Back (Circle): Exit settings and return to Home dashboard.





## 6. Motion (inferred)

* **Focus Indicator Motion:** Smooth vertical glide animation (~150ms `ease-out`) moving the focus border frame between adjacent list items.
* **Page Slide:** Sub-menus slide in from the right (~200ms) when selected.

## 7. Content and data

* **Data Required:**
* `settings_groups`: Array of objects containing:
* `id`: String identifier.
* `label`: Display text.
* `icon_id`: Name of vector icon.
* `target_route`: Sub-menu destination.





## 8. Adaptation for Hearth

* **Keep:** The minimal, clean single-column vertical list with prominent icons and full-width focus enclosures.


* **Change:** Adopt Hearth's custom iconography and warm color scheme. Add quick status values on the right side of rows (e.g., Network: "Connected (Wi-Fi)", Storage: "450 GB Free").
* **Hearth Mapping:** Maps to **Settings Screen**.

## 9. Slint implementation notes

* **Reusable Components:** `SettingsRow`, `IconLabel`, `PageHeader`, `VerticalList`.
* **Theme Globals:** `Theme.settings-row-height`, `Theme.focus-stroke-width`.
* **Tricky Aspects:** Ensuring auto-scrolling keeps the focused row comfortable within the middle viewport height.

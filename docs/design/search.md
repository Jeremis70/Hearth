# Search / Discover Screen

Source: `docs/reference/ps5/search.jpg`

## 1. Purpose

This screen serves as a media and content discovery browser (Store/Catalog). Users see it when searching or browsing curated collections, featured titles, and subscription services.

## 2. Layout

* **Overall structure:**
* **Top Navigation & Search Zone:** Y: 0–220px (0%–20.4%), X: 0–1920px (0%–100%).


* **Section 1 (Trending Games Grid/Row):** Y: 220–580px (20.4%–53.7%), X: 0–1920px (0%–100%).


* **Section 2 (Subscriptions Banner Row):** Y: 580–980px (53.7%–90.7%), X: 0–1920px (0%–100%).


* **Bottom Helper Bar:** Y: 1020–1080px (94.4%–100%), X: 0–1920px (0%–100%).




* **Every visible element:**
* `Top Tabs ("Games", "Media", "Players")`: x: ~170px, y: ~50px, w: ~400px, h: ~40px.


* `Search Input Bar`: x: ~250px, y: ~110px, w: ~1500px, h: ~75px. Left icon x: ~280px. Voice search mic button x: ~180px.


* `Section Header 1 ("Trending games")`: x: ~170px, y: ~225px, h: ~30px.


* `Section Header 2 ("PlayStation Plus and other subscriptions")`: x: ~170px, y: ~590px, h: ~30px.


* `Bottom Hint ("L1/R1 Switch Tabs")`: x: ~1640px, y: ~1035px, w: ~220px, h: ~30px.




* **Grid or row structure:**
* **Trending Row:** Square artwork cards. Card size: ~300x300px. Gap: ~24px. Visible items: 5 full cards + 1 cut off at right edge (~60px visible).


* **Subscriptions Row:** Wide banner cards. Card size: ~520x220px. Gap: ~24px. Visible items: 3 full cards + 1 cut off at right edge.





## 3. Visual style

* **Colors:**
* Background: Deep dark gradient ranging from top-left soft spotlight (`#2A2E3D`) down to solid charcoal black (`#0D0F12`).


* Surfaces / Cards: Solid art covers or dark grey containers (`#181A20`). Search bar fill: `#1E242C` with border `#3A4452`.


* Text: Primary `#FFFFFF`, Secondary `#9EA6B2`, Discount highlight `#EAB308` (Yellow).


* Tag / Platform Pill: `#FFFFFF` at 20% opacity.




* **Typography:**
* Top Tabs: ~26px, Medium.
* Section Titles: ~22px, Regular / Semi-Bold, `#D1D5DB`.


* Search Placeholder: ~22px, Regular, `#8A94A6`.


* Card Title: ~16px, Semi-Bold.


* Card Subtext / Price: ~14px, Regular.


* Free alternative font suggestion: **Manrope**.


* **Corner Radii & Borders:**
* Search bar: Corner radius ~8px.


* Content Cards: Corner radius ~8px–12px.


* Platform Pill Badge: Corner radius ~4px.




* **Icons:**
* Search magnifying glass icon (~24px), microphone icon (~24px), pill controller badge icons.





## 4. Focus and states

* **Focused Card:** Expands slightly with a bright white outline border (~2px) around the perimeter.


* **Focused Search Input:** Border brightens to solid white, background darkens, cursor appears.


* **Discount Badge:** Yellow pill with dark text showing discount percentage.



## 5. Controller navigation

* **Focus Start:** Search bar input field or first item of "Trending games".


* **Navigation:**
* Up/Down: Navigates vertically between Search bar -> Trending Games Row -> Subscriptions Row.


* Left/Right: Scrolls horizontally within the current row.


* L1/R1: Switches top tabs ("Games" / "Media" / "Players").




* **Buttons:**
* Confirm (Cross): Select game card / open search virtual keyboard.
* Back (Circle): Move focus up or return to previous screen.



## 6. Motion (inferred)

* **Row Scrolling:** Smooth horizontal scrolling animation (~200ms `ease-out`) when navigating past visible screen boundaries.
* **Tab Change:** Horizontal page slide effect (~250ms).

## 7. Content and data

* **Data Required:**
* `search_query`: String.
* `trending_items`: List of items (title, cover_art_url, price, discount_pct, platform_tag, ownership_status).


* `subscription_providers`: List of providers (name, logo_url, banner_bg).





## 8. Adaptation for Hearth

* **Keep:** The search bar positioning and structured horizontal rows of content cards with right-edge cutoffs.


* **Change:** Replace "Subscriptions" row with "Emulators & Platforms" or "Collections" (e.g., RetroArch, Dolphin, Favorite Franchises). Replace store prices with launch status or game file sizes.
* **Hearth Mapping:** Maps to **Library / Discovery View**.

## 9. Slint implementation notes

* **Reusable Components:** `SearchBar`, `MediaCardSquare`, `MediaCardWide`, `SectionHeader`, `FooterHint`.
* **Theme Globals:** `Theme.bg-gradient`, `Theme.card-radius`.
* **Tricky Aspects:** Efficient rendering of horizontal scrolling lists in Slint using dynamic model offsets or `Flickable`.

---

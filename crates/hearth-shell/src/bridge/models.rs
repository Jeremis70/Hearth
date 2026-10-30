//! Builds the Slint models from core and platform data.

use std::rc::Rc;
use std::thread;

use std::time::Duration;

use hearth_core::library::LibraryEntry;
use hearth_core::model::{GameId, SystemId};
use hearth_core::systems::Systems;
use hearth_platform::network::{NetworkBackend, WifiNetwork};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};

use super::art::{ArtMap, ConsoleIcons};
use crate::{AppWindow, HomeData, HomeTile, SettingsCategory, SettingsData, TileKind, WifiRow};

/// Placeholder art gradients, picked per game from a hash of its id, shown until (or
/// instead of) real art.
const ART: [(u32, u32); 8] = [
    (0x3B5B3A, 0x1A2218),
    (0x2E6F6A, 0x13242B),
    (0x8C2F22, 0x2A100E),
    (0x5A2E6E, 0x1B1026),
    (0x8A7A5C, 0x26221C),
    (0x9C5A1E, 0x2B170C),
    (0x2E4A7A, 0x111A2B),
    (0x6E2E4A, 0x24101A),
];

fn rgb(hex: u32) -> Color {
    let [_, r, g, b] = hex.to_be_bytes();
    Color::from_rgb_u8(r, g, b)
}

fn art_for(id: &GameId) -> (Color, Color) {
    // FNV-1a: stable across runs (unlike std's RandomState hasher).
    let hash = id.as_str().bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    let (a, b) = ART[hash as usize % ART.len()];
    (rgb(a), rgb(b))
}

/// "12 h played", "35 min played", or "Not played yet".
pub fn format_playtime(playtime: Duration) -> String {
    let minutes = playtime.as_secs() / 60;
    match minutes {
        0 if playtime.is_zero() => "Not played yet".to_owned(),
        0 => "Less than a minute played".to_owned(),
        1..60 => format!("{minutes} min played"),
        _ => format!("{} h played", minutes / 60),
    }
}

/// "Playing for 12 min" / "Playing for 1 h 05 min".
pub fn format_session(played: Duration) -> String {
    let minutes = played.as_secs() / 60;
    if minutes < 60 {
        format!("Playing for {minutes} min")
    } else {
        format!("Playing for {} h {:02} min", minutes / 60, minutes % 60)
    }
}

/// "played 12 min" / "played less than a minute" for the end-of-session toast.
pub fn format_session_total(played: Duration) -> String {
    match played.as_secs() / 60 {
        0 => "played less than a minute".to_owned(),
        m if m < 60 => format!("played {m} min"),
        m => format!("played {} h {:02} min", m / 60, m % 60),
    }
}

/// "1.4 GB", "12.0 MB", "512 KB" (powers of 1024, like file managers).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// "last played today / yesterday / 3 days ago", from Unix seconds.
pub fn format_last_played(last_played: Option<i64>, now: i64) -> Option<String> {
    let days = (now - last_played?).max(0) / 86_400;
    Some(match days {
        0 => "last played today".to_owned(),
        1 => "last played yesterday".to_owned(),
        2..=30 => format!("last played {days} days ago"),
        _ => "last played over a month ago".to_owned(),
    })
}

/// "Played 12 h", "Played 35 min", or "Not played yet": the playtime pill on Home.
pub fn format_played(playtime: Duration) -> String {
    let minutes = playtime.as_secs() / 60;
    match minutes {
        0 if playtime.is_zero() => "Not played yet".to_owned(),
        0 => "Played < 1 min".to_owned(),
        1..60 => format!("Played {minutes} min"),
        _ => format!("Played {} h", minutes / 60),
    }
}

/// The greeting on the Welcome cell for a local hour (0–23).
pub fn greeting(hour: u32) -> &'static str {
    match hour {
        5..12 => "Good morning",
        12..18 => "Good afternoon",
        _ => "Good evening",
    }
}

/// The Home row: the Welcome cell, the library's games (running game first, then most
/// recently played), then the Game Library cell. Media and Settings live in the top bar.
pub fn home_tiles(
    entries: &[LibraryEntry],
    systems: &Systems,
    running: Option<&GameId>,
    now: i64,
    art: &ArtMap,
    icons: &mut ConsoleIcons,
) -> Vec<HomeTile> {
    let mut ordered: Vec<&LibraryEntry> = entries.iter().collect();
    // Stable sort keeps the library's recent-first order for everything else.
    ordered.sort_by_key(|e| Some(&e.game.id) != running);
    let games = ordered.into_iter().map(|e| {
        let (art_a, art_b) = art_for(&e.game.id);
        let tile_art = art.get(&e.game.id).cloned().unwrap_or_default();
        let icon = icons.icon(e.game.system.as_str(), systems);
        let badge = systems
            .get(&e.game.system)
            .map_or_else(|| e.game.system.to_string(), |s| s.short.clone());
        let mut subtitle = format_playtime(e.playtime);
        let last_played = format_last_played(e.last_played, now).unwrap_or_default();
        if !last_played.is_empty() {
            subtitle = format!("{subtitle} · {last_played}");
        }
        HomeTile {
            id: e.game.id.as_str().into(),
            title: e.game.title.as_str().into(),
            badge: badge.into(),
            subtitle: subtitle.into(),
            last_played: last_played.into(),
            played: format_played(e.playtime).into(),
            kind: TileKind::Game,
            art_a,
            art_b,
            running: Some(&e.game.id) == running,
            favorite: e.favorite,
            game_settings: has_game_settings(systems, &e.game.system),
            art: tile_art.grid,
            hero: tile_art.hero,
            hero_x: tile_art
                .hero_position
                .unwrap_or(hearth_core::custom::CENTER),
            hero_rev: tile_art.hero_rev,
            logo: tile_art.logo,
            console_icon: icon.image,
            console_icon_tint: icon.tint,
            monogram: icon.monogram.into(),
        }
    });
    let welcome = HomeTile {
        id: "welcome".into(),
        title: "Welcome".into(),
        kind: TileKind::Welcome,
        art_a: rgb(0x3A2A1E),
        art_b: rgb(0x0C0B0A),
        ..HomeTile::default()
    };
    let library = HomeTile {
        id: "library".into(),
        title: "Game Library".into(),
        badge: SharedString::new(),
        subtitle: "Every game, every console".into(),
        kind: TileKind::Library,
        art_a: rgb(0x4A3626),
        art_b: rgb(0x0C0B0A),
        ..HomeTile::default()
    };
    std::iter::once(welcome)
        .chain(games)
        .chain(std::iter::once(library))
        .collect()
}

/// Whether games of `system` have a Game settings page (per-game emulator settings: Dolphin's
/// for now).
pub fn has_game_settings(systems: &Systems, system: &SystemId) -> bool {
    systems
        .get(system)
        .and_then(|s| s.emulator())
        .is_some_and(|e| e.kind == hearth_core::systems::EmulatorKind::Dolphin)
}

/// Updates the Home tile row: row by row when the same tiles are there in the same order (so
/// Home keeps its tile elements and their animations), else replaced.
pub fn set_tiles(app: &AppWindow, tiles: Vec<HomeTile>) {
    let home = app.global::<HomeData>();
    let current = home.get_tiles();
    let same_ids = current.row_count() == tiles.len()
        && current
            .iter()
            .zip(&tiles)
            .all(|(old, new)| old.id == new.id);
    if same_ids {
        for (i, tile) in tiles.into_iter().enumerate() {
            if current.row_data(i).as_ref() != Some(&tile) {
                current.set_row_data(i, tile);
            }
        }
    } else {
        home.set_tiles(ModelRc::from(Rc::new(VecModel::from(tiles))));
    }
}

/// Settings categories with fake status values (network status comes from the backend).
fn settings_categories(network_status: &str) -> Vec<SettingsCategory> {
    [
        ("network", "Network", network_status),
        ("folders", "Game folders", "2 folders"),
        ("emulators", "Emulators", "5 detected"),
        ("controllers", "Controllers", "None connected"),
        ("display", "Display & sound", ""),
        (
            "system",
            "System",
            concat!("Hearth ", env!("CARGO_PKG_VERSION")),
        ),
    ]
    .into_iter()
    .map(|(key, label, status)| SettingsCategory {
        key: key.into(),
        label: label.into(),
        status: status.into(),
    })
    .collect()
}

fn network_status(networks: &[WifiNetwork]) -> String {
    networks.iter().find(|n| n.connected).map_or_else(
        || "Not connected".to_owned(),
        |n| format!("Wi-Fi · {}", n.ssid),
    )
}

fn wifi_row(n: &WifiNetwork) -> WifiRow {
    WifiRow {
        ssid: n.ssid.as_str().into(),
        strength: i32::from(n.strength),
        secured: n.secured,
        connected: n.connected,
    }
}

/// Installs the static models and wires callbacks for not-yet-implemented actions.
pub fn install(app: &AppWindow) {
    app.global::<HomeData>()
        .set_tiles(ModelRc::from(Rc::new(VecModel::from(
            Vec::<HomeTile>::new(),
        ))));
    let home = app.global::<HomeData>();
    home.on_activated(|what, id| {
        tracing::info!(%what, %id, "not implemented in M0");
    });

    let settings = app.global::<SettingsData>();
    settings.set_categories(ModelRc::from(Rc::new(VecModel::from(settings_categories(
        "Checking…",
    )))));
    settings.on_wifi_selected(|ssid| {
        tracing::info!(%ssid, "connecting arrives in M1");
    });
}

/// Sets the status line of one Settings category.
pub fn set_category_status(app: &AppWindow, key: &str, status: &str) {
    let categories = app.global::<SettingsData>().get_categories();
    for (i, mut category) in categories.iter().enumerate() {
        if category.key == key && category.status != status {
            category.status = status.into();
            categories.set_row_data(i, category);
        }
    }
}

/// Fetches Wi-Fi networks on a worker thread and updates the settings models.
pub fn load_network(app: &AppWindow, backend: impl NetworkBackend + 'static) {
    let weak = app.as_weak();
    let spawned = thread::Builder::new()
        .name("hearth-network".into())
        .spawn(move || {
            let networks = match backend.wifi_networks() {
                Ok(networks) => networks,
                Err(err) => {
                    tracing::warn!("listing Wi-Fi networks failed: {err}");
                    Vec::new()
                }
            };
            let _ = weak.upgrade_in_event_loop(move |app| {
                let settings = app.global::<SettingsData>();
                let rows: Vec<_> = networks.iter().map(wifi_row).collect();
                settings.set_wifi(ModelRc::from(Rc::new(VecModel::from(rows))));
                set_category_status(&app, "network", &network_status(&networks));
            });
        });
    if let Err(err) = spawned {
        tracing::warn!("spawning network thread failed: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hearth_platform::network::MockNetworkBackend;

    fn entry(id: &str, title: &str, last: Option<i64>, mins: u64) -> LibraryEntry {
        use hearth_core::model::{Game, SystemId};
        LibraryEntry {
            game: Game {
                id: GameId::new(id),
                title: title.into(),
                system: SystemId::new("n64"),
                path: "/g.z64".into(),
                folder: None,
            },
            last_played: last,
            playtime: Duration::from_secs(mins * 60),
            favorite: false,
        }
    }

    #[test]
    fn welcome_then_running_game_then_library_order_then_game_library() {
        let systems = Systems::builtin().unwrap();
        let entries = [
            entry("a", "Zelda", Some(1_000), 90),
            entry("b", "Mario", Some(500), 5),
            entry("c", "Banjo", None, 0),
        ];
        let running = GameId::new("b");
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("no-packs-here");
        let mut icons = ConsoleIcons::new(hearth_core::packs::Resolver::new(
            &dir,
            &dir,
            &hearth_core::config::Config::default(),
        ));
        let tiles = home_tiles(
            &entries,
            &systems,
            Some(&running),
            1_000 + 86_400,
            &ArtMap::new(),
            &mut icons,
        );
        let titles: Vec<_> = tiles.iter().map(|t| t.title.to_string()).collect();
        assert_eq!(
            titles,
            ["Welcome", "Mario", "Zelda", "Banjo", "Game Library"]
        );
        assert_eq!(tiles[0].kind, TileKind::Welcome);
        assert!(tiles[1].running);
        assert!(!tiles[2].running);
        assert_eq!(tiles[2].badge, "N64");
        assert_eq!(tiles[2].last_played, "last played yesterday");
        assert_eq!(tiles[2].played, "Played 1 h");
        assert_eq!(tiles[3].played, "Not played yet");
        assert_eq!(tiles[3].last_played, "");
        assert_eq!(tiles[4].kind, TileKind::Library);
        // No icon files anywhere: the monogram.
        assert_eq!(tiles[1].monogram, "N64");
        assert_eq!(tiles[1].art.size().width, 0);
    }

    #[test]
    fn time_formats() {
        assert_eq!(format_playtime(Duration::ZERO), "Not played yet");
        assert_eq!(
            format_playtime(Duration::from_secs(20)),
            "Less than a minute played"
        );
        assert_eq!(
            format_playtime(Duration::from_secs(35 * 60)),
            "35 min played"
        );
        assert_eq!(format_playtime(Duration::from_secs(150 * 60)), "2 h played");
        assert_eq!(
            format_session(Duration::from_secs(12 * 60)),
            "Playing for 12 min"
        );
        assert_eq!(
            format_session(Duration::from_secs(65 * 60)),
            "Playing for 1 h 05 min"
        );
        assert_eq!(
            format_session_total(Duration::from_secs(28)),
            "played less than a minute"
        );
        assert_eq!(
            format_session_total(Duration::from_secs(61 * 60)),
            "played 1 h 01 min"
        );
        assert_eq!(format_played(Duration::from_secs(150 * 60)), "Played 2 h");
        assert_eq!(format_played(Duration::from_secs(35 * 60)), "Played 35 min");
        assert_eq!(format_played(Duration::ZERO), "Not played yet");
        assert_eq!(greeting(4), "Good evening");
        assert_eq!(greeting(5), "Good morning");
        assert_eq!(greeting(12), "Good afternoon");
        assert_eq!(greeting(18), "Good evening");
        assert_eq!(format_size(900), "900 B");
        assert_eq!(format_size(512 * 1024), "512 KB");
        assert_eq!(format_size(1_468_006_400), "1.4 GB");
        assert_eq!(format_last_played(None, 0), None);
        assert_eq!(
            format_last_played(Some(100), 200).as_deref(),
            Some("last played today")
        );
        assert_eq!(
            format_last_played(Some(0), 3 * 86_400).as_deref(),
            Some("last played 3 days ago")
        );
    }

    #[test]
    fn network_status_names_connected_network() {
        let networks = MockNetworkBackend::new().wifi_networks().unwrap();
        assert_eq!(network_status(&networks), "Wi-Fi · Home-5G");
        assert_eq!(network_status(&[]), "Not connected");
    }
}

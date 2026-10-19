//! Builds the Slint models from core and platform data.

use std::rc::Rc;
use std::thread;

use std::time::Duration;

use hearth_core::library::LibraryEntry;
use hearth_core::model::GameId;
use hearth_core::systems::Systems;
use hearth_platform::network::{NetworkBackend, WifiNetwork};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, HomeData, HomeTile, SettingsCategory, SettingsData, TileKind, WifiRow};

/// Placeholder art gradients, picked per game from a hash of its id.
/// TODO(M4): SteamGridDB art.
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

/// Library games (running game first, then most recently played) followed by the fixed
/// Library, Media and Settings tiles.
pub fn home_tiles(
    entries: &[LibraryEntry],
    systems: &Systems,
    running: Option<&GameId>,
    now: i64,
) -> Vec<HomeTile> {
    let mut ordered: Vec<&LibraryEntry> = entries.iter().collect();
    // Stable sort keeps the library's recent-first order for everything else.
    ordered.sort_by_key(|e| Some(&e.game.id) != running);
    let games = ordered.into_iter().map(|e| {
        let (art_a, art_b) = art_for(&e.game.id);
        let badge = systems
            .get(&e.game.system)
            .map_or_else(|| e.game.system.to_string(), |s| s.short.clone());
        let mut subtitle = format_playtime(e.playtime);
        if let Some(last) = format_last_played(e.last_played, now) {
            subtitle = format!("{subtitle} · {last}");
        }
        HomeTile {
            id: e.game.id.as_str().into(),
            title: e.game.title.as_str().into(),
            badge: badge.into(),
            subtitle: subtitle.into(),
            kind: TileKind::Game,
            art_a,
            art_b,
            running: Some(&e.game.id) == running,
        }
    });
    let fixed = [
        (
            "library",
            "Game Library",
            "Every game, every console",
            TileKind::Library,
            0x4A3626,
        ),
        (
            "media",
            "Media",
            "Your movies, shows and music",
            TileKind::Media,
            0x2E3440,
        ),
        (
            "settings",
            "Settings",
            "Network, game folders, emulators, controllers",
            TileKind::Settings,
            0x36302A,
        ),
    ]
    .into_iter()
    .map(|(id, title, subtitle, kind, art)| HomeTile {
        id: id.into(),
        title: title.into(),
        badge: SharedString::new(),
        subtitle: subtitle.into(),
        kind,
        art_a: rgb(art),
        art_b: rgb(0x0C0B0A),
        running: false,
    });
    games.chain(fixed).collect()
}

/// Replaces the Home tile row.
pub fn set_tiles(app: &AppWindow, tiles: Vec<HomeTile>) {
    app.global::<HomeData>()
        .set_tiles(ModelRc::from(Rc::new(VecModel::from(tiles))));
}

/// Settings categories with fake status values (network status comes from the backend).
fn settings_categories(network_status: &str) -> Vec<SettingsCategory> {
    [
        ("network", "Network", network_status),
        ("folders", "Game folders", "2 folders"),
        ("emulators", "Emulators", "5 detected"),
        ("controllers", "Controllers", "1 connected"),
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
    set_tiles(app, home_tiles(&[], &Systems::default(), None, 0));
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
                settings.set_categories(ModelRc::from(Rc::new(VecModel::from(
                    settings_categories(&network_status(&networks)),
                ))));
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
            },
            last_played: last,
            playtime: Duration::from_secs(mins * 60),
            favorite: false,
        }
    }

    #[test]
    fn running_game_first_then_library_order_then_fixed() {
        let systems = Systems::builtin().unwrap();
        let entries = [
            entry("a", "Zelda", Some(1_000), 90),
            entry("b", "Mario", Some(500), 5),
            entry("c", "Banjo", None, 0),
        ];
        let running = GameId::new("b");
        let tiles = home_tiles(&entries, &systems, Some(&running), 1_000 + 86_400);
        let titles: Vec<_> = tiles.iter().map(|t| t.title.to_string()).collect();
        assert_eq!(
            titles,
            [
                "Mario",
                "Zelda",
                "Banjo",
                "Game Library",
                "Media",
                "Settings"
            ]
        );
        assert!(tiles[0].running);
        assert!(!tiles[1].running);
        assert_eq!(tiles[1].badge, "N64");
        assert_eq!(tiles[1].subtitle, "1 h played · last played yesterday");
        assert_eq!(tiles[2].subtitle, "Not played yet");
        assert_eq!(tiles[3].kind, TileKind::Library);
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

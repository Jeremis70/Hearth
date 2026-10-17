//! Builds the Slint models from core and platform data.

use std::rc::Rc;
use std::thread;

use hearth_core::demo::{self, Rgb};
use hearth_platform::network::{NetworkBackend, WifiNetwork};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, HomeData, HomeTile, SettingsCategory, SettingsData, TileKind, WifiRow};

fn color(Rgb(r, g, b): Rgb) -> Color {
    Color::from_rgb_u8(r, g, b)
}

/// Recent games (fake in M0) followed by the fixed Library, Media and Settings tiles.
fn home_tiles() -> Vec<HomeTile> {
    let games = demo::recent_games().into_iter().map(|g| HomeTile {
        id: g.game.id.as_str().into(),
        title: g.game.title.into(),
        badge: g.system_short.into(),
        subtitle: format!("{} · {}", g.playtime, g.last_played).into(),
        kind: TileKind::Game,
        art_a: color(g.art[0]),
        art_b: color(g.art[1]),
    });
    let fixed = [
        (
            "library",
            "Game Library",
            "Every game, every console",
            TileKind::Library,
            Rgb(0x4A, 0x36, 0x26),
        ),
        (
            "media",
            "Media",
            "Your movies, shows and music",
            TileKind::Media,
            Rgb(0x2E, 0x34, 0x40),
        ),
        (
            "settings",
            "Settings",
            "Network, game folders, emulators, controllers",
            TileKind::Settings,
            Rgb(0x36, 0x30, 0x2A),
        ),
    ]
    .into_iter()
    .map(|(id, title, subtitle, kind, art)| HomeTile {
        id: id.into(),
        title: title.into(),
        badge: SharedString::new(),
        subtitle: subtitle.into(),
        kind,
        art_a: color(art),
        art_b: Color::from_rgb_u8(0x0C, 0x0B, 0x0A),
    });
    games.chain(fixed).collect()
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
    let home = app.global::<HomeData>();
    home.set_tiles(ModelRc::from(Rc::new(VecModel::from(home_tiles()))));
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

    #[test]
    fn home_row_is_six_games_then_fixed_tiles() {
        let tiles = home_tiles();
        assert_eq!(tiles.len(), 9);
        assert!(tiles[..6].iter().all(|t| t.kind == TileKind::Game));
        let fixed: Vec<_> = tiles[6..].iter().map(|t| t.kind).collect();
        assert_eq!(
            fixed,
            [TileKind::Library, TileKind::Media, TileKind::Settings]
        );
    }

    #[test]
    fn network_status_names_connected_network() {
        let networks = MockNetworkBackend::new().wifi_networks().unwrap();
        assert_eq!(network_status(&networks), "Wi-Fi · Home-5G");
        assert_eq!(network_status(&[]), "Not connected");
    }
}

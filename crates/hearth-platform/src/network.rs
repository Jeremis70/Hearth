//! Network status and Wi-Fi, behind [`NetworkBackend`].
//!
//! Only [`MockNetworkBackend`] exists so far, so laptop testing never touches the real Wi-Fi.
//! TODO(M1): NetworkManager backend via nmrs, selected with `--real-network`; connect with
//! password.

/// Errors from a network backend.
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    /// The backend could not answer.
    #[error("network backend unavailable: {0}")]
    Unavailable(String),
}

/// One Wi-Fi network seen in a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiNetwork {
    /// Network name.
    pub ssid: String,
    /// Signal strength, 0–100.
    pub strength: u8,
    /// Whether a password is required.
    pub secured: bool,
    /// Whether this is the network we're connected to.
    pub connected: bool,
}

/// Source of network state.
pub trait NetworkBackend: Send + Sync {
    /// Visible Wi-Fi networks, connected one first, then by descending strength.
    fn wifi_networks(&self) -> Result<Vec<WifiNetwork>, NetworkError>;
}

/// Fake backend returning a fixed list of networks.
#[derive(Debug, Clone, Default)]
pub struct MockNetworkBackend;

impl MockNetworkBackend {
    /// Creates the mock.
    pub fn new() -> Self {
        Self
    }
}

impl NetworkBackend for MockNetworkBackend {
    fn wifi_networks(&self) -> Result<Vec<WifiNetwork>, NetworkError> {
        let mut networks: Vec<_> = [
            ("Café du Coin", 38, false, false),
            ("Home-5G", 92, true, true),
            ("Home-2.4G", 81, true, false),
            ("Voisin_Wifi", 55, true, false),
            ("Bell-4821", 23, true, false),
            ("Guest", 64, false, false),
        ]
        .into_iter()
        .map(|(ssid, strength, secured, connected)| WifiNetwork {
            ssid: ssid.to_owned(),
            strength,
            secured,
            connected,
        })
        .collect();
        networks.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then(b.strength.cmp(&a.strength))
        });
        Ok(networks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_lists_connected_first_then_by_strength() {
        let networks = MockNetworkBackend::new().wifi_networks().unwrap();
        assert_eq!(networks.len(), 6);
        assert!(networks[0].connected);
        assert_eq!(networks.iter().filter(|n| n.connected).count(), 1);
        assert!(
            networks[1..]
                .windows(2)
                .all(|w| w[0].strength >= w[1].strength)
        );
        assert!(networks.iter().all(|n| n.strength <= 100));
    }
}

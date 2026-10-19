//! Where artwork comes from: [`MetadataProvider`], with SteamGridDB as the real one
//! ([`super::steamgriddb`]) and [`MockMetadataProvider`] for tests and `--mock-network`.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Mutex;

/// The kinds of artwork Hearth shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ArtKind {
    /// Square tile art (Home row).
    Grid,
    /// Wide banner behind the hero zone.
    Hero,
    /// Transparent game logo, shown instead of the title.
    Logo,
    /// Small square icon.
    Icon,
}

impl ArtKind {
    /// Every kind, in fetch order.
    pub const ALL: [Self; 4] = [Self::Grid, Self::Hero, Self::Logo, Self::Icon];

    /// File name stem in the cache, and the name stored in the `art` table.
    pub fn name(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Hero => "hero",
            Self::Logo => "logo",
            Self::Icon => "icon",
        }
    }

    /// The kind with this [`name`](Self::name).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// A game in the provider's catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogGame {
    /// The provider's id (SteamGridDB game id), kept permanently once matched.
    pub id: u64,
    /// Its name there.
    pub name: String,
}

/// One downloadable image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRef {
    /// Full-size image URL.
    pub url: String,
    /// File extension for the cache (`png`, `jpg`, `webp`).
    pub extension: String,
}

/// Errors talking to a provider. Most are worth retrying later (the network was down, the
/// service failed, the key was refused and may be fixed); see [`ProviderError::is_permanent`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// No connection, DNS failure, timeout.
    #[error("network error: {0}")]
    Network(String),
    /// The service answered with an error status.
    #[error("HTTP {status} from {what}")]
    Http {
        /// Status code.
        status: u16,
        /// What was asked for.
        what: String,
    },
    /// The answer wasn't what the API documents.
    #[error("unexpected answer: {0}")]
    Invalid(String),
}

impl ProviderError {
    /// A request the service will refuse every time (HTTP 4xx other than 401, 403, 408 and
    /// 429): retrying it can't help.
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::Http { status, .. }
            if (400..500).contains(status) && ![401, 403, 408, 429].contains(status))
    }
}

/// A source of game artwork. Blocking: called from the art worker thread only.
pub trait MetadataProvider: Send {
    /// Games whose name looks like `name`, most relevant first.
    fn search(&self, name: &str) -> Result<Vec<CatalogGame>, ProviderError>;

    /// The best static, safe image of `kind` for the game `id` (top community score; for
    /// grids, square or closest to square), or `None` if there is none.
    fn best_asset(&self, id: u64, kind: ArtKind) -> Result<Option<AssetRef>, ProviderError>;

    /// Downloads an image.
    fn download(&self, asset: &AssetRef) -> Result<Vec<u8>, ProviderError>;
}

impl<T: MetadataProvider + Sync> MetadataProvider for std::sync::Arc<T> {
    fn search(&self, name: &str) -> Result<Vec<CatalogGame>, ProviderError> {
        (**self).search(name)
    }

    fn best_asset(&self, id: u64, kind: ArtKind) -> Result<Option<AssetRef>, ProviderError> {
        (**self).best_asset(id, kind)
    }

    fn download(&self, asset: &AssetRef) -> Result<Vec<u8>, ProviderError> {
        (**self).download(asset)
    }
}

/// In-memory provider: never touches the network.
///
/// [`MockMetadataProvider::synthetic`] matches every name to itself and draws a plain
/// gradient for each kind of art (for `--mock-network`). [`MockMetadataProvider::new`] knows
/// only the games added with [`add_game`](Self::add_game), and
/// [`offline`](Self::offline) fails every call like a machine without network.
#[derive(Debug, Default)]
pub struct MockMetadataProvider {
    synthetic: bool,
    offline: bool,
    games: Vec<(CatalogGame, Vec<ArtKind>)>,
    calls: Mutex<HashMap<&'static str, usize>>,
}

impl MockMetadataProvider {
    /// A provider with an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches every searched name, with every kind of art.
    pub fn synthetic() -> Self {
        Self {
            synthetic: true,
            ..Self::default()
        }
    }

    /// Fails every call with a network error.
    pub fn offline() -> Self {
        Self {
            offline: true,
            ..Self::default()
        }
    }

    /// Adds a catalog game with the given kinds of art.
    pub fn add_game(mut self, id: u64, name: &str, kinds: &[ArtKind]) -> Self {
        self.games.push((
            CatalogGame {
                id,
                name: name.to_owned(),
            },
            kinds.to_vec(),
        ));
        self
    }

    /// How many times `method` ("search", "best_asset", "download") was called.
    pub fn calls(&self, method: &str) -> usize {
        self.calls
            .lock()
            .map(|c| c.get(method).copied().unwrap_or(0))
            .unwrap_or(0)
    }

    fn count(&self, method: &'static str) -> Result<(), ProviderError> {
        if let Ok(mut calls) = self.calls.lock() {
            *calls.entry(method).or_default() += 1;
        }
        if self.offline {
            Err(ProviderError::Network("mock: offline".into()))
        } else {
            Ok(())
        }
    }
}

impl MetadataProvider for MockMetadataProvider {
    fn search(&self, name: &str) -> Result<Vec<CatalogGame>, ProviderError> {
        self.count("search")?;
        if self.synthetic {
            return Ok(vec![CatalogGame {
                id: name
                    .bytes()
                    .fold(7u64, |h, b| h.wrapping_mul(31) + u64::from(b)),
                name: name.to_owned(),
            }]);
        }
        let words = super::identity::tokens(name);
        Ok(self
            .games
            .iter()
            .filter(|(g, _)| {
                let theirs = super::identity::tokens(&g.name);
                words.iter().any(|w| theirs.contains(w))
            })
            .map(|(g, _)| g.clone())
            .collect())
    }

    fn best_asset(&self, id: u64, kind: ArtKind) -> Result<Option<AssetRef>, ProviderError> {
        self.count("best_asset")?;
        let known = self.synthetic
            || self
                .games
                .iter()
                .any(|(g, kinds)| g.id == id && kinds.contains(&kind));
        Ok(known.then(|| AssetRef {
            url: format!("mock://{id}/{}", kind.name()),
            extension: "png".into(),
        }))
    }

    fn download(&self, asset: &AssetRef) -> Result<Vec<u8>, ProviderError> {
        self.count("download")?;
        let (id, kind) = asset
            .url
            .strip_prefix("mock://")
            .and_then(|rest| rest.split_once('/'))
            .ok_or_else(|| ProviderError::Invalid(asset.url.clone()))?;
        let seed = id
            .bytes()
            .fold(0u8, |h, b| h.wrapping_mul(13).wrapping_add(b));
        let (w, h) = match ArtKind::from_name(kind) {
            Some(ArtKind::Hero) => (192, 62),
            Some(ArtKind::Logo) => (96, 36),
            _ => (64, 64),
        };
        let image = image::RgbaImage::from_fn(w, h, |x, y| {
            let t = u8::try_from(x * 255 / w).unwrap_or(255);
            let alpha = if kind == "logo" && (y < 6 || y > h - 6) {
                0
            } else {
                255
            };
            image::Rgba([seed, t, 255 - seed / 2, alpha])
        });
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|err| ProviderError::Invalid(err.to_string()))?;
        Ok(png)
    }
}

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

/// One image a provider offers for a game, with a small preview of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The provider's id for the image (SteamGridDB asset id).
    pub id: u64,
    /// The full-size image.
    pub full: AssetRef,
    /// A small preview.
    pub thumb: AssetRef,
    /// Full size in pixels (0 when unknown).
    pub width: u32,
    /// Full size in pixels (0 when unknown).
    pub height: u32,
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

    /// Every static, safe image of `kind` for the game `id`, the best first (for the user to
    /// choose from).
    fn assets(&self, id: u64, kind: ArtKind) -> Result<Vec<Asset>, ProviderError>;

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

    fn assets(&self, id: u64, kind: ArtKind) -> Result<Vec<Asset>, ProviderError> {
        (**self).assets(id, kind)
    }

    fn download(&self, asset: &AssetRef) -> Result<Vec<u8>, ProviderError> {
        (**self).download(asset)
    }
}

/// How many images [`MockMetadataProvider::fixtures`] offers per kind.
pub const FIXTURE_ASSETS: u64 = 6;

/// In-memory provider: never touches the network.
///
/// [`MockMetadataProvider::fixtures`] matches every name to itself, offers a few look-alike
/// games beside it ("<name> II", "Super <name>"…) and [`FIXTURE_ASSETS`] images of each kind,
/// drawn as numbered gradients (for `--mock-sgdb=fixtures`, only with a throwaway
/// `HEARTH_HOME`). [`MockMetadataProvider::new`] knows only the games added with
/// [`add_game`](Self::add_game), and [`offline`](Self::offline) fails every call like a
/// machine without network (`--mock-network`).
#[derive(Debug, Default)]
pub struct MockMetadataProvider {
    fixtures: bool,
    offline: bool,
    games: Vec<(CatalogGame, Vec<ArtKind>)>,
    calls: Mutex<HashMap<&'static str, usize>>,
}

impl MockMetadataProvider {
    /// A provider with an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches every searched name (plus look-alikes), with several images of every kind.
    pub fn fixtures() -> Self {
        Self {
            fixtures: true,
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

    /// How many times `method` ("search", "best_asset", "assets", "download") was called.
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

    fn has(&self, id: u64, kind: ArtKind) -> bool {
        self.fixtures
            || self
                .games
                .iter()
                .any(|(g, kinds)| g.id == id && kinds.contains(&kind))
    }

    fn asset_ref(id: u64, kind: ArtKind, n: u64, thumb: bool) -> AssetRef {
        let scheme = if thumb { "mock-thumb" } else { "mock" };
        AssetRef {
            url: format!("{scheme}://{id}/{}/{n}", kind.name()),
            extension: "png".into(),
        }
    }
}

/// A stable id for a fixture game's name.
fn fixture_id(name: &str) -> u64 {
    name.bytes()
        .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)))
        % 1_000_000_000
}

impl MetadataProvider for MockMetadataProvider {
    fn search(&self, name: &str) -> Result<Vec<CatalogGame>, ProviderError> {
        self.count("search")?;
        if self.fixtures {
            let names = [
                name.to_owned(),
                format!("{name} II"),
                format!("Super {name}"),
                format!("{name}: Remastered"),
            ];
            return Ok(names
                .into_iter()
                .map(|name| CatalogGame {
                    id: fixture_id(&name),
                    name,
                })
                .collect());
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
        Ok(self
            .has(id, kind)
            .then(|| Self::asset_ref(id, kind, 0, false)))
    }

    fn assets(&self, id: u64, kind: ArtKind) -> Result<Vec<Asset>, ProviderError> {
        self.count("assets")?;
        if !self.has(id, kind) {
            return Ok(Vec::new());
        }
        let count = if self.fixtures { FIXTURE_ASSETS } else { 1 };
        Ok((0..count)
            .map(|n| {
                let (width, height) = mock_size(kind, false);
                Asset {
                    id: id * 100 + n,
                    full: Self::asset_ref(id, kind, n, false),
                    thumb: Self::asset_ref(id, kind, n, true),
                    width,
                    height,
                }
            })
            .collect())
    }

    fn download(&self, asset: &AssetRef) -> Result<Vec<u8>, ProviderError> {
        self.count("download")?;
        let invalid = || ProviderError::Invalid(asset.url.clone());
        let (scheme, rest) = asset.url.split_once("://").ok_or_else(invalid)?;
        let mut parts = rest.split('/');
        let (id, kind) = (
            parts.next().ok_or_else(invalid)?,
            parts.next().ok_or_else(invalid)?,
        );
        let n: u32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let kind = ArtKind::from_name(kind).ok_or_else(invalid)?;
        let seed = id
            .bytes()
            .fold(0u8, |h, b| h.wrapping_mul(13).wrapping_add(b))
            .wrapping_add(u8::try_from(n * 40 % 256).unwrap_or(0));
        let (w, h) = mock_size(kind, scheme == "mock-thumb");
        // A gradient with n + 1 bright bars: each image looks different, and a hero's bars
        // show where it is cropped.
        let bars = n + 1;
        let image = image::RgbaImage::from_fn(w, h, |x, y| {
            let t = u8::try_from(x * 255 / w).unwrap_or(255);
            let alpha = if kind == ArtKind::Logo && (y < h / 6 || y > h - h / 6) {
                0
            } else {
                255
            };
            let bar = (x * (2 * bars + 1) / w) % 2 == 1 && y > h / 3 && y < h * 2 / 3;
            if bar {
                image::Rgba([255, 255, 255, alpha])
            } else {
                image::Rgba([seed, t, 255 - seed / 2, alpha])
            }
        });
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|err| ProviderError::Invalid(err.to_string()))?;
        Ok(png)
    }
}

/// Mock image sizes: heroes are as wide as SteamGridDB's (about 3.1:1).
fn mock_size(kind: ArtKind, thumb: bool) -> (u32, u32) {
    let (w, h) = match kind {
        ArtKind::Hero => (1920, 620),
        ArtKind::Logo => (640, 240),
        ArtKind::Grid => (512, 512),
        ArtKind::Icon => (128, 128),
    };
    if thumb { (w / 4, h / 4) } else { (w, h) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_offer_look_alikes_and_several_images() {
        let mock = MockMetadataProvider::fixtures();
        let found = mock.search("Twilight Princess").unwrap();
        assert_eq!(found[0].name, "Twilight Princess");
        assert!(found.len() > 2);
        assert_ne!(found[0].id, found[1].id);
        let heroes = mock.assets(found[1].id, ArtKind::Hero).unwrap();
        assert_eq!(heroes.len() as u64, FIXTURE_ASSETS);
        let full = image::load_from_memory(&mock.download(&heroes[2].full).unwrap()).unwrap();
        let thumb = image::load_from_memory(&mock.download(&heroes[2].thumb).unwrap()).unwrap();
        assert!(full.width() > 3 * full.height());
        assert!(thumb.width() < full.width());
        assert_ne!(
            mock.download(&heroes[0].full).unwrap(),
            mock.download(&heroes[1].full).unwrap()
        );
    }

    #[test]
    fn the_plain_mock_has_one_image_per_kind_it_knows() {
        let mock = MockMetadataProvider::new().add_game(3, "Zelda", &[ArtKind::Hero]);
        assert_eq!(mock.assets(3, ArtKind::Hero).unwrap().len(), 1);
        assert!(mock.assets(3, ArtKind::Logo).unwrap().is_empty());
        assert!(
            MockMetadataProvider::offline()
                .assets(3, ArtKind::Hero)
                .is_err()
        );
    }
}

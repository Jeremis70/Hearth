//! The art cache on disk, local override files, and the pre-blurred hero.
//!
//! Layout: `<cache>/art/<art key>/{grid,hero,logo,icon}.<ext>` plus `hero-blur.png`
//! (the blurred, downscaled hero behind Home's widgets: Slint has no backdrop blur).
//!
//! The game's own art (`crate::extras`: `grid`, `hero`, `logo`, `icon` in its game folder)
//! always wins over the cache. A hero of its own gets its own blurred copy
//! (`local-hero-blur.png`), remade when the hero is newer.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::identity::ArtKey;
use super::provider::ArtKind;

/// Extensions a cached image may have.
const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
/// Width of the blurred hero: it is shown stretched and blurry, so small is fine.
const BLUR_WIDTH: u32 = 480;
/// Gaussian sigma at [`BLUR_WIDTH`].
const BLUR_SIGMA: f32 = 10.0;

/// Paths of a game's art, wherever each came from. `None` means "show the placeholder".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameArt {
    /// Square tile art.
    pub grid: Option<PathBuf>,
    /// Sharp hero banner.
    pub hero: Option<PathBuf>,
    /// Blurred, downscaled hero.
    pub hero_blur: Option<PathBuf>,
    /// Game logo.
    pub logo: Option<PathBuf>,
    /// Small icon.
    pub icon: Option<PathBuf>,
}

impl GameArt {
    /// The path for `kind`.
    pub fn get(&self, kind: ArtKind) -> Option<&Path> {
        match kind {
            ArtKind::Grid => self.grid.as_deref(),
            ArtKind::Hero => self.hero.as_deref(),
            ArtKind::Logo => self.logo.as_deref(),
            ArtKind::Icon => self.icon.as_deref(),
        }
    }

    /// Sets the path of one kind.
    pub fn set(&mut self, kind: ArtKind, path: PathBuf) {
        let slot = match kind {
            ArtKind::Grid => &mut self.grid,
            ArtKind::Hero => &mut self.hero,
            ArtKind::Logo => &mut self.logo,
            ArtKind::Icon => &mut self.icon,
        };
        *slot = Some(path);
    }

    /// True if nothing is set.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The art cache folder (`<cache>/art`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtCache {
    root: PathBuf,
}

impl ArtCache {
    /// A cache rooted at `root`. Nothing is created until something is written.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The folder of one game.
    pub fn dir(&self, key: &ArtKey) -> PathBuf {
        self.root.join(key.as_str())
    }

    /// The cached image of `kind`, if any.
    pub fn cached(&self, key: &ArtKey, kind: ArtKind) -> Option<PathBuf> {
        let dir = self.dir(key);
        EXTENSIONS
            .iter()
            .map(|ext| dir.join(format!("{}.{ext}", kind.name())))
            .find(|path| path.is_file())
    }

    /// The blurred hero made from the cached hero.
    pub fn blurred_hero(&self, key: &ArtKey) -> PathBuf {
        self.dir(key).join("hero-blur.png")
    }

    fn local_blurred_hero(&self, key: &ArtKey) -> PathBuf {
        self.dir(key).join("local-hero-blur.png")
    }

    /// Stores a downloaded image, replacing any older one of the same kind. Written to a
    /// temporary file first, so a crash never leaves half an image under the real name.
    pub fn store(
        &self,
        key: &ArtKey,
        kind: ArtKind,
        extension: &str,
        bytes: &[u8],
    ) -> io::Result<PathBuf> {
        let dir = self.dir(key);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.{extension}", kind.name()));
        let tmp = dir.join(format!(".{}.{extension}.part", kind.name()));
        fs::write(&tmp, bytes)?;
        for ext in EXTENSIONS.iter().filter(|e| **e != extension) {
            let _ = fs::remove_file(dir.join(format!("{}.{ext}", kind.name())));
        }
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Notes where the cached `kind` was downloaded from.
    pub fn set_source_url(&self, key: &ArtKey, kind: ArtKind, url: &str) {
        let path = self.dir(key).join(format!("{}.url", kind.name()));
        if let Err(err) = fs::write(&path, url) {
            tracing::debug!(path = %path.display(), "can't note the art's source: {err}");
        }
    }

    /// Where the cached `kind` was downloaded from, if noted.
    pub fn source_url(&self, key: &ArtKey, kind: ArtKind) -> Option<String> {
        fs::read_to_string(self.dir(key).join(format!("{}.url", kind.name()))).ok()
    }

    /// Where the Customize page's candidates and their thumbnails are cached.
    pub fn candidates_dir(&self, key: &ArtKey) -> PathBuf {
        self.dir(key).join("candidates")
    }

    /// Deletes one game's cached art. Only folders inside the cache are touched.
    pub fn remove(&self, key: &ArtKey) -> io::Result<()> {
        let safe = key.as_str().split('/').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
        if !safe || key.as_str().matches('/').count() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing to delete art for odd key {key}"),
            ));
        }
        match fs::remove_dir_all(self.dir(key)) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// The art to show: the game's own (`local`, from `crate::extras::Extras::art`) first,
    /// then the cache. Makes the blurred hero if it is missing or older than its hero (slow:
    /// call off the UI thread).
    pub fn resolve(&self, key: &ArtKey, local: &GameArt) -> GameArt {
        let mut art = GameArt::default();
        for kind in ArtKind::ALL {
            let path = local
                .get(kind)
                .map(Path::to_path_buf)
                .or_else(|| self.cached(key, kind));
            if let Some(path) = path {
                art.set(kind, path);
            }
        }
        if let Some(hero) = &art.hero {
            let blurred = if local.hero.is_some() {
                self.local_blurred_hero(key)
            } else {
                self.blurred_hero(key)
            };
            if is_stale(&blurred, hero)
                && let Err(err) = make_blurred(hero, &blurred)
            {
                tracing::warn!(hero = %hero.display(), "can't blur the hero: {err}");
            }
            art.hero_blur = blurred.is_file().then_some(blurred);
        }
        art
    }
}

/// True if `derived` is missing or older than `source`.
pub(super) fn is_stale(derived: &Path, source: &Path) -> bool {
    let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    match (modified(derived), modified(source)) {
        (Some(d), Some(s)) => d < s,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

/// Writes a downscaled, blurred copy of `source` to `target` (PNG).
pub fn make_blurred(source: &Path, target: &Path) -> Result<(), image::ImageError> {
    let image = super::animation::still(source, super::provider::ArtKind::Hero)?;
    let height = (u64::from(image.height()) * u64::from(BLUR_WIDTH)
        / u64::from(image.width().max(1)))
    .clamp(1, u64::from(BLUR_WIDTH));
    let small = image.resize_exact(
        BLUR_WIDTH,
        u32::try_from(height).unwrap_or(BLUR_WIDTH),
        image::imageops::FilterType::Triangle,
    );
    let blurred = image::imageops::blur(&small.to_rgba8(), BLUR_SIGMA);
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = target.with_extension("png.part");
    blurred.save_with_format(&tmp, image::ImageFormat::Png)?;
    fs::rename(&tmp, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SystemId;
    use crate::testdir::tempdir;

    fn png(path: &Path, w: u32, h: u32) {
        image::RgbaImage::from_pixel(w, h, image::Rgba([200, 100, 50, 255]))
            .save(path)
            .unwrap();
    }

    fn key() -> ArtKey {
        ArtKey::new(&SystemId::new("gc"), Some("GZ2E01"), "")
    }

    #[test]
    fn cache_paths_follow_the_key() {
        let cache = ArtCache::new("/c/art");
        assert_eq!(cache.dir(&key()), Path::new("/c/art/gc/GZ2E01"));
        assert_eq!(
            cache.blurred_hero(&key()),
            Path::new("/c/art/gc/GZ2E01/hero-blur.png")
        );
        let unnamed = ArtKey::new(&SystemId::new("n64"), None, "Pikmin");
        assert!(cache.dir(&unnamed).starts_with("/c/art/n64"));
    }

    #[test]
    fn store_replaces_other_extensions_and_resolve_blurs_the_hero() {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(64, 20, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        cache.store(&key(), ArtKind::Hero, "jpg", b"old").unwrap();
        let hero = cache.store(&key(), ArtKind::Hero, "png", &bytes).unwrap();
        assert_eq!(cache.cached(&key(), ArtKind::Hero), Some(hero.clone()));
        assert!(!cache.dir(&key()).join("hero.jpg").exists());

        let art = cache.resolve(&key(), &GameArt::default());
        assert_eq!(art.hero, Some(hero));
        assert_eq!(art.hero_blur, Some(cache.blurred_hero(&key())));
        let blurred = image::open(cache.blurred_hero(&key())).unwrap();
        assert_eq!((blurred.width(), blurred.height()), (BLUR_WIDTH, 150));
        assert_eq!(art.grid, None);
    }

    #[test]
    fn the_games_own_art_wins_over_the_cache() {
        let dir = tempdir();
        let folder = dir.path().join("gc/Twilight Princess");
        fs::create_dir_all(&folder).unwrap();
        png(&folder.join("grid.png"), 8, 8);
        png(&folder.join("hero.png"), 32, 10);
        let local = GameArt {
            grid: Some(folder.join("grid.png")),
            hero: Some(folder.join("hero.png")),
            ..GameArt::default()
        };

        let cache = ArtCache::new(dir.path().join("art"));
        cache
            .store(&key(), ArtKind::Grid, "png", b"cached")
            .unwrap();
        cache
            .store(&key(), ArtKind::Logo, "png", b"cached")
            .unwrap();

        let art = cache.resolve(&key(), &local);
        assert_eq!(art.grid, Some(folder.join("grid.png")));
        assert_eq!(art.logo, cache.cached(&key(), ArtKind::Logo));
        assert_eq!(
            art.hero_blur,
            Some(cache.dir(&key()).join("local-hero-blur.png"))
        );
        // Nothing is written into the game folder.
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 2);
    }

    #[test]
    fn remove_deletes_only_one_game_folder() {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let other = ArtKey::new(&SystemId::new("gc"), Some("GALE01"), "");
        cache.store(&key(), ArtKind::Grid, "png", b"x").unwrap();
        cache.store(&other, ArtKind::Grid, "png", b"x").unwrap();
        cache.remove(&key()).unwrap();
        assert!(!cache.dir(&key()).exists());
        assert!(cache.dir(&other).exists());
        cache.remove(&key()).unwrap();
        assert!(cache.remove(&ArtKey::from_stored("gc/../..")).is_err());
        assert!(cache.remove(&ArtKey::from_stored("gc")).is_err());
    }
}

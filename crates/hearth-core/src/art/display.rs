//! Display-sized copies of cached art, so Home decodes small files instead of 4K originals.
//!
//! Copies live in `<cache>/art/<art key>/sized/<class>/<kind>.qoi` (`local-<kind>.qoi` when the
//! source is a local file next to the ROM). QOI is lossless, keeps alpha and decodes several
//! times faster than PNG. The originals stay as they are (for "Change artwork" and other
//! screen sizes). A copy is remade when it is missing or older than its source; making one
//! for a class removes the game's copies for other classes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::cache::ArtCache;
use super::identity::ArtKey;
use super::provider::ArtKind;

/// Screen heights the copies are made for. A screen uses the smallest one at least as tall as
/// itself (taller screens use the largest).
const CLASSES: [u32; 4] = [720, 1080, 1440, 2160];

/// The screen size a set of copies is made for, by screen height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayClass(u32);

impl Default for DisplayClass {
    fn default() -> Self {
        Self(1080)
    }
}

impl DisplayClass {
    /// The class for a screen (or window) `height` pixels tall.
    pub fn for_height(height: u32) -> Self {
        Self(
            CLASSES
                .into_iter()
                .find(|&class| height <= class)
                .unwrap_or(CLASSES[CLASSES.len() - 1]),
        )
    }

    /// The class named `name` ("1080p"), if it is one.
    pub fn from_name(name: &str) -> Option<Self> {
        let height: u32 = name.trim().strip_suffix('p')?.parse().ok()?;
        CLASSES.contains(&height).then_some(Self(height))
    }

    /// "1080p": the folder name, and how the class is remembered.
    pub fn name(self) -> String {
        format!("{}p", self.0)
    }

    /// The box a copy of `kind` must fit in (aspect kept, never enlarged):
    /// - hero: as tall as the screen, any width (heroes are about 3:1, wider than any screen,
    ///   so cover-fit crops the sides and never needs more height);
    /// - logo: twice Home's 640 × 260 logo box at 1080p, scaled with the class;
    /// - grid: twice the focused tile (192), scaled with the class;
    /// - icon: same as the grid.
    pub fn bounds(self, kind: ArtKind) -> (u32, u32) {
        let scale = |px: u32| px * self.0 / 1080;
        match kind {
            ArtKind::Hero => (u32::MAX, self.0),
            ArtKind::Logo => (scale(1280), scale(520)),
            ArtKind::Grid | ArtKind::Icon => (scale(384), scale(384)),
        }
    }

    /// The box an animation's frames must fit in. Like [`Self::bounds`], but logos at 1× Home's
    /// box: every frame is kept decoded, so the 2× sharpness of the still isn't worth 4× the
    /// memory.
    pub fn anim_bounds(self, kind: ArtKind) -> (u32, u32) {
        match kind {
            ArtKind::Logo => (640 * self.0 / 1080, 260 * self.0 / 1080),
            _ => self.bounds(kind),
        }
    }
}

/// The size `width` × `height` shrinks to inside `bounds`, aspect kept, never enlarged.
pub(super) fn fit(width: u32, height: u32, (max_w, max_h): (u32, u32)) -> (u32, u32) {
    let ratio = (f64::from(max_w) / f64::from(width.max(1)))
        .min(f64::from(max_h) / f64::from(height.max(1)));
    if ratio >= 1.0 {
        return (width, height);
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let scaled = |v: u32| ((f64::from(v) * ratio).round() as u32).max(1);
    (scaled(width), scaled(height))
}

impl ArtCache {
    /// Where the display copy of `source` (the `kind` image of `key`) goes for `class`.
    pub fn sized_path(
        &self,
        key: &ArtKey,
        kind: ArtKind,
        source: &Path,
        class: DisplayClass,
    ) -> PathBuf {
        let dir = self.dir(key);
        let prefix = if source.starts_with(&dir) {
            ""
        } else {
            "local-"
        };
        dir.join("sized")
            .join(class.name())
            .join(format!("{prefix}{}.qoi", kind.name()))
    }

    /// The display copy of `source` for `class`, made first if it is missing or older than
    /// `source` (slow: call off the UI thread).
    pub fn sized(
        &self,
        key: &ArtKey,
        kind: ArtKind,
        source: &Path,
        class: DisplayClass,
    ) -> Result<PathBuf, image::ImageError> {
        let target = self.sized_path(key, kind, source, class);
        if !super::cache::is_stale(&target, source) {
            return Ok(target);
        }
        // An animated source's still: see `animation::still`.
        let image = super::animation::still(source, kind)?;
        let (w, h) = fit(image.width(), image.height(), class.bounds(kind));
        let image = if (w, h) == (image.width(), image.height()) {
            image
        } else {
            image.resize_exact(w, h, image::imageops::FilterType::Triangle)
        };
        let Some(class_dir) = target.parent() else {
            return Ok(target);
        };
        fs::create_dir_all(class_dir)?;
        self.remove_other_classes(key, class)?;
        let tmp = class_dir.join(format!(
            ".{}.part",
            target.file_name().unwrap_or_default().to_string_lossy()
        ));
        image
            .to_rgba8()
            .save_with_format(&tmp, image::ImageFormat::Qoi)?;
        fs::rename(&tmp, &target)?;
        Ok(target)
    }

    /// Deletes `key`'s display copies made for any class but `keep`.
    pub(super) fn remove_other_classes(&self, key: &ArtKey, keep: DisplayClass) -> io::Result<()> {
        let sized = self.dir(key).join("sized");
        for entry in fs::read_dir(&sized)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_str().and_then(DisplayClass::from_name) != Some(keep) {
                fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SystemId;
    use crate::testdir::tempdir;

    fn key() -> ArtKey {
        ArtKey::new(&SystemId::new("gc"), Some("GZ2E01"), "")
    }

    fn store_png(cache: &ArtCache, kind: ArtKind, w: u32, h: u32) -> PathBuf {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 128]))
            .write_to(&mut io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        cache.store(&key(), kind, "png", &bytes).unwrap()
    }

    fn size(path: &Path) -> (u32, u32) {
        let image = image::open(path).unwrap();
        (image.width(), image.height())
    }

    #[test]
    fn classes_follow_the_screen_height() {
        assert_eq!(DisplayClass::for_height(720).name(), "720p");
        assert_eq!(DisplayClass::for_height(1080).name(), "1080p");
        assert_eq!(DisplayClass::for_height(1200).name(), "1440p");
        assert_eq!(DisplayClass::for_height(4320).name(), "2160p");
        assert_eq!(DisplayClass::from_name("1440p"), Some(DisplayClass(1440)));
        assert_eq!(DisplayClass::from_name("1000p"), None);
        assert_eq!(DisplayClass::from_name("junk"), None);
    }

    #[test]
    fn copies_fit_their_kind_and_are_never_enlarged() {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let class = DisplayClass::for_height(1080);
        let hero = store_png(&cache, ArtKind::Hero, 3840, 1240);
        let logo = store_png(&cache, ArtKind::Logo, 2000, 500);
        let grid = store_png(&cache, ArtKind::Grid, 100, 100);

        let sized = cache.sized(&key(), ArtKind::Hero, &hero, class).unwrap();
        assert!(sized.ends_with("gc/GZ2E01/sized/1080p/hero.qoi"));
        assert_eq!(size(&sized), (3345, 1080));
        let sized = cache.sized(&key(), ArtKind::Logo, &logo, class).unwrap();
        assert_eq!(size(&sized), (1280, 320));
        let sized = cache.sized(&key(), ArtKind::Grid, &grid, class).unwrap();
        assert_eq!(size(&sized), (100, 100));
        // Alpha survives.
        assert_eq!(
            image::open(&sized).unwrap().to_rgba8().get_pixel(0, 0)[3],
            128
        );

        let small = cache
            .sized(&key(), ArtKind::Logo, &logo, DisplayClass::for_height(720))
            .unwrap();
        assert_eq!(size(&small), (853, 213));
    }

    #[test]
    fn copies_are_reused_remade_when_stale_and_one_class_is_kept() {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let class = DisplayClass::default();
        let grid = store_png(&cache, ArtKind::Grid, 800, 800);
        let first = cache.sized(&key(), ArtKind::Grid, &grid, class).unwrap();
        let made = fs::metadata(&first).unwrap().modified().unwrap();
        cache.sized(&key(), ArtKind::Grid, &grid, class).unwrap();
        assert_eq!(fs::metadata(&first).unwrap().modified().unwrap(), made);

        // A newer original (Change artwork, a re-download) makes a new copy.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let grid = store_png(&cache, ArtKind::Grid, 600, 300);
        cache.sized(&key(), ArtKind::Grid, &grid, class).unwrap();
        assert_eq!(size(&first), (384, 192));

        // Another screen size replaces this class's copies.
        let tall = DisplayClass::for_height(2160);
        let other = cache.sized(&key(), ArtKind::Grid, &grid, tall).unwrap();
        assert_eq!(size(&other), (600, 300));
        assert!(!first.exists());
        assert!(grid.exists(), "originals stay");
    }

    #[test]
    fn local_sources_get_their_own_copy() {
        let dir = tempdir();
        let cache = ArtCache::new(dir.path().join("art"));
        let local = dir.path().join("roms/tp-hero.png");
        assert!(
            cache
                .sized_path(&key(), ArtKind::Hero, &local, DisplayClass::default())
                .ends_with("sized/1080p/local-hero.qoi")
        );
    }
}

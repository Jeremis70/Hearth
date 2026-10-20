//! Game art and console icons for the UI.
//!
//! The art worker ([`hearth_core::art::ArtService`]) reports file paths; they are decoded and
//! downscaled here on that same worker thread into `SharedPixelBuffer`s (which are `Send`),
//! then turned into `slint::Image`s on the UI thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hearth_core::art::{ArtCache, ArtEvent, ArtService, GameArt, MetadataProvider};
use hearth_core::config::PackCategory;
use hearth_core::model::GameId;
use hearth_core::packs::{Origin, Resolver};
use hearth_core::systems::Systems;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use super::session::defer;

/// Largest decoded sizes (pixels) per kind: about twice what Home shows at 1080p.
const GRID_MAX: u32 = 384;
const HERO_MAX: u32 = 1920;
const LOGO_MAX: u32 = 900;

/// One game's art, decoded, ready to cross to the UI thread.
#[derive(Debug, Default)]
pub struct DecodedArt {
    grid: Option<SharedPixelBuffer<Rgba8Pixel>>,
    hero: Option<SharedPixelBuffer<Rgba8Pixel>>,
    hero_blur: Option<SharedPixelBuffer<Rgba8Pixel>>,
    logo: Option<SharedPixelBuffer<Rgba8Pixel>>,
    match_name: Option<String>,
}

/// One game's art on the UI thread. Missing images are empty (placeholders are drawn).
#[derive(Debug, Clone, Default)]
pub struct TileArt {
    pub grid: Image,
    pub hero: Image,
    pub hero_blur: Image,
    pub logo: Image,
    /// The SteamGridDB game it matched.
    pub match_name: Option<String>,
}

impl From<DecodedArt> for TileArt {
    fn from(art: DecodedArt) -> Self {
        let image =
            |b: Option<SharedPixelBuffer<Rgba8Pixel>>| b.map(Image::from_rgba8).unwrap_or_default();
        Self {
            grid: image(art.grid),
            hero: image(art.hero),
            hero_blur: image(art.hero_blur),
            logo: image(art.logo),
            match_name: art.match_name,
        }
    }
}

/// Decodes `path`, shrunk to fit `max` × `max` if larger.
fn decode(path: Option<&Path>, max: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let path = path?;
    let image = match image::ImageReader::open(path).and_then(|r| r.with_guessed_format()) {
        Ok(reader) => reader.decode(),
        Err(err) => Err(image::ImageError::IoError(err)),
    };
    let image = match image {
        Ok(image) => image,
        Err(err) => {
            tracing::warn!(path = %path.display(), "can't decode art: {err}");
            return None;
        }
    };
    let image = if image.width() > max || image.height() > max {
        image.resize(max, max, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    ))
}

fn decode_all(art: &GameArt, match_name: Option<String>) -> DecodedArt {
    DecodedArt {
        grid: decode(art.grid.as_deref(), GRID_MAX),
        hero: decode(art.hero.as_deref(), HERO_MAX),
        hero_blur: decode(art.hero_blur.as_deref(), HERO_MAX),
        logo: decode(art.logo.as_deref(), LOGO_MAX),
        match_name,
    }
}

/// Starts the art worker; its results reach the session controller.
pub fn start(
    cache: ArtCache,
    library_db: PathBuf,
    provider: Option<Box<dyn MetadataProvider>>,
) -> Option<ArtService> {
    let spawned = ArtService::spawn(cache, library_db, provider, |event| match event {
        ArtEvent::Art {
            game,
            art,
            match_name,
        } => {
            super::startup::note("first art event");
            let decoded = decode_all(&art, match_name);
            defer(move |c| c.set_art(game, decoded));
        }
    });
    match spawned {
        Ok(service) => Some(service),
        Err(err) => {
            tracing::error!("cannot start the art worker: {err}");
            None
        }
    }
}

/// A console's icon, ready for a tile.
#[derive(Debug, Clone, Default)]
pub struct ConsoleIcon {
    /// The icon, or empty: show `monogram` instead.
    pub image: Image,
    /// Hearth's own glyphs are monochrome and get tinted; pack icons are drawn as they are.
    pub tint: bool,
    /// Short text for the monogram fallback ("N64", "GC").
    pub monogram: String,
}

/// Console icons from the selected pack, falling back to Hearth's glyphs, then a monogram.
#[derive(Debug)]
pub struct ConsoleIcons {
    resolver: Resolver,
    loaded: HashMap<String, ConsoleIcon>,
}

impl ConsoleIcons {
    /// Icons resolved by `resolver`.
    pub fn new(resolver: Resolver) -> Self {
        Self {
            resolver,
            loaded: HashMap::new(),
        }
    }

    /// Switches the pack for a category; console icons reload on next use.
    pub fn select(&mut self, category: PackCategory, pack: Option<&str>) {
        self.resolver.select(category, pack);
        if category == PackCategory::ConsoleIcons {
            self.loaded.clear();
        }
    }

    /// The selected pack of a category.
    pub fn selected(&self, category: PackCategory) -> Option<&str> {
        self.resolver.selected(category)
    }

    /// The icon of `system` (loaded once; SVGs are tiny).
    pub fn icon(&mut self, system: &str, systems: &Systems) -> ConsoleIcon {
        if let Some(icon) = self.loaded.get(system) {
            return icon.clone();
        }
        let monogram = monogram(system, systems);
        let icon = match self.resolver.console_icon(system) {
            Some(found) => match Image::load_from_path(&found.path) {
                Ok(image) => ConsoleIcon {
                    image,
                    tint: found.origin == Origin::Default,
                    monogram,
                },
                Err(err) => {
                    tracing::warn!(path = %found.path.display(), "can't load console icon: {err}");
                    ConsoleIcon {
                        monogram,
                        ..ConsoleIcon::default()
                    }
                }
            },
            None => ConsoleIcon {
                monogram,
                ..ConsoleIcon::default()
            },
        };
        self.loaded.insert(system.to_owned(), icon.clone());
        icon
    }
}

/// "N64", "Wii" (short names up to 4 characters), else the system id in capitals ("GC").
fn monogram(system: &str, systems: &Systems) -> String {
    let short = systems
        .get(&hearth_core::model::SystemId::new(system))
        .map(|s| s.short.clone())
        .filter(|s| s.chars().count() <= 4);
    short.unwrap_or_else(|| system.to_uppercase())
}

/// The art of every game, by id.
pub type ArtMap = HashMap<GameId, TileArt>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monograms_are_short() {
        let systems = Systems::builtin().unwrap();
        assert_eq!(monogram("n64", &systems), "N64");
        assert_eq!(monogram("gc", &systems), "GC");
        assert_eq!(monogram("wii", &systems), "Wii");
        assert_eq!(monogram("snes", &systems), "SNES");
    }
}

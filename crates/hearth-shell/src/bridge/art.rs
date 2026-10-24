//! Game art and console icons for the UI.
//!
//! The art worker ([`hearth_core::art::ArtService`]) reports file paths. The [`ArtLoader`]
//! turns them into display-sized copies ([`hearth_core::art::display`]) and decodes those on a
//! small thread pool, most visible first: the focused cell's hero and logo, the grids in view,
//! the neighbours' heroes and logos, then the rest. Decoded images cross to the UI thread as
//! `SharedPixelBuffer`s (which are `Send`) and become `slint::Image`s there.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use hearth_core::art::{
    ArtCache, ArtEvent, ArtKey, ArtKind, ArtService, DisplayClass, GameArt, MetadataProvider,
};
use hearth_core::config::PackCategory;
use hearth_core::custom::CustomStore;
use hearth_core::model::GameId;
use hearth_core::packs::{Origin, Resolver};
use hearth_core::systems::Systems;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use super::session::defer;

/// The kinds Home draws (the blurred hero and the icon stay on disk).
const SHOWN: [ArtKind; 3] = [ArtKind::Hero, ArtKind::Logo, ArtKind::Grid];
/// Game cells that can be in view at once (a 16:9 or 16:10 screen shows about 11).
const IN_VIEW: usize = 12;
/// Decoding threads.
const THREADS: usize = 3;

/// One game's art on the UI thread. Missing images are empty (placeholders are drawn).
#[derive(Debug, Clone, Default)]
pub struct TileArt {
    pub grid: Image,
    pub hero: Image,
    pub logo: Image,
    /// The SteamGridDB game it matched.
    pub match_name: Option<String>,
}

impl TileArt {
    /// Puts a decoded image (or none) in its slot.
    pub fn set(&mut self, kind: ArtKind, pixels: Option<SharedPixelBuffer<Rgba8Pixel>>) {
        let image = pixels.map(Image::from_rgba8).unwrap_or_default();
        match kind {
            ArtKind::Grid => self.grid = image,
            ArtKind::Hero => self.hero = image,
            ArtKind::Logo => self.logo = image,
            ArtKind::Icon => {}
        }
    }
}

/// One image to show: where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    key: ArtKey,
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
}

#[derive(Debug, Default)]
struct State {
    class: DisplayClass,
    /// Bumped by a class change; older results are dropped.
    generation: u64,
    /// Position of each game in the Home row, and the focused position.
    order: HashMap<GameId, usize>,
    focus: usize,
    /// Every image to show, and those still to decode.
    sources: HashMap<(GameId, ArtKind), Source>,
    pending: Vec<(GameId, ArtKind)>,
}

impl State {
    /// Lower is sooner.
    fn rank(&self, game: &GameId, kind: ArtKind) -> (u8, usize, u8) {
        let distance = self
            .order
            .get(game)
            .map_or(usize::MAX, |pos| pos.abs_diff(self.focus));
        let kind_order = match kind {
            ArtKind::Hero => 0,
            ArtKind::Logo => 1,
            _ => 2,
        };
        match (distance, kind) {
            (0, _) => (0, 0, kind_order),
            (d, ArtKind::Grid) if d <= IN_VIEW => (1, d, 0),
            (d, _) if d <= 2 => (2, d, kind_order),
            (d, _) => (3, d, kind_order),
        }
    }

    /// Takes the most urgent pending image.
    fn next(&mut self) -> Option<(GameId, ArtKind, Source)> {
        loop {
            let best = self
                .pending
                .iter()
                .enumerate()
                .min_by_key(|(_, (game, kind))| self.rank(game, *kind))
                .map(|(i, _)| i)?;
            let (game, kind) = self.pending.swap_remove(best);
            if let Some(source) = self.sources.get(&(game.clone(), kind)) {
                return Some((game, kind, source.clone()));
            }
        }
    }
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    work: Condvar,
}

/// Decodes art for Home on a few threads, most visible first. Cheap to clone.
#[derive(Debug, Clone)]
pub struct ArtLoader {
    shared: Arc<Shared>,
}

impl ArtLoader {
    /// Starts the decoding threads; copies for `class` go in `cache`.
    pub fn start(cache: &ArtCache, class: DisplayClass) -> Self {
        let shared = Arc::new(Shared::default());
        shared.lock().class = class;
        let threads = thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(1))
            .clamp(1, THREADS);
        for i in 0..threads {
            let (shared, cache) = (shared.clone(), cache.clone());
            let spawned = thread::Builder::new()
                .name(format!("hearth-art-decode-{i}"))
                .spawn(move || decode_loop(&shared, &cache));
            if let Err(err) = spawned {
                tracing::error!("cannot start an art decoding thread: {err}");
            }
        }
        Self { shared }
    }

    /// The art worker found `art` for `game`: decodes what changed, clears what is gone.
    pub fn submit(&self, game: &GameId, key: &ArtKey, art: &GameArt) {
        let mut cleared = Vec::new();
        {
            let mut state = self.shared.lock();
            for kind in SHOWN {
                let slot = (game.clone(), kind);
                let Some(path) = art.get(kind) else {
                    if state.sources.remove(&slot).is_some() {
                        state.pending.retain(|p| *p != slot);
                        cleared.push(kind);
                    }
                    continue;
                };
                let source = Source {
                    key: key.clone(),
                    path: path.to_path_buf(),
                    modified: std::fs::metadata(path).and_then(|m| m.modified()).ok(),
                };
                if state.sources.get(&slot) == Some(&source) {
                    continue;
                }
                state.sources.insert(slot.clone(), source);
                if !state.pending.contains(&slot) {
                    state.pending.push(slot);
                }
            }
        }
        self.shared.work.notify_all();
        if !cleared.is_empty() {
            let game = game.clone();
            defer(move |c| {
                for kind in cleared {
                    c.set_art_image(&game, kind, None);
                }
            });
        }
    }

    /// The Home row's games, in order (`order[i]` is at cell `i`).
    pub fn set_order(&self, order: &[Option<GameId>]) {
        let mut state = self.shared.lock();
        state.order = order
            .iter()
            .enumerate()
            .filter_map(|(i, id)| id.clone().map(|id| (id, i)))
            .collect();
    }

    /// The focused cell moved.
    pub fn set_focus(&self, index: usize) {
        self.shared.lock().focus = index;
    }

    /// True if `game` has a `kind` image to show (decoded or not yet).
    pub fn has_source(&self, game: &GameId, kind: ArtKind) -> bool {
        self.shared
            .lock()
            .sources
            .contains_key(&(game.clone(), kind))
    }

    /// The screen class; changing it remakes and reloads every image.
    pub fn class(&self) -> DisplayClass {
        self.shared.lock().class
    }

    /// Switches to `class`: every image is decoded again from copies for it.
    pub fn set_class(&self, class: DisplayClass) {
        {
            let mut state = self.shared.lock();
            if state.class == class {
                return;
            }
            state.class = class;
            state.generation += 1;
            state.pending = state.sources.keys().cloned().collect();
        }
        self.shared.work.notify_all();
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn decode_loop(shared: &Shared, cache: &ArtCache) {
    loop {
        let (game, kind, source, class, generation) = {
            let mut state = shared.lock();
            loop {
                if let Some((game, kind, source)) = state.next() {
                    break (game, kind, source, state.class, state.generation);
                }
                state = shared
                    .work
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let pixels = load(cache, &source, kind, class);
        let current = {
            let state = shared.lock();
            state.generation == generation
                && state.sources.get(&(game.clone(), kind)) == Some(&source)
        };
        if current {
            defer(move |c| c.set_art_image(&game, kind, pixels));
        }
    }
}

/// The display copy of `source` for `class` (made if needed), decoded. If the copy can't be
/// written, the original is decoded and shrunk in memory instead.
fn load(
    cache: &ArtCache,
    source: &Source,
    kind: ArtKind,
    class: DisplayClass,
) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let path = match cache.sized(&source.key, kind, &source.path, class) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(path = %source.path.display(), "can't make the display copy: {err}");
            source.path.clone()
        }
    };
    let image = match image::ImageReader::open(&path).and_then(|r| r.with_guessed_format()) {
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
    let (max_w, max_h) = class.bounds(kind);
    let image = if image.width() > max_w || image.height() > max_h {
        image.resize(max_w, max_h, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.into_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    ))
}

/// Starts the art worker; its paths go to `loader`, match names to the session.
pub fn start(
    cache: ArtCache,
    picks: CustomStore,
    library_db: PathBuf,
    provider: Option<Box<dyn MetadataProvider>>,
    loader: ArtLoader,
) -> Option<ArtService> {
    let spawned = ArtService::spawn(
        cache,
        picks,
        library_db,
        provider,
        move |event| match event {
            ArtEvent::Art {
                game,
                key,
                art,
                match_name,
            } => {
                super::startup::note("first art event");
                loader.submit(&game, &key, &art);
                defer(move |c| c.set_art_match(&game, match_name));
            }
        },
    );
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

    fn state(order: &[&str], focus: usize) -> State {
        let mut state = State {
            focus,
            ..State::default()
        };
        state.order = order
            .iter()
            .enumerate()
            .map(|(i, id)| (GameId::new(*id), i))
            .collect();
        for id in order {
            for kind in SHOWN {
                let slot = (GameId::new(*id), kind);
                state.sources.insert(
                    slot.clone(),
                    Source {
                        key: ArtKey::from_stored("n64/X"),
                        path: PathBuf::from(format!("/{id}/{}.png", kind.name())),
                        modified: None,
                    },
                );
                state.pending.push(slot);
            }
        }
        state
    }

    #[test]
    fn focused_hero_and_logo_first_then_grids_in_view_then_neighbours() {
        let ids: Vec<String> = (0..20).map(|i| format!("g{i}")).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut state = state(&ids, 1);
        let mut taken = Vec::new();
        while let Some((game, kind, _)) = state.next() {
            taken.push(format!("{}:{}", game.as_str(), kind.name()));
        }
        assert_eq!(taken[..3], ["g1:hero", "g1:logo", "g1:grid"]);
        // Then every grid in view, nearest first...
        assert_eq!(taken[3..5], ["g0:grid", "g2:grid"]);
        assert!(taken[3..16].iter().all(|t| t.ends_with(":grid")));
        // ...then the neighbours' heroes and logos, then the rest.
        assert!(taken[16..22].iter().all(|t| !t.ends_with(":grid")));
        assert_eq!(taken[22..25], ["g4:hero", "g4:logo", "g5:hero"]);
        assert_eq!(taken.len(), 60);
    }

    #[test]
    fn focus_changes_the_order() {
        let mut state = state(&["a", "b", "c", "d"], 0);
        state.focus = 3;
        assert_eq!(
            state.next().map(|(g, k, _)| (g, k)),
            Some((GameId::new("d"), ArtKind::Hero))
        );
    }

    #[test]
    fn monograms_are_short() {
        let systems = Systems::builtin().unwrap();
        assert_eq!(monogram("n64", &systems), "N64");
        assert_eq!(monogram("gc", &systems), "GC");
        assert_eq!(monogram("wii", &systems), "Wii");
        assert_eq!(monogram("snes", &systems), "SNES");
    }
}

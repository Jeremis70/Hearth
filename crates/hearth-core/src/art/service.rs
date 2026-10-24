//! The art worker: one thread that identifies games, shows what is cached at once, and fetches
//! the rest from the [`MetadataProvider`].
//!
//! Per game, run once and remembered in the library database:
//! 1. identify it ([`identify`]) and record its art key;
//! 2. report the art already there (local files, cache) right away;
//! 3. if there is no result yet, search the provider, pick the match, note which kinds of
//!    art exist, then download them. A match is kept permanently; "no match" is kept too
//!    and not retried (until a later "Refresh artwork");
//! 4. a network or HTTP error puts the key on the queue instead. The queue is retried at the
//!    next startup and, while Hearth runs, on a timer once each entry's backoff is over.
//!
//! A matched game whose cached files went missing downloads just those again, without a new
//! search.
//!
//! With animated art on ([`ArtService::set_animated`]), a matched game also gets the best
//! animated hero and logo beside their stills (one more lookup each, same queue and backoff),
//! up to [`AUTO_ANIMATED_HERO_MAX`] / [`AUTO_ANIMATED_LOGO_MAX`] (bigger ones stay among the
//! Customize page's candidates and download when picked). A kind looked
//! up once isn't looked up again, whether one was taken or not; turning animation off never
//! deletes them, so turning it back on needs no download.
//!
//! The worker also serves the Customize page ([`customize`]): candidates to choose from,
//! applying a choice, matching another game, and every change to the user's picks (so writes
//! to a game's `custom.toml` never race).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::cache::{ArtCache, GameArt};
use super::identity::{ArtKey, Identity, identify, pick_match};
use super::provider::{ArtKind, MetadataProvider, ProviderError};
use crate::custom::{Custom, CustomStore};
use crate::extras::{Extra, Extras};
use crate::library::{ArtCandidate, ArtStatus, Library, LibraryError, RemovedGame};
use crate::model::{Game, GameId};

/// How often the queue is looked at while Hearth runs.
pub const QUEUE_CHECK: Duration = Duration::from_secs(60);
/// The largest animated hero downloaded without being picked: the provider's own cap (30 MiB).
/// Animated heroes run from ~0.2 MB to ~30 MB on SteamGridDB; a console has the room, so
/// there is no lower limit of its own.
pub const AUTO_ANIMATED_HERO_MAX: u64 = 30 * 1024 * 1024;
/// The largest animated logo downloaded without being picked (the provider's own cap).
pub const AUTO_ANIMATED_LOGO_MAX: u64 = 30 * 1024 * 1024;
/// The kinds that animate.
pub const ANIMATED_KINDS: [ArtKind; 2] = [ArtKind::Hero, ArtKind::Logo];

mod customize;

pub use customize::{Availability, Candidate, CandidateSet};

/// What the worker tells the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum ArtEvent {
    /// The art to show for a game. Sent for every game when the library is loaded, and again
    /// whenever more art arrives.
    Art {
        /// The game.
        game: GameId,
        /// Its art key (where its display copies go, see [`super::display`]).
        key: ArtKey,
        /// Its art; empty means placeholders.
        art: GameArt,
        /// The SteamGridDB game it matched, if any (for Properties).
        match_name: Option<String>,
        /// Where the hero is cropped: 0 = its left edge … 1 = its right edge.
        hero_position: f32,
    },
    /// Things to choose from, for the Customize page (answer to a candidates request). Thumbs
    /// still missing follow as [`ArtEvent::Thumb`].
    Candidates {
        /// The game's art key.
        key: ArtKey,
        /// Which list.
        set: CandidateSet,
        /// The choices, best first.
        items: Vec<Candidate>,
        /// Whether they are fresh from SteamGridDB, or why not.
        availability: Availability,
    },
    /// A candidate's thumbnail arrived.
    Thumb {
        /// The game's art key.
        key: ArtKey,
        /// Which list.
        set: CandidateSet,
        /// The candidate's id.
        id: u64,
        /// The thumbnail.
        path: PathBuf,
    },
    /// A game's picks changed (after any change requested through [`ArtService`]).
    Custom {
        /// The game's art key.
        key: ArtKey,
        /// Its picks now.
        custom: Custom,
    },
    /// A requested change failed; nothing changed.
    Failed {
        /// The game's art key.
        key: ArtKey,
        /// For the user ("Couldn't download the image").
        message: String,
    },
}

enum Command {
    Library(Vec<Game>),
    Candidates(ArtKey, CandidateSet),
    Apply(ArtKey, ArtKind, u64),
    PickFile(ArtKey, Extra, PathBuf),
    Reset(ArtKey, Extra),
    Rematch(ArtKey, u64, String),
    HeroPosition(ArtKey, f32),
    ThemeStart(ArtKey, f32),
    RemoveTheme(ArtKey),
    Animated(bool),
    /// Thumbnails left over from a list (resumed when nothing else waits).
    Thumbs(customize::ThumbWork),
}

/// Handle to the art worker thread; once every clone is dropped, the thread stops after its
/// current job.
#[derive(Debug, Clone)]
pub struct ArtService {
    tx: Sender<Command>,
}

impl ArtService {
    /// Starts the worker. Without a `provider` (no API key) only local and cached art is
    /// shown. `animated`: whether animated heroes and logos are fetched too.
    pub fn spawn(
        cache: ArtCache,
        picks: CustomStore,
        library_db: PathBuf,
        provider: Option<Box<dyn MetadataProvider>>,
        animated: bool,
        events: impl Fn(ArtEvent) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("hearth-art".into())
            .spawn(move || {
                let library = match Library::open(&library_db) {
                    Ok(library) => library,
                    Err(err) => {
                        tracing::warn!("art worker can't open the library: {err}");
                        return;
                    }
                };
                let mut worker = Worker::new(cache, picks, library, provider, Box::new(events));
                worker.animated = animated;
                worker.inbox = Some(rx);
                loop {
                    match worker.next_command() {
                        Ok(command) => worker.handle(command),
                        Err(RecvTimeoutError::Timeout) => worker.retry_due(),
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(Self { tx })
    }

    /// The library was (re)loaded: report art for these games and fetch what is missing.
    pub fn library_loaded(&self, games: Vec<Game>) {
        self.send(Command::Library(games));
    }

    /// Asks for `set`'s candidates for `key` ([`ArtEvent::Candidates`], then thumbs). A newer
    /// request stops the thumbs of an older one.
    pub fn candidates(&self, key: ArtKey, set: CandidateSet) {
        self.send(Command::Candidates(key, set));
    }

    /// Uses the SteamGridDB image `asset` (from the last candidates of `kind`) as `key`'s
    /// `kind`.
    pub fn apply(&self, key: ArtKey, kind: ArtKind, asset: u64) {
        self.send(Command::Apply(key, kind, asset));
    }

    /// Copies `file` in as `key`'s `extra`.
    pub fn pick_file(&self, key: ArtKey, extra: Extra, file: PathBuf) {
        self.send(Command::PickFile(key, extra, file));
    }

    /// Back to automatic for `key`'s `extra`.
    pub fn reset(&self, key: ArtKey, extra: Extra) {
        self.send(Command::Reset(key, extra));
    }

    /// Matches `key` to the SteamGridDB game `sgdb_id` instead: its automatic art is fetched
    /// again; picks made from the old match go, picks from files stay, the hero is centered.
    pub fn rematch(&self, key: ArtKey, sgdb_id: u64, name: String) {
        self.send(Command::Rematch(key, sgdb_id, name));
    }

    /// Where `key`'s hero is cropped (0 to 1).
    pub fn set_hero_position(&self, key: ArtKey, position: f32) {
        self.send(Command::HeroPosition(key, position));
    }

    /// Seconds added to where `key`'s theme starts.
    pub fn set_theme_start(&self, key: ArtKey, seconds: f32) {
        self.send(Command::ThemeStart(key, seconds));
    }

    /// Removes `key`'s theme (see [`CustomStore::remove_theme`]).
    pub fn remove_theme(&self, key: ArtKey) {
        self.send(Command::RemoveTheme(key));
    }

    /// Whether animated heroes and logos are fetched (and offered first). Turning it on
    /// fetches the ones never looked up.
    pub fn set_animated(&self, on: bool) {
        self.send(Command::Animated(on));
    }

    fn send(&self, command: Command) {
        if self.tx.send(command).is_err() {
            tracing::warn!("the art worker has stopped");
        }
    }
}

/// Every game sharing one art key (two dumps of the same disc), plus how it was identified.
struct Known {
    identity: Identity,
    games: Vec<Game>,
}

struct Worker {
    cache: ArtCache,
    library: Library,
    provider: Option<Box<dyn MetadataProvider>>,
    events: Box<dyn Fn(ArtEvent) + Send>,
    games: HashMap<ArtKey, Known>,
    warned_no_key: bool,
    /// What the user picked for each game.
    picks: CustomStore,
    /// Where a game's own art comes from (ahead of the cache).
    extras: Extras,
    /// Requests (none in tests that drive the worker directly).
    inbox: Option<Receiver<Command>>,
    /// Requests read early (while checking for newer ones), handled next.
    pending: VecDeque<Command>,
    /// Backoff for requests made from the Customize page.
    net: customize::NetBackoff,
    /// The last candidates listed per key and kind (to apply one by id, and to mark the one
    /// in use again after a change).
    listed: HashMap<(ArtKey, ArtKind), (Vec<super::provider::Asset>, Availability)>,
    /// Thumbnails put off for a request that came in meanwhile.
    backlog: Option<customize::ThumbWork>,
    /// Fetch animated heroes and logos too.
    animated: bool,
}

/// What to fetch for one key.
enum Job {
    /// Never looked up: search, then download everything.
    Full,
    /// Matched before; these kinds are missing from the cache, and these animated kinds were
    /// never looked up.
    Missing {
        sgdb_id: u64,
        sgdb_name: String,
        kinds: Vec<ArtKind>,
        animated: Vec<ArtKind>,
    },
}

impl Worker {
    fn new(
        cache: ArtCache,
        picks: CustomStore,
        library: Library,
        provider: Option<Box<dyn MetadataProvider>>,
        events: Box<dyn Fn(ArtEvent) + Send>,
    ) -> Self {
        Self {
            cache,
            library,
            provider,
            events,
            games: HashMap::new(),
            warned_no_key: false,
            extras: Extras::standard(picks.clone()),
            picks,
            inbox: None,
            pending: VecDeque::new(),
            net: customize::NetBackoff::default(),
            listed: HashMap::new(),
            backlog: None,
            animated: true,
        }
    }

    fn next_command(&mut self) -> Result<Command, RecvTimeoutError> {
        if let Some(command) = self.pending.pop_front() {
            return Ok(command);
        }
        let Some(inbox) = &self.inbox else {
            return Err(RecvTimeoutError::Disconnected);
        };
        // Put-off thumbnails once nothing else waits.
        if let Ok(command) = inbox.try_recv() {
            return Ok(command);
        }
        if let Some(work) = self.backlog.take() {
            return Ok(Command::Thumbs(work));
        }
        inbox.recv_timeout(QUEUE_CHECK)
    }

    /// True once any request waits: thumbnails give way (they resume afterwards, unless a
    /// newer list is wanted).
    fn superseded(&mut self) -> bool {
        if let Some(inbox) = &self.inbox {
            while let Ok(command) = inbox.try_recv() {
                self.pending.push_back(command);
            }
        }
        !self.pending.is_empty()
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Library(games) => self.load(games),
            Command::Candidates(key, set) => {
                // A newer list: thumbs put off for an older one aren't wanted any more.
                self.backlog = None;
                self.candidates(&key, set);
            }
            Command::Thumbs(work) => self.resume_thumbs(work),
            Command::Apply(key, kind, asset) => self.apply(&key, kind, asset),
            Command::PickFile(key, extra, file) => {
                let done = self.picks.pick_file(&key, extra, &file);
                self.changed(&key, done.map(drop), "Couldn't copy the file");
            }
            Command::Reset(key, extra) => {
                let done = self.picks.reset(&key, extra);
                self.changed(&key, done, "Couldn't reset it");
            }
            Command::Rematch(key, sgdb_id, name) => self.rematch(&key, sgdb_id, name),
            Command::HeroPosition(key, position) => {
                let done = self.picks.set_hero_position(&key, position);
                self.changed(&key, done, "Couldn't save the position");
            }
            Command::ThemeStart(key, seconds) => {
                let done = self.picks.set_theme_start(&key, seconds);
                self.changed(&key, done, "Couldn't save the start");
            }
            Command::RemoveTheme(key) => {
                let done = self.picks.remove_theme(&key).map(drop);
                self.changed(&key, done, "Couldn't remove the theme");
            }
            Command::Animated(on) => self.set_animated(on),
        }
    }

    fn set_animated(&mut self, on: bool) {
        if self.animated == on {
            return;
        }
        self.animated = on;
        // The Customize page's lists put animated images first only while it's on.
        let listed: Vec<(ArtKey, ArtKind)> = self.listed.keys().cloned().collect();
        for (key, kind) in listed {
            self.send_art_list(&key, kind);
        }
        if !on || self.provider.is_none() {
            return;
        }
        let mut keys: Vec<ArtKey> = self.games.keys().cloned().collect();
        keys.sort();
        let jobs: Vec<(ArtKey, Job)> = keys
            .into_iter()
            .filter_map(|key| {
                let status = self.library.art_status(&key).ok().flatten();
                self.job_for(&key, status).map(|job| (key, job))
            })
            .collect();
        if !jobs.is_empty() {
            self.run(jobs);
        }
    }

    /// After a change to `key`'s picks: the new art and picks, or what failed.
    fn changed(&self, key: &ArtKey, done: std::io::Result<()>, failure: &str) {
        if let Err(err) = done {
            tracing::warn!(%key, "{failure}: {err}");
            (self.events)(ArtEvent::Failed {
                key: key.clone(),
                message: failure.to_owned(),
            });
            return;
        }
        let status = self.library.art_status(key).ok().flatten();
        self.report(key, status.as_ref());
        (self.events)(ArtEvent::Custom {
            key: key.clone(),
            custom: self.picks.load(key),
        });
        // The lists again, marking what is in use now (no new request).
        let kinds: Vec<ArtKind> = self
            .listed
            .keys()
            .filter(|(k, _)| k == key)
            .map(|(_, kind)| *kind)
            .collect();
        for kind in kinds {
            self.send_art_list(key, kind);
        }
    }

    fn load(&mut self, games: Vec<Game>) {
        self.games.clear();
        for game in games {
            let identity = identify(&game.system, &game.path);
            self.picks.remember(&game.id, &identity.key);
            if let Err(err) = self.library.set_art_key(&game.id, &identity.key) {
                tracing::warn!("can't record the art key of {}: {err}", game.title);
            }
            self.games
                .entry(identity.key.clone())
                .or_insert_with(|| Known {
                    identity,
                    games: Vec::new(),
                })
                .games
                .push(game);
        }
        let mut keys: Vec<ArtKey> = self.games.keys().cloned().collect();
        keys.sort();
        // Everything already on disk first, so the row fills in before any network wait.
        let mut jobs = Vec::new();
        for key in keys {
            let status = self.library.art_status(&key).unwrap_or_else(|err| {
                tracing::warn!("can't read the art status of {key}: {err}");
                None
            });
            self.report(&key, status.as_ref());
            if let Some(job) = self.job_for(&key, status) {
                jobs.push((key, job));
            }
        }
        if jobs.is_empty() {
            return;
        }
        if self.provider.is_none() {
            if !self.warned_no_key {
                tracing::info!(
                    "no SteamGridDB API key (set {} or [art] steamgriddb_key in hearth.toml): \
                     {} games keep placeholder art",
                    crate::config::STEAMGRIDDB_KEY_ENV,
                    jobs.len()
                );
                self.warned_no_key = true;
            }
            return;
        }
        // At startup the whole queue is retried, whatever its backoff said.
        self.run(jobs);
    }

    /// Retries queued keys whose backoff is over.
    fn retry_due(&mut self) {
        if self.provider.is_none() {
            return;
        }
        let now = unix_now();
        let queue = match self.library.art_queue() {
            Ok(queue) => queue,
            Err(err) => {
                tracing::warn!("can't read the art queue: {err}");
                return;
            }
        };
        let jobs: Vec<_> = queue
            .into_iter()
            .filter(|q| q.next_try <= now && self.games.contains_key(&q.key))
            .filter_map(|q| {
                let status = self.library.art_status(&q.key).ok().flatten();
                self.job_for(&q.key, status).map(|job| (q.key, job))
            })
            .collect();
        if !jobs.is_empty() {
            tracing::info!(
                count = jobs.len(),
                "retrying artwork that needed the network"
            );
            self.run(jobs);
        }
    }

    fn job_for(&self, key: &ArtKey, status: Option<ArtStatus>) -> Option<Job> {
        match status {
            None => Some(Job::Full),
            Some(ArtStatus::NoMatch) => None,
            Some(ArtStatus::Matched {
                sgdb_id,
                sgdb_name,
                kinds,
            }) => {
                let missing: Vec<_> = kinds
                    .into_iter()
                    .filter(|kind| self.cache.cached(key, *kind).is_none())
                    .collect();
                let animated = self.animated_to_look_up(key);
                (!missing.is_empty() || !animated.is_empty()).then_some(Job::Missing {
                    sgdb_id,
                    sgdb_name,
                    kinds: missing,
                    animated,
                })
            }
        }
    }

    /// The animated kinds `key` still needs looked up (none while animation is off).
    fn animated_to_look_up(&self, key: &ArtKey) -> Vec<ArtKind> {
        if !self.animated {
            return Vec::new();
        }
        ANIMATED_KINDS
            .into_iter()
            .filter(|kind| !self.cache.animated_looked_up(key, *kind))
            .collect()
    }

    fn run(&mut self, jobs: Vec<(ArtKey, Job)>) {
        let mut offline: Option<String> = None;
        let mut queued = 0usize;
        for (key, job) in jobs {
            // Once the network is down, don't wait on it for every game: queue the rest.
            if let Some(error) = &offline {
                self.queue(&key, error);
                queued += 1;
                continue;
            }
            match self.fetch(&key, job) {
                Ok(status) => {
                    self.net.succeeded();
                    if let Err(err) = self.library.set_art_status(&key, &status, unix_now()) {
                        tracing::warn!("can't save the art result for {key}: {err}");
                    }
                    self.report(&key, Some(&status));
                }
                Err(FetchError::Provider(err)) => {
                    let error = err.to_string();
                    self.queue(&key, &error);
                    queued += 1;
                    if matches!(err, ProviderError::Network(_)) {
                        self.net.failed();
                        offline = Some(error);
                    } else {
                        tracing::info!(%key, "artwork failed, will retry: {error}");
                    }
                }
                Err(err @ (FetchError::Library(_) | FetchError::Unknown)) => {
                    tracing::warn!("art for {key}: {err}");
                }
            }
        }
        if let Some(error) = offline {
            tracing::info!(
                games = queued,
                "no network for artwork ({error}); queued, will retry"
            );
        }
    }

    fn queue(&self, key: &ArtKey, error: &str) {
        if let Err(err) = self.library.queue_art(key, error, unix_now()) {
            tracing::warn!("can't queue the art for {key}: {err}");
        }
    }

    fn fetch(&self, key: &ArtKey, job: Job) -> Result<ArtStatus, FetchError> {
        let Some(provider) = self.provider.as_deref() else {
            return Err(FetchError::Provider(ProviderError::Invalid(
                "no provider".into(),
            )));
        };
        let Some(known) = self.games.get(key) else {
            return Err(FetchError::Unknown);
        };
        let (sgdb_id, sgdb_name, kinds, animated) = match job {
            Job::Missing {
                sgdb_id,
                sgdb_name,
                kinds,
                animated,
            } => (sgdb_id, sgdb_name, kinds, animated),
            Job::Full => {
                let mut found = None;
                let mut considered = Vec::new();
                for name in &known.identity.search_names {
                    let results = provider.search(name)?;
                    considered.extend(results.iter().map(|g| ArtCandidate {
                        sgdb_id: g.id,
                        name: g.name.clone(),
                        query: name.clone(),
                    }));
                    if let Some(hit) = pick_match(&results, name, |g| g.name.as_str()) {
                        found = Some(hit.clone());
                        break;
                    }
                }
                // Kept for "Wrong game?".
                if let Err(err) = self.library.set_art_candidates(key, &considered) {
                    tracing::warn!("can't save the match candidates of {key}: {err}");
                }
                let Some(hit) = found else {
                    tracing::info!(
                        %key,
                        names = ?known.identity.search_names,
                        "no SteamGridDB match; keeping the placeholder art"
                    );
                    return Ok(ArtStatus::NoMatch);
                };
                tracing::info!(%key, id = hit.id, name = %hit.name, "matched on SteamGridDB");
                (
                    hit.id,
                    hit.name,
                    ArtKind::ALL.to_vec(),
                    self.animated_to_look_up(key),
                )
            }
        };
        // Look up every kind first and save the match with the kinds that exist, so a failed
        // download later only re-downloads what's missing.
        let mut assets = Vec::new();
        for kind in kinds {
            match provider.best_asset(sgdb_id, kind) {
                Ok(Some(asset)) => assets.push((kind, asset)),
                Ok(None) => {}
                // One kind the service always refuses mustn't hold back the others.
                Err(err) if err.is_permanent() => {
                    tracing::warn!(%key, kind = kind.name(), "skipping this kind of art: {err}");
                }
                Err(err) => return Err(err.into()),
            }
        }
        let mut all_kinds: Vec<ArtKind> = match self.library.art_status(key)? {
            Some(ArtStatus::Matched { kinds, .. }) => kinds,
            _ => Vec::new(),
        };
        all_kinds.extend(assets.iter().map(|(kind, _)| *kind));
        all_kinds.sort();
        all_kinds.dedup();
        let status = ArtStatus::Matched {
            sgdb_id,
            sgdb_name,
            kinds: all_kinds,
        };
        self.library.set_art_status(key, &status, unix_now())?;
        for (kind, asset) in assets {
            let bytes = provider.download(&asset)?;
            if image::guess_format(&bytes).is_err() {
                return Err(FetchError::Provider(ProviderError::Invalid(format!(
                    "{} for {key} isn't an image",
                    kind.name()
                ))));
            }
            if let Err(err) = self.cache.store(key, kind, &asset.extension, &bytes) {
                tracing::warn!("can't cache the {} of {key}: {err}", kind.name());
            } else {
                // So the Customize page can mark the automatic choice among the candidates.
                self.cache.set_source_url(key, kind, &asset.url);
            }
        }
        for kind in animated {
            self.fetch_animated(provider, key, sgdb_id, kind)?;
        }
        Ok(status)
    }

    /// Looks up and downloads the automatic animated `kind`: the best one small enough
    /// ([`AUTO_ANIMATED_HERO_MAX`], [`AUTO_ANIMATED_LOGO_MAX`]) whose content really
    /// animates. Remembers when there is none.
    fn fetch_animated(
        &self,
        provider: &dyn MetadataProvider,
        key: &ArtKey,
        sgdb_id: u64,
        kind: ArtKind,
    ) -> Result<(), FetchError> {
        let max = if kind == ArtKind::Hero {
            AUTO_ANIMATED_HERO_MAX
        } else {
            AUTO_ANIMATED_LOGO_MAX
        };
        let assets = match provider.animated_assets(sgdb_id, kind) {
            Ok(assets) => assets,
            Err(err) if err.is_permanent() => {
                tracing::warn!(%key, kind = kind.name(), "skipping animated art: {err}");
                Vec::new()
            }
            Err(err) => return Err(err.into()),
        };
        // Unknown sizes are tried too: the download stops at `max`.
        for asset in assets
            .iter()
            .filter(|a| a.full.size.is_none_or(|s| s <= max))
        {
            let Some(bytes) = provider.download_limited(&asset.full, max)? else {
                continue;
            };
            let Some(format) = super::animation::probe_bytes(&bytes) else {
                tracing::info!(%key, kind = kind.name(), id = asset.id, "listed as animated, isn't");
                continue;
            };
            match self
                .cache
                .store_animated(key, kind, format.extension(), &bytes)
            {
                Ok(_) => {
                    self.cache
                        .set_animated_source_url(key, kind, &asset.full.url);
                    tracing::info!(
                        %key,
                        kind = kind.name(),
                        id = asset.id,
                        bytes = bytes.len(),
                        format = format.name(),
                        "animated art cached"
                    );
                }
                Err(err) => {
                    tracing::warn!("can't cache the animated {} of {key}: {err}", kind.name());
                }
            }
            return Ok(());
        }
        tracing::info!(
            %key,
            kind = kind.name(),
            listed = assets.len(),
            "no animated art small enough to take automatically"
        );
        if let Err(err) = self.cache.mark_no_animated(key, kind) {
            tracing::warn!("can't note the animated {} of {key}: {err}", kind.name());
        }
        Ok(())
    }

    /// Sends the art on disk for every game with `key`.
    fn report(&self, key: &ArtKey, status: Option<&ArtStatus>) {
        let Some(known) = self.games.get(key) else {
            return;
        };
        let match_name = match status {
            Some(ArtStatus::Matched { sgdb_name, .. }) => Some(sgdb_name.clone()),
            _ => None,
        };
        let hero_position = self.picks.load(key).hero_position();
        for game in &known.games {
            (self.events)(ArtEvent::Art {
                game: game.id.clone(),
                key: key.clone(),
                art: self.cache.resolve(key, &self.extras.art(game)),
                match_name: match_name.clone(),
                hero_position,
            });
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum FetchError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Library(#[from] LibraryError),
    #[error("no game in the library has this key any more")]
    Unknown,
}

/// After a sync, deletes the cached art of removed games, unless their art key is still used
/// by another game. Only games whose system folder under `games_root` is still there count as
/// removed: a missing or unmounted folder must never wipe art. Call only after a successful
/// scan of `games_root`.
pub fn remove_orphaned_art(
    library: &Library,
    cache: &ArtCache,
    removed: &[RemovedGame],
    games_root: &Path,
) {
    let mut done = HashSet::new();
    for gone in removed {
        let Some(key) = &gone.art_key else {
            continue;
        };
        let folder = gone
            .game
            .path
            .strip_prefix(games_root)
            .ok()
            .and_then(|rel| rel.components().next())
            .map(|first| games_root.join(first));
        if !folder.is_some_and(|f| f.is_dir()) {
            tracing::info!(
                game = %gone.game.title,
                "keeping its art: its folder isn't under the scanned games root any more"
            );
            continue;
        }
        if !done.insert(key.clone()) || library.art_key_in_use(key).unwrap_or(true) {
            continue;
        }
        tracing::info!(%key, game = %gone.game.title, "game removed; deleting its cached art");
        if let Err(err) = cache.remove(key) {
            tracing::warn!("can't delete the art of {key}: {err}");
        }
        if let Err(err) = library.forget_art(key) {
            tracing::warn!("can't forget the art of {key}: {err}");
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::art::provider::MockMetadataProvider;
    use crate::model::SystemId;
    use crate::rom_header::tests::{disc, n64_rom};
    use crate::testdir::tempdir;

    pub(super) struct Setup {
        pub(super) _dir: tempfile::TempDir,
        pub(super) root: PathBuf,
        pub(super) db: PathBuf,
        pub(super) cache: ArtCache,
        pub(super) picks: CustomStore,
        pub(super) games: Vec<Game>,
    }

    pub(super) fn setup() -> Setup {
        let dir = tempdir();
        let root = dir.path().join("Games");
        std::fs::create_dir_all(root.join("gc")).unwrap();
        std::fs::create_dir_all(root.join("n64")).unwrap();
        let tp = root.join("gc/Legend of Zelda, The - Twilight Princess (USA).iso");
        std::fs::write(
            &tp,
            disc(b"", 0, b"GZ2E01", "The Legend of Zelda Twilight Princess"),
        )
        .unwrap();
        let oot = root.join("n64/Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!].z64");
        std::fs::write(&oot, n64_rom("z64", b"CZLE", "THE LEGEND OF ZELDA")).unwrap();
        let unknown = root.join("n64/Homebrew Thing (PD).z64");
        std::fs::write(&unknown, n64_rom("z64", b"HBTE", "HOMEBREW")).unwrap();
        let games = [
            ("tp", "Twilight Princess", "gc", tp),
            ("oot", "Ocarina of Time", "n64", oot),
            ("hb", "Homebrew Thing", "n64", unknown),
        ]
        .into_iter()
        .map(|(id, title, system, path)| Game {
            id: GameId::new(id),
            title: title.into(),
            system: SystemId::new(system),
            path,
            folder: None,
        })
        .collect::<Vec<_>>();
        let db = dir.path().join("library.sqlite3");
        Library::open(&db).unwrap().sync(&games).unwrap();
        Setup {
            cache: ArtCache::new(dir.path().join("cache/art")),
            picks: CustomStore::new(dir.path().join("data/games")),
            root,
            db,
            games,
            _dir: dir,
        }
    }

    fn catalog() -> Arc<MockMetadataProvider> {
        Arc::new(
            MockMetadataProvider::new()
                .add_game(1, "The Legend of Zelda", &ArtKind::ALL)
                .add_game(2, "The Legend of Zelda: Ocarina of Time", &ArtKind::ALL)
                .add_game(
                    3,
                    "The Legend of Zelda: Twilight Princess",
                    &[ArtKind::Grid, ArtKind::Hero, ArtKind::Logo],
                ),
        )
    }

    fn offline() -> Arc<MockMetadataProvider> {
        Arc::new(MockMetadataProvider::offline())
    }

    /// Runs one library load on a worker in this thread and returns the events.
    fn load(s: &Setup, provider: Option<Arc<MockMetadataProvider>>) -> (Worker, Vec<ArtEvent>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let mut worker = Worker::new(
            s.cache.clone(),
            s.picks.clone(),
            Library::open(&s.db).unwrap(),
            provider.map(|p| Box::new(p) as Box<dyn MetadataProvider>),
            Box::new(move |e| sink.lock().unwrap().push(e)),
        );
        worker.load(s.games.clone());
        let events = events.lock().unwrap().clone();
        (worker, events)
    }

    fn last_art(events: &[ArtEvent], id: &str) -> (GameArt, Option<String>) {
        events
            .iter()
            .rev()
            .find_map(|event| match event {
                ArtEvent::Art {
                    game,
                    art,
                    match_name,
                    ..
                } if game.as_str() == id => Some((art.clone(), match_name.clone())),
                _ => None,
            })
            .unwrap()
    }

    fn oot_art(s: &Setup) -> GameArt {
        s.cache
            .resolve(&ArtKey::from_stored("n64/CZLE"), &GameArt::default())
    }

    #[test]
    fn first_run_fetches_and_caches_then_reuses_the_cache() {
        let s = setup();
        let (worker, events) = load(&s, Some(catalog()));
        let (tp, tp_match) = last_art(&events, "tp");
        assert!(tp.grid.is_some() && tp.hero.is_some() && tp.logo.is_some());
        assert!(tp.hero_blur.is_some());
        assert_eq!(tp.icon, None);
        assert_eq!(
            tp_match.as_deref(),
            Some("The Legend of Zelda: Twilight Princess")
        );
        let (oot, oot_match) = last_art(&events, "oot");
        assert_eq!(oot, oot_art(&s));
        assert!(
            oot.grid
                .unwrap()
                .starts_with(s.cache.dir(&ArtKey::from_stored("n64/CZLE")))
        );
        // Not "The Legend of Zelda" (the truncated N64 header name).
        assert_eq!(
            oot_match.as_deref(),
            Some("The Legend of Zelda: Ocarina of Time")
        );
        // What the matcher saw is kept for "Wrong game?".
        let considered = worker
            .library
            .art_candidates(&ArtKey::from_stored("n64/CZLE"))
            .unwrap();
        assert!(considered.iter().any(|c| c.sgdb_id == 1));
        assert!(considered.iter().any(|c| c.sgdb_id == 2));
        let (hb, _) = last_art(&events, "hb");
        assert!(hb.is_empty());
        assert_eq!(
            worker
                .library
                .art_status(&ArtKey::from_stored("n64/HBTE"))
                .unwrap(),
            Some(ArtStatus::NoMatch)
        );

        // Second start, offline: everything from the cache, nothing asked.
        let provider = offline();
        let (worker, events) = load(&s, Some(provider.clone()));
        assert_eq!(last_art(&events, "tp").0, tp);
        assert_eq!(last_art(&events, "oot").0, oot_art(&s));
        assert_eq!(provider.calls("search") + provider.calls("best_asset"), 0);
        assert!(worker.library.art_queue().unwrap().is_empty());
    }

    /// Like the fixtures, but the animated images come biggest first.
    struct BigFirst(MockMetadataProvider);
    impl MetadataProvider for BigFirst {
        fn search(&self, name: &str) -> Result<Vec<super::super::CatalogGame>, ProviderError> {
            self.0.search(name)
        }
        fn best_asset(
            &self,
            id: u64,
            kind: ArtKind,
        ) -> Result<Option<super::super::AssetRef>, ProviderError> {
            self.0.best_asset(id, kind)
        }
        fn assets(
            &self,
            id: u64,
            kind: ArtKind,
        ) -> Result<Vec<super::super::Asset>, ProviderError> {
            self.0.assets(id, kind)
        }
        fn animated_assets(
            &self,
            id: u64,
            kind: ArtKind,
        ) -> Result<Vec<super::super::Asset>, ProviderError> {
            let mut list = self.0.animated_assets(id, kind)?;
            list.reverse();
            Ok(list)
        }
        fn download(&self, asset: &super::super::AssetRef) -> Result<Vec<u8>, ProviderError> {
            self.0.download(asset)
        }
    }

    #[test]
    fn animated_heroes_and_logos_come_beside_their_stills_and_are_looked_up_once() {
        let s = setup();
        let (_, events) = load(&s, Some(Arc::new(MockMetadataProvider::fixtures())));
        let (tp, _) = last_art(&events, "tp");
        let hero = tp.hero_animated.clone().unwrap();
        assert!(hero.ends_with("gc/GZ2E01/hero-animated.png"));
        assert_eq!(
            super::super::animation::probe(&hero),
            Some(super::super::AnimFormat::Apng)
        );
        assert!(
            tp.hero.as_ref().unwrap().ends_with("gc/GZ2E01/hero.png"),
            "the still stays"
        );
        assert!(tp.logo_animated.is_some());
        assert_eq!(tp.animated(ArtKind::Grid), None);

        let again = Arc::new(MockMetadataProvider::fixtures());
        load(&s, Some(again.clone()));
        assert_eq!(again.calls("animated_assets"), 0);
    }

    #[test]
    fn animated_art_waits_until_turned_on() {
        let s = setup();
        let provider = Arc::new(MockMetadataProvider::fixtures());
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let mut worker = Worker::new(
            s.cache.clone(),
            s.picks.clone(),
            Library::open(&s.db).unwrap(),
            Some(Box::new(provider.clone())),
            Box::new(move |e| sink.lock().unwrap().push(e)),
        );
        worker.animated = false;
        worker.load(s.games.clone());
        assert_eq!(provider.calls("animated_assets"), 0);
        assert!(
            last_art(&events.lock().unwrap(), "tp")
                .0
                .hero_animated
                .is_none()
        );

        worker.handle(Command::Animated(true));
        assert!(provider.calls("animated_assets") > 0);
        assert!(
            last_art(&events.lock().unwrap(), "tp")
                .0
                .hero_animated
                .is_some()
        );
    }

    #[test]
    fn heroes_too_big_to_take_automatically_are_skipped_without_downloading() {
        let s = setup();
        let provider = BigFirst(MockMetadataProvider::fixtures());
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let mut worker = Worker::new(
            s.cache.clone(),
            s.picks.clone(),
            Library::open(&s.db).unwrap(),
            Some(Box::new(provider)),
            Box::new(move |e| sink.lock().unwrap().push(e)),
        );
        worker.load(s.games.clone());
        let key = ArtKey::from_stored("gc/GZ2E01");
        let url = s.cache.animated_source_url(&key, ArtKind::Hero).unwrap();
        assert!(url.ends_with("/hero/0"), "the small one: {url}");
        assert!(s.cache.cached_animated(&key, ArtKind::Hero).is_some());
    }

    #[test]
    fn no_match_is_not_retried() {
        let s = setup();
        load(&s, Some(catalog()));
        let provider = catalog();
        let (worker, events) = load(&s, Some(provider.clone()));
        assert_eq!(provider.calls("search"), 0);
        assert!(last_art(&events, "hb").0.is_empty());
        assert!(worker.library.art_queue().unwrap().is_empty());
        assert_eq!(
            worker
                .library
                .art_status(&ArtKey::from_stored("n64/HBTE"))
                .unwrap(),
            Some(ArtStatus::NoMatch)
        );
    }

    #[test]
    fn offline_queues_every_game_and_startup_retries_them() {
        let s = setup();
        let (worker, events) = load(&s, Some(offline()));
        assert!(
            events
                .iter()
                .all(|e| matches!(e, ArtEvent::Art { art, .. } if art.is_empty()))
        );
        let queue = worker.library.art_queue().unwrap();
        assert_eq!(queue.len(), 3);
        assert!(queue.iter().all(|q| q.attempts == 1));
        drop(worker);

        // Back online at the next start: the queue is retried despite its backoff.
        let (worker, events) = load(&s, Some(catalog()));
        assert!(last_art(&events, "tp").0.grid.is_some());
        assert!(worker.library.art_queue().unwrap().is_empty());
    }

    #[test]
    fn retry_waits_for_the_backoff() {
        let s = setup();
        let (mut worker, _) = load(&s, Some(offline()));
        worker.provider = Some(Box::new(catalog()));
        worker.retry_due();
        assert_eq!(worker.library.art_queue().unwrap().len(), 3, "not due yet");
        for q in worker.library.art_queue().unwrap() {
            worker
                .library
                .queue_art(&q.key, "x", unix_now() - 3 * 3_600)
                .unwrap();
        }
        worker.retry_due();
        assert!(worker.library.art_queue().unwrap().is_empty());
    }

    #[test]
    fn without_a_key_only_cached_art_is_shown() {
        let s = setup();
        load(&s, Some(catalog()));
        let (worker, events) = load(&s, None);
        assert!(last_art(&events, "tp").0.grid.is_some());
        assert!(last_art(&events, "hb").0.is_empty());
        assert!(
            !worker.warned_no_key,
            "nothing left to fetch, nothing to warn about"
        );
        // A fresh library without a key: placeholders and one warning.
        let fresh = setup();
        let (worker, events) = load(&fresh, None);
        assert!(
            events
                .iter()
                .all(|e| matches!(e, ArtEvent::Art { art, .. } if art.is_empty()))
        );
        assert!(worker.warned_no_key);
    }

    #[test]
    fn missing_cache_files_are_downloaded_again_without_a_search() {
        let s = setup();
        load(&s, Some(catalog()));
        let key = ArtKey::from_stored("gc/GZ2E01");
        let hero = s.cache.cached(&key, ArtKind::Hero).unwrap();
        std::fs::remove_file(&hero).unwrap();
        // Only the TP hero is missing; a catalog without any search hit still restores it.
        let only_assets =
            Arc::new(MockMetadataProvider::new().add_game(3, "zzz", &[ArtKind::Hero]));
        let (_, events) = load(&s, Some(only_assets.clone()));
        assert_eq!(only_assets.calls("search"), 0);
        assert_eq!(last_art(&events, "tp").0.hero, Some(hero));
    }

    #[test]
    fn removed_games_lose_their_art_only_when_their_folder_is_there() {
        let s = setup();
        let (worker, _) = load(&s, Some(catalog()));
        let tp = ArtKey::from_stored("gc/GZ2E01");
        let oot = ArtKey::from_stored("n64/CZLE");
        drop(worker);

        let mut library = Library::open(&s.db).unwrap();
        // TP deleted from an existing gc folder; the n64 folder is unmounted.
        std::fs::remove_file(&s.games[0].path).unwrap();
        std::fs::remove_dir_all(s.root.join("n64")).unwrap();
        let removed = library.sync(&[]).unwrap();
        assert_eq!(removed.len(), 3);
        remove_orphaned_art(&library, &s.cache, &removed, &s.root);
        assert!(!s.cache.dir(&tp).exists());
        assert_eq!(library.art_status(&tp).unwrap(), None);
        assert!(s.cache.dir(&oot).exists());
        assert!(library.art_status(&oot).unwrap().is_some());
    }

    /// Refuses every logo request like a bad query would (HTTP 400).
    struct NoLogos(Arc<MockMetadataProvider>);

    impl MetadataProvider for NoLogos {
        fn search(
            &self,
            name: &str,
        ) -> Result<Vec<crate::art::provider::CatalogGame>, ProviderError> {
            self.0.search(name)
        }
        fn best_asset(
            &self,
            id: u64,
            kind: ArtKind,
        ) -> Result<Option<crate::art::provider::AssetRef>, ProviderError> {
            if kind == ArtKind::Logo {
                return Err(ProviderError::Http {
                    status: 400,
                    what: "/logos/game".into(),
                });
            }
            self.0.best_asset(id, kind)
        }
        fn assets(
            &self,
            id: u64,
            kind: ArtKind,
        ) -> Result<Vec<crate::art::provider::Asset>, ProviderError> {
            self.0.assets(id, kind)
        }
        fn download(
            &self,
            asset: &crate::art::provider::AssetRef,
        ) -> Result<Vec<u8>, ProviderError> {
            self.0.download(asset)
        }
    }

    #[test]
    fn a_kind_refused_for_good_is_skipped_not_queued() {
        let s = setup();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let mut worker = Worker::new(
            s.cache.clone(),
            s.picks.clone(),
            Library::open(&s.db).unwrap(),
            Some(Box::new(NoLogos(catalog()))),
            Box::new(move |e| sink.lock().unwrap().push(e)),
        );
        worker.load(s.games.clone());
        let events = events.lock().unwrap().clone();
        let (tp, _) = last_art(&events, "tp");
        assert!(tp.grid.is_some() && tp.hero.is_some());
        assert_eq!(tp.logo, None);
        assert!(worker.library.art_queue().unwrap().is_empty());

        assert!(
            !ProviderError::Http {
                status: 401,
                what: String::new()
            }
            .is_permanent()
        );
        assert!(
            !ProviderError::Http {
                status: 503,
                what: String::new()
            }
            .is_permanent()
        );
        assert!(!ProviderError::Network(String::new()).is_permanent());
    }
}

//! The art worker's side of the Customize page: candidates to choose from (cached, so they
//! show offline), applying one, and matching another game.
//!
//! Requests go through the worker thread one at a time, like the art fetch. After a network
//! error, the page's requests wait like the art queue does ([`art_retry_delay`]: 5 min,
//! doubling to 6 h) and answer from the cache meanwhile; any success ends the wait.
//!
//! Cache, next to the game's art: `<cache>/art/<key>/candidates/<kind>.json` (the list) and
//! `candidates/<kind>/<asset id>.<ext>` (thumbs); `candidates/games/<sgdb id>.<ext>` for
//! "Wrong game?" (the game list itself is in the library).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{ArtEvent, Job, Worker, unix_now};
use crate::art::identity::ArtKey;
use crate::art::provider::{ArtKind, Asset, AssetRef, ProviderError};
use crate::custom::Source;
use crate::extras::Extra;
use crate::library::{ArtCandidate, ArtStatus, art_retry_delay};

/// How many games "Wrong game?" offers.
const MAX_GAMES: usize = 8;

/// Which list of candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateSet {
    /// Images of one kind for the matched game.
    Art(ArtKind),
    /// Other games the match could be ("Wrong game?").
    Games,
}

impl CandidateSet {
    fn dir_name(self) -> &'static str {
        match self {
            Self::Art(kind) => kind.name(),
            Self::Games => "games",
        }
    }
}

/// One thing to choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// SteamGridDB asset id (images) or game id (games).
    pub id: u64,
    /// The game's name; for images, the size ("1920 × 620").
    pub name: String,
    /// Games: the search that found it.
    pub detail: String,
    /// The thumbnail, once cached.
    pub thumb: Option<PathBuf>,
    /// What is used now.
    pub current: bool,
}

/// Whether a list came fresh from SteamGridDB, or why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Fresh.
    Live,
    /// SteamGridDB can't be reached: saved results, if any; tried again after `retry_in`.
    Offline {
        /// Until the next try.
        retry_in: Duration,
    },
    /// SteamGridDB answered with an error (401/403: the key was refused; 429: too many
    /// requests): saved results, if any; tried again after `retry_in`.
    Refused {
        /// The HTTP status.
        status: u16,
        /// Until the next try.
        retry_in: Duration,
    },
    /// SteamGridDB's answer couldn't be read: saved results, if any.
    Unexpected,
    /// No SteamGridDB key (or provider): saved results only.
    NoProvider,
    /// The game has no match: nothing to list (use "Wrong game?" or a file).
    NotMatched,
}

/// The page's own backoff after network errors.
#[derive(Debug, Default)]
pub(super) struct NetBackoff {
    failures: u32,
    until: Option<Instant>,
    /// The HTTP status of the last failure (`None`: a network error).
    last: Option<u16>,
}

impl NetBackoff {
    /// `None` when a request may go out now, else how long until it may.
    fn waiting(&self) -> Option<Duration> {
        self.until
            .and_then(|until| until.checked_duration_since(Instant::now()))
            .filter(|left| !left.is_zero())
    }

    pub(super) fn failed(&mut self) {
        self.failed_with(None);
    }

    fn failed_with(&mut self, status: Option<u16>) {
        self.failures += 1;
        self.until = Some(Instant::now() + art_retry_delay(self.failures));
        self.last = status;
    }

    pub(super) fn succeeded(&mut self) {
        *self = Self::default();
    }
}

/// A listed image, as cached.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cached {
    id: u64,
    url: String,
    extension: String,
    thumb: String,
    thumb_extension: String,
    width: u32,
    height: u32,
}

impl From<&Asset> for Cached {
    fn from(a: &Asset) -> Self {
        Self {
            id: a.id,
            url: a.full.url.clone(),
            extension: a.full.extension.clone(),
            thumb: a.thumb.url.clone(),
            thumb_extension: a.thumb.extension.clone(),
            width: a.width,
            height: a.height,
        }
    }
}

impl From<Cached> for Asset {
    fn from(c: Cached) -> Self {
        Self {
            id: c.id,
            full: AssetRef {
                url: c.url,
                extension: c.extension,
            },
            thumb: AssetRef {
                url: c.thumb,
                extension: c.thumb_extension,
            },
            width: c.width,
            height: c.height,
        }
    }
}

/// Thumbnails still to fetch for a list.
#[derive(Debug)]
pub(super) enum ThumbWork {
    /// Images of a kind (or games' grids already looked up): id and thumb.
    Images(ArtKey, CandidateSet, Vec<(u64, AssetRef)>),
    /// Games whose best grid is still to look up.
    Games(ArtKey, Vec<u64>),
}

/// Why a provider call can't happen now.
enum Blocked {
    NoProvider,
    /// Backing off: how long still, and the HTTP status that started it (`None`: network).
    Waiting(Duration, Option<u16>),
}

impl From<Blocked> for Availability {
    fn from(b: Blocked) -> Self {
        match b {
            Blocked::NoProvider => Self::NoProvider,
            Blocked::Waiting(retry_in, None) => Self::Offline { retry_in },
            Blocked::Waiting(retry_in, Some(status)) => Self::Refused { status, retry_in },
        }
    }
}

impl Worker {
    fn may_call(&self) -> Result<(), Blocked> {
        if self.provider.is_none() {
            return Err(Blocked::NoProvider);
        }
        match self.net.waiting() {
            Some(left) => Err(Blocked::Waiting(left, self.net.last)),
            None => Ok(()),
        }
    }

    /// Notes a provider error; the availability to report for it. Like the art queue, every
    /// error but a permanent refusal waits before the next request.
    fn call_failed(&mut self, err: &ProviderError) -> Availability {
        tracing::warn!("SteamGridDB request for the Customize page failed: {err}");
        if !err.is_permanent() {
            let status = match err {
                ProviderError::Http { status, .. } => Some(*status),
                _ => None,
            };
            self.net.failed_with(status);
        }
        let retry_in = self.net.waiting().unwrap_or_default();
        match err {
            ProviderError::Network(_) => Availability::Offline { retry_in },
            ProviderError::Http { status, .. } => Availability::Refused {
                status: *status,
                retry_in,
            },
            ProviderError::Invalid(_) => Availability::Unexpected,
        }
    }

    fn thumb_dir(&self, key: &ArtKey, set: CandidateSet) -> PathBuf {
        self.cache.candidates_dir(key).join(set.dir_name())
    }

    fn cached_thumb(&self, key: &ArtKey, set: CandidateSet, id: u64) -> Option<PathBuf> {
        let dir = self.thumb_dir(key, set);
        ["png", "jpg", "jpeg", "webp", "gif"]
            .iter()
            .map(|ext| dir.join(format!("{id}.{ext}")))
            .find(|p| p.is_file())
    }

    fn list_file(&self, key: &ArtKey, kind: ArtKind) -> PathBuf {
        self.cache
            .candidates_dir(key)
            .join(format!("{}.json", kind.name()))
    }

    pub(super) fn candidates(&mut self, key: &ArtKey, set: CandidateSet) {
        match set {
            CandidateSet::Art(kind) => self.art_candidates(key, kind),
            CandidateSet::Games => self.game_candidates(key),
        }
    }

    fn art_candidates(&mut self, key: &ArtKey, kind: ArtKind) {
        let status = self.library.art_status(key).ok().flatten();
        let Some(ArtStatus::Matched { sgdb_id, .. }) = status else {
            self.send_list(
                key,
                CandidateSet::Art(kind),
                Vec::new(),
                Availability::NotMatched,
            );
            return;
        };
        let cached: Vec<Asset> = std::fs::read(self.list_file(key, kind))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<Cached>>(&bytes).ok())
            .map(|list| list.into_iter().map(Asset::from).collect())
            .unwrap_or_default();
        let (assets, availability) = match self.may_call() {
            Err(blocked) => (cached, blocked.into()),
            Ok(()) => match self.provider_assets(sgdb_id, kind) {
                Ok(fresh) => {
                    self.net.succeeded();
                    self.save_list(key, kind, &fresh);
                    (fresh, Availability::Live)
                }
                Err(err) => (cached, self.call_failed(&err)),
            },
        };
        self.listed
            .insert((key.clone(), kind), (assets.clone(), availability));
        self.send_art_list(key, kind);
        let set = CandidateSet::Art(kind);
        if availability == Availability::Live {
            let missing: Vec<(u64, AssetRef)> = assets
                .into_iter()
                .filter(|a| self.cached_thumb(key, set, a.id).is_none())
                .map(|a| (a.id, a.thumb))
                .collect();
            self.fetch_thumbs(key, set, missing);
        }
    }

    /// Sends the images listed last for `key`'s `kind`, marking the one in use: the picked
    /// SteamGridDB image, or the automatic one when it is known which it was (art fetched
    /// before Hearth noted its source can't be told apart: then none is marked).
    pub(super) fn send_art_list(&self, key: &ArtKey, kind: ArtKind) {
        let Some((assets, availability)) = self.listed.get(&(key.clone(), kind)) else {
            return;
        };
        let custom = self.picks.load(key);
        let entry = custom.entry(Extra::Art(kind));
        let auto_url = self.cache.source_url(key, kind);
        let items = assets
            .iter()
            .map(|a| Candidate {
                id: a.id,
                name: if a.width > 0 {
                    format!("{} × {}", a.width, a.height)
                } else {
                    String::new()
                },
                detail: String::new(),
                thumb: self.cached_thumb(key, CandidateSet::Art(kind), a.id),
                current: match entry.source {
                    Some(Source::Sgdb) => entry.asset == Some(a.id),
                    Some(Source::File | Source::None) => false,
                    None => auto_url.as_deref() == Some(a.full.url.as_str()),
                },
            })
            .collect();
        self.send_list(key, CandidateSet::Art(kind), items, *availability);
    }

    /// Thumbnails put off earlier.
    pub(super) fn resume_thumbs(&mut self, work: ThumbWork) {
        match work {
            ThumbWork::Images(key, set, thumbs) => self.fetch_thumbs(&key, set, thumbs),
            ThumbWork::Games(key, ids) => self.game_thumbs(&key, ids),
        }
    }

    fn provider_assets(&self, sgdb_id: u64, kind: ArtKind) -> Result<Vec<Asset>, ProviderError> {
        match self.provider.as_deref() {
            Some(provider) => provider.assets(sgdb_id, kind),
            None => Ok(Vec::new()),
        }
    }

    fn save_list(&self, key: &ArtKey, kind: ArtKind, assets: &[Asset]) {
        let list: Vec<Cached> = assets.iter().map(Cached::from).collect();
        let path = self.list_file(key, kind);
        let saved = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::write(
                    &path,
                    serde_json::to_vec(&list).map_err(std::io::Error::other)?,
                )
            });
        if let Err(err) = saved {
            tracing::warn!(path = %path.display(), "can't cache the candidates: {err}");
        }
    }

    fn game_candidates(&mut self, key: &ArtKey) {
        let set = CandidateSet::Games;
        let matched = match self.library.art_status(key).ok().flatten() {
            Some(ArtStatus::Matched { sgdb_id, .. }) => Some(sgdb_id),
            _ => None,
        };
        let names = self
            .games
            .get(key)
            .map(|k| k.identity.search_names.clone())
            .unwrap_or_default();
        let availability = match self.may_call() {
            Err(blocked) => blocked.into(),
            Ok(()) => match self.search_all(&names) {
                Ok(found) => {
                    self.net.succeeded();
                    if let Err(err) = self.library.set_art_candidates(key, &found) {
                        tracing::warn!("can't save the match candidates of {key}: {err}");
                    }
                    Availability::Live
                }
                Err(err) => self.call_failed(&err),
            },
        };
        let mut found = self.library.art_candidates(key).unwrap_or_default();
        found.truncate(MAX_GAMES);
        let items: Vec<Candidate> = found
            .iter()
            .map(|c| Candidate {
                id: c.sgdb_id,
                name: c.name.clone(),
                detail: c.query.clone(),
                thumb: self.cached_thumb(key, set, c.sgdb_id),
                current: matched == Some(c.sgdb_id),
            })
            .collect();
        self.send_list(key, set, items.clone(), availability);
        if availability != Availability::Live {
            return;
        }
        let missing = items
            .into_iter()
            .filter(|i| i.thumb.is_none())
            .map(|i| i.id)
            .collect();
        self.game_thumbs(key, missing);
    }

    /// Each game's best grid, as its thumbnail.
    fn game_thumbs(&mut self, key: &ArtKey, ids: Vec<u64>) {
        for (i, id) in ids.iter().enumerate() {
            if self.superseded() {
                self.backlog = Some(ThumbWork::Games(key.clone(), ids[i..].to_vec()));
                return;
            }
            let grid = match self.provider_assets(*id, ArtKind::Grid) {
                Ok(grids) => grids.into_iter().next(),
                Err(err) => {
                    self.call_failed(&err);
                    return;
                }
            };
            if let Some(grid) = grid {
                self.fetch_thumbs(key, CandidateSet::Games, vec![(*id, grid.thumb)]);
            }
        }
    }

    /// Every search name's results, best first, each game once.
    fn search_all(&self, names: &[String]) -> Result<Vec<ArtCandidate>, ProviderError> {
        let Some(provider) = self.provider.as_deref() else {
            return Ok(Vec::new());
        };
        let mut found: Vec<ArtCandidate> = Vec::new();
        for name in names {
            for game in provider.search(name)? {
                if !found.iter().any(|f| f.sgdb_id == game.id) {
                    found.push(ArtCandidate {
                        sgdb_id: game.id,
                        name: game.name,
                        query: name.clone(),
                    });
                }
            }
        }
        Ok(found)
    }

    /// Downloads thumbs one by one, reporting each. Gives way to any request (the rest is put
    /// off, see [`Worker::next_command`]); stops when the network fails.
    fn fetch_thumbs(&mut self, key: &ArtKey, set: CandidateSet, thumbs: Vec<(u64, AssetRef)>) {
        for (i, (id, thumb)) in thumbs.iter().cloned().enumerate() {
            if self.superseded() {
                self.backlog = Some(ThumbWork::Images(key.clone(), set, thumbs[i..].to_vec()));
                return;
            }
            let Some(provider) = self.provider.as_deref() else {
                return;
            };
            let bytes = match provider.download(&thumb) {
                Ok(bytes) if image::guess_format(&bytes).is_ok() => bytes,
                Ok(_) => continue,
                Err(err) => {
                    self.call_failed(&err);
                    return;
                }
            };
            let dir = self.thumb_dir(key, set);
            let path = dir.join(format!("{id}.{}", thumb.extension));
            let stored = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes));
            match stored {
                Ok(()) => (self.events)(ArtEvent::Thumb {
                    key: key.clone(),
                    set,
                    id,
                    path,
                }),
                Err(err) => tracing::warn!(path = %path.display(), "can't cache a thumb: {err}"),
            }
        }
    }

    fn send_list(
        &self,
        key: &ArtKey,
        set: CandidateSet,
        items: Vec<Candidate>,
        availability: Availability,
    ) {
        (self.events)(ArtEvent::Candidates {
            key: key.clone(),
            set,
            items,
            availability,
        });
    }

    /// Downloads the listed image `asset` and makes it `key`'s `kind`.
    pub(super) fn apply(&mut self, key: &ArtKey, kind: ArtKind, asset: u64) {
        let listed = self
            .listed
            .get(&(key.clone(), kind))
            .and_then(|(list, _)| list.iter().find(|a| a.id == asset))
            .cloned();
        let Some(asset) = listed else {
            tracing::warn!(%key, kind = kind.name(), asset, "apply: not among the listed images");
            self.changed(
                key,
                Err(std::io::Error::other("unknown image")),
                "Couldn't use it",
            );
            return;
        };
        let downloaded = match (self.may_call(), self.provider.as_deref()) {
            (Ok(()), Some(provider)) => provider.download(&asset.full),
            _ => {
                let err = std::io::Error::other("SteamGridDB can't be reached");
                self.changed(key, Err(err), "Can't reach SteamGridDB");
                return;
            }
        };
        let bytes = match downloaded {
            Ok(bytes) if image::guess_format(&bytes).is_ok() => {
                self.net.succeeded();
                bytes
            }
            Ok(_) => {
                let err = std::io::Error::other("not an image");
                self.changed(key, Err(err), "Couldn't download the image");
                return;
            }
            Err(err) => {
                self.call_failed(&err);
                let err = std::io::Error::other(err.to_string());
                self.changed(key, Err(err), "Couldn't download the image");
                return;
            }
        };
        let done = self
            .picks
            .pick_asset(key, kind, asset.id, &asset.full.extension, &bytes)
            .map(drop);
        self.changed(key, done, "Couldn't save the image");
    }

    /// Matches `key` to another SteamGridDB game and fetches its automatic art.
    pub(super) fn rematch(&mut self, key: &ArtKey, sgdb_id: u64, name: String) {
        tracing::info!(%key, sgdb_id, %name, "matched by the user");
        if let Err(err) = self.picks.forget_match(key) {
            tracing::warn!(%key, "can't drop the old match's picks: {err}");
        }
        // The old match's art, its candidates and display copies all go.
        if let Err(err) = self.cache.remove(key) {
            tracing::warn!(%key, "can't drop the old match's art: {err}");
        }
        let status = ArtStatus::Matched {
            sgdb_id,
            sgdb_name: name.clone(),
            kinds: Vec::new(),
        };
        if let Err(err) = self.library.set_art_status(key, &status, unix_now()) {
            tracing::warn!(%key, "can't save the new match: {err}");
            self.changed(
                key,
                Err(std::io::Error::other(err.to_string())),
                "Couldn't change the match",
            );
            return;
        }
        self.listed.retain(|(k, _), _| k != key);
        // Placeholders until the new art is in (or the queue gets it later).
        self.changed(key, Ok(()), "");
        let job = Job::Missing {
            sgdb_id,
            sgdb_name: name,
            kinds: ArtKind::ALL.to_vec(),
        };
        if self.provider.is_some() {
            self.run(vec![(key.clone(), job)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::art::MockMetadataProvider;
    use crate::art::service::Command;
    use crate::art::service::tests::{Setup, setup};
    use crate::custom::Lookup;
    use crate::library::Library;

    struct Run {
        worker: Worker,
        events: Arc<Mutex<Vec<ArtEvent>>>,
    }

    impl Run {
        fn new(s: &Setup, provider: Arc<MockMetadataProvider>) -> Self {
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
            Self { worker, events }
        }

        fn take(&self) -> Vec<ArtEvent> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }

        fn list(&self, want: CandidateSet) -> (Vec<Candidate>, Availability) {
            self.take()
                .into_iter()
                .find_map(|e| match e {
                    ArtEvent::Candidates {
                        set,
                        items,
                        availability,
                        ..
                    } if set == want => Some((items, availability)),
                    _ => None,
                })
                .expect("a candidates event")
        }
    }

    fn tp() -> ArtKey {
        ArtKey::from_stored("gc/GZ2E01")
    }

    #[test]
    fn heroes_are_listed_with_thumbs_and_the_automatic_one_marked() {
        let s = setup();
        let mut run = Run::new(&s, Arc::new(MockMetadataProvider::fixtures()));
        run.take();
        let set = CandidateSet::Art(ArtKind::Hero);
        run.worker.candidates(&tp(), set);
        let events = run.take();
        let (items, availability) = events
            .iter()
            .find_map(|e| match e {
                ArtEvent::Candidates {
                    items,
                    availability,
                    ..
                } => Some((items.clone(), *availability)),
                _ => None,
            })
            .unwrap();
        assert_eq!(availability, Availability::Live);
        assert_eq!(items.len(), 6);
        assert_eq!(items.iter().filter(|i| i.current).count(), 1);
        assert!(items[0].current, "the automatic hero is the best one");
        let thumbs = events
            .iter()
            .filter(|e| matches!(e, ArtEvent::Thumb { .. }))
            .count();
        assert_eq!(thumbs, 6);

        // Applying one: it becomes the hero, marked current.
        let third = items[2].id;
        run.worker.apply(&tp(), ArtKind::Hero, third);
        assert!(matches!(
            s.picks.find(&tp(), Extra::Art(ArtKind::Hero)),
            Lookup::Found(_)
        ));
        let events = run.take();
        assert!(events.iter().any(|e| matches!(e, ArtEvent::Custom { .. })));
        run.worker.candidates(&tp(), set);
        let (items, _) = run.list(set);
        assert!(items[2].current && !items[0].current);
        assert!(
            items.iter().all(|i| i.thumb.is_some()),
            "thumbs come from the cache"
        );
    }

    #[test]
    fn offline_shows_saved_candidates_and_waits_before_trying_again() {
        let s = setup();
        let mut run = Run::new(&s, Arc::new(MockMetadataProvider::fixtures()));
        let set = CandidateSet::Art(ArtKind::Logo);
        run.worker.candidates(&tp(), set);
        run.take();

        let offline = Arc::new(MockMetadataProvider::offline());
        run.worker.provider = Some(Box::new(offline.clone()));
        run.worker.candidates(&tp(), set);
        let (items, availability) = run.list(set);
        assert_eq!(items.len(), 6);
        assert!(
            matches!(availability, Availability::Offline { retry_in } if retry_in > Duration::ZERO)
        );
        let calls = offline.calls("assets");
        run.worker.candidates(&tp(), set);
        assert_eq!(offline.calls("assets"), calls, "backing off");
        assert!(matches!(run.list(set).1, Availability::Offline { .. }));

        // Nothing saved for another kind: an empty list, still offline.
        run.worker
            .candidates(&tp(), CandidateSet::Art(ArtKind::Icon));
        let (items, availability) = run.list(CandidateSet::Art(ArtKind::Icon));
        assert!(items.is_empty() && matches!(availability, Availability::Offline { .. }));
    }

    /// Matches like the fixtures, but SteamGridDB refuses every image list (a bad key).
    struct RefusesImages(MockMetadataProvider);
    impl crate::art::MetadataProvider for RefusesImages {
        fn search(&self, name: &str) -> Result<Vec<crate::art::CatalogGame>, ProviderError> {
            self.0.search(name)
        }
        fn best_asset(&self, id: u64, kind: ArtKind) -> Result<Option<AssetRef>, ProviderError> {
            self.0.best_asset(id, kind)
        }
        fn assets(&self, _: u64, _: ArtKind) -> Result<Vec<Asset>, ProviderError> {
            Err(ProviderError::Http {
                status: 401,
                what: "/heroes/game".into(),
            })
        }
        fn download(&self, asset: &AssetRef) -> Result<Vec<u8>, ProviderError> {
            self.0.download(asset)
        }
    }

    #[test]
    fn a_refused_key_is_not_reported_as_offline() {
        let s = setup();
        let mut run = Run::new(&s, Arc::new(MockMetadataProvider::fixtures()));
        run.worker.provider = Some(Box::new(RefusesImages(MockMetadataProvider::fixtures())));
        let set = CandidateSet::Art(ArtKind::Hero);
        run.worker.candidates(&tp(), set);
        let (_, availability) = run.list(set);
        assert!(matches!(
            availability,
            Availability::Refused { status: 401, .. }
        ));
        // It waits before asking again, and says why meanwhile.
        run.worker
            .candidates(&tp(), CandidateSet::Art(ArtKind::Logo));
        let (_, availability) = run.list(CandidateSet::Art(ArtKind::Logo));
        assert!(
            matches!(availability, Availability::Refused { status: 401, retry_in } if retry_in > Duration::ZERO)
        );
    }

    #[test]
    fn art_of_unknown_origin_is_not_claimed_and_changes_remark_the_list() {
        let s = setup();
        let mut run = Run::new(&s, Arc::new(MockMetadataProvider::fixtures()));
        // Art cached before Hearth noted where it came from.
        std::fs::remove_file(s.cache.dir(&tp()).join("hero.url")).unwrap();
        let set = CandidateSet::Art(ArtKind::Hero);
        run.worker.candidates(&tp(), set);
        let (items, _) = run.list(set);
        assert!(
            items.iter().all(|i| !i.current),
            "can't tell which one it is"
        );

        // A change sends the list again, marked, without asking SteamGridDB.
        run.worker.apply(&tp(), ArtKind::Hero, items[4].id);
        let (items, _) = run.list(set);
        assert!(items[4].current);
        run.worker
            .handle(Command::Reset(tp(), Extra::Art(ArtKind::Hero)));
        let (items, _) = run.list(set);
        assert!(
            items.iter().all(|i| !i.current),
            "automatic again, still unknown"
        );
    }

    #[test]
    fn requests_go_before_thumbnails_which_resume_afterwards() {
        let s = setup();
        let mut run = Run::new(&s, Arc::new(MockMetadataProvider::fixtures()));
        let set = CandidateSet::Art(ArtKind::Logo);
        let (tx, rx) = std::sync::mpsc::channel();
        run.worker.inbox = Some(rx);
        // Listed once without thumbs (the list file is cached), then a pick waits.
        run.worker.listed.clear();
        tx.send(Command::HeroPosition(tp(), 0.3)).unwrap();
        run.worker.candidates(&tp(), set);
        let thumbs = |events: &[ArtEvent]| {
            events
                .iter()
                .filter(|e| matches!(e, ArtEvent::Thumb { .. }))
                .count()
        };
        assert_eq!(thumbs(&run.take()), 0, "the waiting request goes first");
        let next = run.worker.next_command().unwrap();
        assert!(matches!(next, Command::HeroPosition(..)));
        run.worker.handle(next);
        let next = run.worker.next_command().unwrap();
        assert!(matches!(next, Command::Thumbs(_)), "then the thumbs resume");
        run.worker.handle(next);
        assert_eq!(thumbs(&run.take()), 6);
    }

    #[test]
    fn a_game_without_a_match_has_no_images_to_list() {
        let s = setup();
        let mut run = Run::new(&s, Arc::new(MockMetadataProvider::new()));
        let hb = ArtKey::from_stored("n64/HBTE");
        run.worker.candidates(&hb, CandidateSet::Art(ArtKind::Grid));
        let (items, availability) = run.list(CandidateSet::Art(ArtKind::Grid));
        assert!(items.is_empty());
        assert_eq!(availability, Availability::NotMatched);
    }

    #[test]
    fn rematching_replaces_the_art_and_drops_only_steamgriddb_picks() {
        let s = setup();
        let mut run = Run::new(&s, Arc::new(MockMetadataProvider::fixtures()));
        run.worker
            .candidates(&tp(), CandidateSet::Art(ArtKind::Hero));
        let first = match &run.list(CandidateSet::Art(ArtKind::Hero)).0[..] {
            [first, ..] => first.id,
            [] => panic!("no heroes"),
        };
        run.worker.apply(&tp(), ArtKind::Hero, first);
        let logo = s.root.join("logo.png");
        std::fs::write(&logo, b"png").unwrap();
        s.picks
            .pick_file(&tp(), Extra::Art(ArtKind::Logo), &logo)
            .unwrap();
        s.picks.set_hero_position(&tp(), 0.1).unwrap();
        let old_grid = s.cache.cached(&tp(), ArtKind::Grid).unwrap();
        let old_bytes = std::fs::read(&old_grid).unwrap();

        run.worker.candidates(&tp(), CandidateSet::Games);
        let (games, availability) = run.list(CandidateSet::Games);
        assert_eq!(availability, Availability::Live);
        assert!(games.len() > 2);
        assert_eq!(games.iter().filter(|g| g.current).count(), 1);
        let other = games.iter().find(|g| !g.current).unwrap().clone();
        assert!(run.take().is_empty(), "thumbs were sent with the list");

        run.worker.rematch(&tp(), other.id, other.name.clone());
        let status = run.worker.library.art_status(&tp()).unwrap().unwrap();
        assert!(matches!(status, ArtStatus::Matched { sgdb_id, .. } if sgdb_id == other.id));
        assert_eq!(s.picks.find(&tp(), Extra::Art(ArtKind::Hero)), Lookup::Pass);
        assert!(matches!(
            s.picks.find(&tp(), Extra::Art(ArtKind::Logo)),
            Lookup::Found(_)
        ));
        assert_eq!(s.picks.load(&tp()).hero_position(), crate::custom::CENTER);
        let new_grid = s.cache.cached(&tp(), ArtKind::Grid).unwrap();
        assert_ne!(std::fs::read(new_grid).unwrap(), old_bytes);
        assert!(!s.cache.candidates_dir(&tp()).join("hero.json").exists());
    }
}

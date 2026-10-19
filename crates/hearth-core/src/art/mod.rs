//! Per-game artwork: identification, the SteamGridDB provider, the on-disk cache and the
//! worker that ties them together. No network code lives in the UI crate.

pub mod cache;
pub mod identity;
pub mod provider;
pub mod steamgriddb;

pub use cache::{ArtCache, GameArt};
pub use identity::{ArtKey, Identity, identify};
pub use provider::{ArtKind, MetadataProvider, MockMetadataProvider, ProviderError};
pub use steamgriddb::SteamGridDb;

//! Per-game artwork: identification, the SteamGridDB provider, the on-disk cache and the
//! worker that ties them together. No network code lives in the UI crate.

pub mod cache;
pub mod display;
pub mod identity;
pub mod provider;
pub mod service;
pub mod steamgriddb;

pub use cache::{ArtCache, GameArt};
pub use display::DisplayClass;
pub use identity::{ArtKey, Identity, identify};
pub use provider::{
    ArtKind, Asset, AssetRef, CatalogGame, MetadataProvider, MockMetadataProvider, ProviderError,
};
pub use service::{ArtEvent, ArtService, remove_orphaned_art};
pub use steamgriddb::SteamGridDb;

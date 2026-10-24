//! Platform-neutral core of Hearth: game model, systems, library and runtimes.
//!
//! No Slint and no D-Bus in this crate.
//!
//! TODO(M2): ES-DE import for all systems, `sources` (LibrarySource + steam/ports).

pub mod art;
pub mod config;
pub mod controls;
pub mod custom;
pub mod extras;
pub mod files;
pub mod library;
pub mod model;
pub mod packs;
pub mod paths;
pub mod profiles;
pub mod rom_header;
pub mod runtime;
pub mod scan;
pub mod session;
pub mod systems;
pub mod title;
pub mod users;

#[cfg(test)]
pub(crate) mod testdir;

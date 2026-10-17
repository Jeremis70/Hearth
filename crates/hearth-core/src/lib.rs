//! Platform-neutral core of Hearth: game model, systems, library and runtimes.
//!
//! No Slint and no D-Bus in this crate.
//!
//! TODO(M2): ES-DE import for all systems, `sources` (LibrarySource + steam/ports).

pub mod demo;
pub mod library;
pub mod model;
pub mod paths;
pub mod scan;
pub mod systems;
pub mod title;

#[cfg(test)]
pub(crate) mod testdir;

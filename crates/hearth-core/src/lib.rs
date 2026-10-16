//! Platform-neutral core of Hearth: game model, systems, library and runtimes.
//!
//! No Slint and no D-Bus in this crate.
//!
//! TODO(M2): `systems` (load systems.toml), `sources` (LibrarySource + roms/steam/ports),
//! `library` (SQLite cache), `runtime` (Runtime trait, ProcessRuntime), `profiles`.

pub mod demo;
pub mod model;
pub mod paths;

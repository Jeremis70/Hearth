//! Emulator settings: which config keys Hearth owns, which belong to the user, and how both
//! reach the emulator's own files at launch (design README §9 "Emulator settings").

pub mod doc;
pub mod gpu;
pub mod schema;
pub mod store;

pub use doc::{ConfigDoc, Style};
pub use gpu::Gpu;
pub use schema::{Schema, Scope, Setting};

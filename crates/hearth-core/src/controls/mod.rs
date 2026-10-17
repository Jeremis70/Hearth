//! Controller mapping: one Hearth-owned format, layered per system and per game, translated
//! into each emulator's own config at launch (`profiles::*_input`).
//!
//! Files describe the Hearth pad by button position, never by brand letters; see
//! `data/controls/*.toml` for the shipped defaults.

pub mod defaults;
pub mod device;
pub mod file;
pub mod key;
pub mod layers;
pub mod model;

pub use device::{ControllerIdentity, ControllerKind};
pub use file::Problem;
pub use key::GameKey;
pub use layers::{Resolved, Source, resolve, user_dir};
pub use model::{EmulatorAction, Mapping, PadInput, StickOptions, Target, TriggerMode};

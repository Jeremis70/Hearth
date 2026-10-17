//! Per-emulator profiles: settings that make every emulator behave like part of one console
//! (fullscreen, no emulator UI or hotkeys, saves under Hearth's data directory).

pub mod dolphin;
pub mod dolphin_input;
pub mod input;
pub mod retroarch;
pub mod retroarch_input;

pub use dolphin::{DolphinInstall, DolphinProfile};
pub use retroarch::RetroArchProfile;

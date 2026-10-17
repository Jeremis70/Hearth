//! Per-emulator profiles: settings that make every emulator behave like part of one console
//! (fullscreen, no emulator UI or hotkeys, saves under Hearth's data directory).

pub mod dolphin;
pub mod retroarch;

pub use dolphin::{DolphinInstall, DolphinProfile};
pub use retroarch::RetroArchProfile;

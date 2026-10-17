//! Per-emulator profiles: settings that make every emulator behave like part of one console
//! (fullscreen, no emulator UI or hotkeys, saves under Hearth's data directory).

pub mod retroarch;

pub use retroarch::RetroArchProfile;

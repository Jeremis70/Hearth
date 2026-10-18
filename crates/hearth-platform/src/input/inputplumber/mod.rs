//! Controller input through the InputPlumber daemon: it owns the physical controllers, gives
//! games a virtual pad, and lets Hearth take all input over D-Bus while its own screens are
//! shown. See `docs/spikes/inputplumber.md`.

pub mod client;

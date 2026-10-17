//! Linux integrations for Hearth, each behind a trait with a mock implementation.
//!
//! TODO(M1): real `network` backend (nmrs), `power` (logind-zbus).
//! TODO(M3): InputPlumber input backend (zbus).

pub mod gamescope;
pub mod input;
pub mod network;

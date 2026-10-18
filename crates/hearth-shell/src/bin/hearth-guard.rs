//! Hearth's input guard (see `src/guard.rs`): waits for Hearth to go away, then hands the
//! controllers back to InputPlumber unless Hearth said it already did.

use std::io::Read;

use hearth_platform::input::inputplumber;
use tracing_subscriber::EnvFilter;

/// What Hearth writes when it has released the controllers itself (`guard::STAND_DOWN`).
const STAND_DOWN: &str = "released\n";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let mut message = String::new();
    // Ends when Hearth closes the pipe or dies.
    let _ = std::io::stdin().read_to_string(&mut message);
    if message == STAND_DOWN {
        return;
    }
    tracing::warn!("Hearth ended without handing the controllers back; doing it now");
    match inputplumber::release_all() {
        Ok(()) => tracing::info!("controllers handed back to InputPlumber"),
        Err(err) => tracing::error!("handing the controllers back failed: {err}"),
    }
}

//! Hearth shell: the Slint UI and the glue to core and platform services.

mod bridge;

use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use hearth_core::paths::DataDirs;
use hearth_platform::input::gilrs::GilrsBackend;
use hearth_platform::network::MockNetworkBackend;
use slint::ComponentHandle;
use tracing_subscriber::EnvFilter;

slint::include_modules!();

/// Hearth, a controller-first living-room console.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Run in a resizable window (development).
    #[arg(long, conflicts_with = "fullscreen")]
    windowed: bool,
    /// Run fullscreen (the default).
    #[arg(long)]
    fullscreen: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();

    let dirs = DataDirs::from_env().context("resolving data directories")?;
    dirs.create_all()?;
    tracing::info!(data = %dirs.data().display(), "using data directories");

    // gamescope is the compositor on the appliance, so always use winit, never LinuxKMS.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .select()
        .context("selecting the winit backend")?;

    let app = AppWindow::new()?;
    app.window().set_fullscreen(!args.windowed);

    bridge::models::install(&app);
    bridge::models::load_network(&app, MockNetworkBackend::new());

    let clock = slint::Timer::default();
    update_clock(&app);
    let weak = app.as_weak();
    clock.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            if let Some(app) = weak.upgrade() {
                update_clock(&app);
            }
        },
    );

    if let Err(err) = bridge::input::start(&app, GilrsBackend::new()) {
        tracing::warn!("controller input unavailable, keyboard only: {err:#}");
    }

    app.run()?;
    Ok(())
}

fn update_clock(app: &AppWindow) {
    let now = chrono::Local::now().format("%-I:%M %p").to_string();
    app.global::<Status>().set_clock(now.into());
}

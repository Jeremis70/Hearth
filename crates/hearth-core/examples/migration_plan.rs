//! Prints what the move into profiles would do in `HEARTH_HOME`, without changing anything:
//! every rename, the backup's size and the free space it needs.
//!
//! ```text
//! HEARTH_HOME=./.dev-data cargo run -p hearth-core --example migration_plan
//! ```

use hearth_core::paths::DataDirs;
use hearth_core::users::migrate::Migrator;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dirs = DataDirs::from_env()?;
    let Some(plan) = Migrator::new(&dirs).plan()? else {
        println!("Nothing to migrate (new install, done already, or under way: see the marker).");
        return Ok(());
    };
    println!("Moves ({}):", plan.moves.len());
    for m in &plan.moves {
        println!(
            "  {}\n    {}\n    → {}",
            m.what,
            m.from.display(),
            m.to.display()
        );
    }
    println!(
        "Backup: {} files, {} bytes ({:.1} MiB); needs {:.1} MiB free",
        plan.backup_files.len(),
        plan.backup_bytes,
        mib(plan.backup_bytes),
        mib(plan.backup_bytes * 2)
    );
    println!("Not backed up (moved unchanged):");
    for item in &plan.not_backed_up {
        println!("  {} ({:.1} MiB)", item.path.display(), mib(item.bytes));
    }
    for dir in &plan.unattributed {
        println!(
            "Saves of an unknown core, left in the RetroArch folder: {}",
            dir.display()
        );
    }
    println!(
        "Library upgrade to per-profile history: {}",
        plan.library_upgrade
    );
    Ok(())
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

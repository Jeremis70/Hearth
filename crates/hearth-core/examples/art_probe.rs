//! Asks SteamGridDB what Hearth would pick for a name or a ROM, without downloading or
//! writing anything.
//!
//! `cargo run -p hearth-core --example art_probe -- "<name>"`
//! `cargo run -p hearth-core --example art_probe -- --rom <system> <path>`
//!
//! The key comes from HEARTH_STEAMGRIDDB_KEY, else `<HEARTH_HOME or XDG config>/hearth.toml`.

use std::path::Path;

use hearth_core::art::identity::{identify, pick_match};
use hearth_core::art::{ArtKind, MetadataProvider, SteamGridDb};
use hearth_core::config::{Config, STEAMGRIDDB_KEY_ENV};
use hearth_core::model::SystemId;
use hearth_core::paths::DataDirs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let names = match args.as_slice() {
        [flag, system, path] if flag == "--rom" => {
            let identity = identify(&SystemId::new(system.as_str()), Path::new(path));
            println!(
                "key: {}, search names: {:?}",
                identity.key, identity.search_names
            );
            identity.search_names
        }
        [name] => vec![name.clone()],
        _ => return Err("usage: art_probe <name> | --rom <system> <path>".into()),
    };
    let config = Config::load(&DataDirs::from_env()?.config_file())?;
    let key = config
        .steamgriddb_key(std::env::var(STEAMGRIDDB_KEY_ENV).ok())
        .unwrap_or_default();
    let provider = SteamGridDb::new(key);
    for name in &names {
        let results = provider.search(name)?;
        println!("search {name:?}: {} results", results.len());
        for game in results.iter().take(5) {
            println!("  {} {}", game.id, game.name);
        }
        let Some(hit) = pick_match(&results, name, |g| g.name.as_str()) else {
            println!("  no match");
            continue;
        };
        println!("match: {} {}", hit.id, hit.name);
        for kind in ArtKind::ALL {
            match provider.best_asset(hit.id, kind) {
                Ok(Some(asset)) => println!("  {}: {}", kind.name(), asset.url),
                Ok(None) => println!("  {}: none", kind.name()),
                Err(err) => println!("  {}: error: {err}", kind.name()),
            }
        }
        return Ok(());
    }
    Ok(())
}

//! Asks SteamGridDB what Hearth would pick for a name or a ROM, without downloading or
//! writing anything.
//!
//! `cargo run -p hearth-core --example art_probe -- "<name>"`
//! `cargo run -p hearth-core --example art_probe -- --rom <system> <path>`
//! `cargo run -p hearth-core --example art_probe -- --candidates <system> <path>`: what the
//! Customize page would list ("Wrong game?" games, then the match's heroes).
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
    let candidates = args.first().is_some_and(|a| a == "--candidates");
    let names = match args.as_slice() {
        [flag, system, path] if flag == "--rom" || flag == "--candidates" => {
            let identity = identify(&SystemId::new(system.as_str()), Path::new(path));
            println!(
                "key: {}, search names: {:?}",
                identity.key, identity.search_names
            );
            identity.search_names
        }
        [name] => vec![name.clone()],
        _ => {
            return Err(
                "usage: art_probe <name> | --rom <system> <path> | --candidates <system> <path>"
                    .into(),
            );
        }
    };
    let config = Config::load(&DataDirs::from_env()?.config_file())?;
    let key = config
        .steamgriddb_key(std::env::var(STEAMGRIDDB_KEY_ENV).ok())
        .unwrap_or_default();
    let provider = SteamGridDb::new(key);
    if candidates {
        return list_candidates(&provider, &names);
    }
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

/// "Wrong game?": every search's games, each once (as the art worker lists them); then the
/// heroes of the match.
fn list_candidates(
    provider: &SteamGridDb,
    names: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut seen = Vec::new();
    let mut matched = None;
    println!("Wrong game? candidates:");
    for name in names {
        let results = provider.search(name)?;
        if matched.is_none() {
            matched = pick_match(&results, name, |g| g.name.as_str()).cloned();
        }
        for game in results {
            if !seen.contains(&game.id) {
                seen.push(game.id);
                if seen.len() <= 8 {
                    println!("  {} {} (searching {name:?})", game.id, game.name);
                }
            }
        }
    }
    println!("  ({} in all, the page shows 8)", seen.len());
    let Some(hit) = matched else {
        println!("no match: no heroes to list");
        return Ok(());
    };
    let heroes = provider.assets(hit.id, ArtKind::Hero)?;
    println!("heroes of {} {}: {}", hit.id, hit.name, heroes.len());
    for hero in heroes.iter().take(8) {
        println!(
            "  {} {}x{} ({:.2}:1) thumb .{}",
            hero.id,
            hero.width,
            hero.height,
            f64::from(hero.width) / f64::from(hero.height.max(1)),
            hero.thumb.extension
        );
    }
    Ok(())
}

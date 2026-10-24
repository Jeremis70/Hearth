use std::ops::ControlFlow;

use super::*;
use crate::testdir::tempdir;

fn write(path: PathBuf, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A home shaped like the dev data before profiles, with fake saves.
fn old_home(home: &Path) {
    let ra = home.join("retroarch");
    write(
        ra.join("saves/Mupen64Plus-Next/Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!].srm"),
        b"oot battery save",
    );
    write(ra.join("saves/Some-Other-Core/Game.srm"), b"other core");
    write(ra.join("states/Mupen64Plus-Next/OoT.state"), b"oot state");
    write(ra.join("system/Mupen64plus/mupen64plus.ini"), b"bios-ish");
    write(
        ra.join("config/retroarch-core-options.cfg"),
        b"core = \"1\"\n",
    );
    write(
        ra.join("remaps/Mupen64Plus-Next/Mupen64Plus-Next.rmp"),
        b"remap",
    );
    write(ra.join("logs/retroarch.log"), b"log");
    write(ra.join("cache/x"), b"cache");
    write(ra.join("autoconfig/sdl2/Pad.cfg"), b"pad");
    write(
        ra.join("retroarch.cfg"),
        format!(
            "savefile_directory = \"{}/saves\"\nlog_dir = \"./.dev-data/retroarch/logs\"\n",
            ra.display()
        )
        .as_bytes(),
    );
    let dol = home.join("dolphin");
    write(
        dol.join("GC/USA/Card A/01-GZ2E-gczelda2.gci"),
        b"tp gci save",
    );
    fs::create_dir_all(dol.join("GC/JAP")).unwrap();
    fs::create_dir_all(dol.join("GC/EUR")).unwrap();
    write(dol.join("GC/SRAM.raw"), b"sram");
    write(dol.join("Wii/shared2/sys/SYSCONF"), b"sysconf");
    write(dol.join("StateSaves/GZ2E01.s01"), b"tp state");
    write(dol.join("Config/Dolphin.ini"), b"[General]\n");
    write(dol.join("Cache/Shaders/x.cache"), &[7; 4096]);
    write(dol.join("Logs/hearth-launch.log"), b"log");
    fs::create_dir_all(dol.join("Load/Textures")).unwrap();
    std::os::unix::fs::symlink("/nonexistent/texture-pack", dol.join("Load/Textures/TP")).unwrap();
    let ctl = home.join("config/controls");
    write(ctl.join("systems/n64.toml"), b"# n64\n");
    write(ctl.join("games/gc/GZ2E01.toml"), b"# tp\n");
    write(ctl.join("device.toml"), b"sdl_name = \"x\"\n");
    write(home.join("config/hearth.toml"), b"[packs]\n");
    old_library(&home.join("data/library.sqlite3"));
}

fn old_library(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE games (id TEXT PRIMARY KEY, title TEXT NOT NULL, system TEXT NOT NULL,
             path TEXT NOT NULL, last_played INTEGER, playtime_secs INTEGER NOT NULL DEFAULT 0,
             favorite INTEGER NOT NULL DEFAULT 0, art_key TEXT, folder TEXT);
         INSERT INTO games VALUES ('oot', 'OoT', 'n64', '/g/oot.z64', 10, 3738, 1, NULL, NULL);
         PRAGMA user_version = 3;",
    )
    .unwrap();
}

fn dirs(home: &Path) -> DataDirs {
    DataDirs::resolve(Some(home.as_os_str().to_owned())).unwrap()
}

fn plenty(dirs: &DataDirs) -> Migrator<'_> {
    Migrator::new(dirs).with_free_space(u64::MAX)
}

/// Everything in the home but the backups and the marker.
fn layout(home: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    snapshot(home)
        .into_iter()
        .filter(|(p, _)| !p.starts_with("backup") && !p.ends_with("migration.toml"))
        .filter(|(p, _)| !p.ends_with("profile.toml"))
        // SQLite's bytes vary from run to run; `history` checks the content.
        .filter(|(p, _)| !p.starts_with("data/library.sqlite3"))
        // The fixture's retroarch.cfg names its own temporary home.
        .map(|(p, c)| {
            let text = String::from_utf8_lossy(&c).replace(&*home.to_string_lossy(), "<home>");
            (p, text.into_bytes())
        })
        .collect()
}

fn history(home: &Path) -> (i64, bool) {
    let lib = Library::open(&home.join("data/library.sqlite3")).unwrap();
    let e = lib
        .get(&ProfileId::default(), &crate::model::GameId::new("oot"))
        .unwrap()
        .unwrap();
    (e.playtime.as_secs() as i64, e.favorite)
}

fn check_migrated(home: &Path) {
    let p = home.join("profiles/default");
    let read = |rel: &str| fs::read(p.join(rel)).unwrap();
    assert_eq!(
        read("saves/n64/Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!].srm"),
        b"oot battery save"
    );
    assert_eq!(
        read("saves/gc/Card A/USA/01-GZ2E-gczelda2.gci"),
        b"tp gci save"
    );
    assert_eq!(read("saves/wii/nand/shared2/sys/SYSCONF"), b"sysconf");
    assert_eq!(
        read("emulators/retroarch/states/Mupen64Plus-Next/OoT.state"),
        b"oot state"
    );
    assert_eq!(
        read("emulators/retroarch/saves/Some-Other-Core/Game.srm"),
        b"other core"
    );
    assert_eq!(read("emulators/dolphin/StateSaves/GZ2E01.s01"), b"tp state");
    assert_eq!(read("emulators/dolphin/GC/SRAM.raw"), b"sram");
    assert_eq!(read("controls/systems/n64.toml"), b"# n64\n");
    assert_eq!(read("controls/games/gc/GZ2E01.toml"), b"# tp\n");
    assert!(p.join("profile.toml").is_file());
    assert_eq!(
        fs::read(home.join("emulators/retroarch/system/Mupen64plus/mupen64plus.ini")).unwrap(),
        b"bios-ish"
    );
    // Shared things stay.
    assert!(home.join("config/controls/device.toml").is_file());
    assert!(home.join("config/hearth.toml").is_file());
    for gone in [
        "retroarch",
        "dolphin",
        "config/controls/systems",
        "config/controls/games",
    ] {
        assert!(!exists(&home.join(gone)), "{gone}");
    }
    assert_eq!(history(home), (3738, true));
}

#[test]
fn a_new_install_has_nothing_to_do() {
    let dir = tempdir();
    let d = dirs(dir.path());
    assert_eq!(plenty(&d).run().unwrap(), Outcome::NothingToDo);
    assert!(!exists(&dir.path().join("backup")));
    assert!(!exists(&d.migration_marker()));
}

#[test]
fn migrates_backs_up_and_then_does_nothing() {
    let dir = tempdir();
    let home = dir.path();
    old_home(home);
    let before = snapshot(home);
    let d = dirs(home);
    let Outcome::Migrated(summary) = plenty(&d).run().unwrap() else {
        panic!("expected a migration");
    };
    check_migrated(home);

    // Backup: only what can't be regenerated, verified by its SHA256SUMS.
    let backup = &summary.backup;
    let sums = fs::read_to_string(backup.join("SHA256SUMS")).unwrap();
    let names: Vec<&str> = sums.lines().map(|l| &l[66..]).collect();
    for wanted in [
        "retroarch/saves/Mupen64Plus-Next/Legend of Zelda, The - Ocarina of Time (U) (V1.2) [!].srm",
        "retroarch/states/Mupen64Plus-Next/OoT.state",
        "retroarch/retroarch.cfg",
        "dolphin/GC/USA/Card A/01-GZ2E-gczelda2.gci",
        "dolphin/Wii/shared2/sys/SYSCONF",
        "dolphin/Config/Dolphin.ini",
        "config/controls/games/gc/GZ2E01.toml",
        "data/library.sqlite3",
    ] {
        assert!(names.contains(&wanted), "{wanted} not in {names:?}");
    }
    for unwanted in [
        "retroarch/system",
        "retroarch/logs",
        "dolphin/Cache",
        "dolphin/Logs",
    ] {
        assert!(!names.iter().any(|n| n.starts_with(unwanted)), "{unwanted}");
    }
    for line in sums.lines() {
        let (hash, name) = line.split_once("  ").unwrap();
        assert_eq!(hash_file(&backup.join(name)).unwrap().0, hash, "{name}");
        assert_eq!(
            fs::read(backup.join(name)).unwrap(),
            before[Path::new(name)],
            "{name}"
        );
    }
    // The symlink is copied as a link, not followed.
    assert_eq!(
        fs::read_link(backup.join("dolphin/Load/Textures/TP")).unwrap(),
        Path::new("/nonexistent/texture-pack")
    );
    assert!(
        summary
            .not_backed_up
            .iter()
            .any(|n| n.path.ends_with("dolphin/Cache") && n.bytes == 4096)
    );
    assert!(
        summary
            .unattributed
            .iter()
            .any(|p| p.ends_with("saves/Some-Other-Core"))
    );
    // retroarch.cfg still names the old folder, but is regenerated at launch. (Its relative
    // `./.dev-data/retroarch` line isn't this home's folder.)
    assert_eq!(summary.stale.len(), 1, "{:?}", summary.stale);
    assert!(summary.stale.iter().all(|s| s.regenerated));

    // A second start changes nothing.
    let after = snapshot(home);
    assert_eq!(plenty(&d).run().unwrap(), Outcome::NothingToDo);
    assert_eq!(snapshot(home), after);
}

#[test]
fn raw_memory_cards_move_to_the_gc_saves() {
    let dir = tempdir();
    let home = dir.path();
    write(home.join("dolphin/GC/MemoryCardA.USA.raw"), b"raw card");
    write(home.join("dolphin/GC/USA/.keep"), b"");
    let d = dirs(home);
    plenty(&d).run().unwrap();
    assert_eq!(
        fs::read(home.join("profiles/default/saves/gc/MemoryCardA.USA.raw")).unwrap(),
        b"raw card"
    );
    assert!(!exists(&home.join("dolphin")));
}

#[test]
fn a_kill_after_any_step_resumes_to_the_same_result() {
    let reference = {
        let dir = tempdir();
        old_home(dir.path());
        plenty(&dirs(dir.path())).run().unwrap();
        layout(dir.path())
    };
    let steps = {
        let dir = tempdir();
        old_home(dir.path());
        plenty(&dirs(dir.path()))
            .plan()
            .unwrap()
            .unwrap()
            .moves
            .len()
    };
    for stop in 0..=steps {
        let dir = tempdir();
        let home = dir.path();
        old_home(home);
        let d = dirs(home);
        let err = plenty(&d)
            .on_step(Box::new(move |n| {
                if n == stop {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            }))
            .run()
            .unwrap_err();
        assert!(matches!(err, MigrateError::Interrupted(n) if n == stop));
        let Outcome::Migrated(summary) = plenty(&d).run().unwrap() else {
            panic!("expected the run to resume after step {stop}");
        };
        assert!(summary.resumed);
        assert_eq!(layout(home), reference, "stopped after step {stop}");
        check_migrated(home);
        assert_eq!(fs::read_dir(home.join("backup")).unwrap().count(), 1);
    }
}

#[test]
fn a_rename_made_just_before_a_kill_counts_as_done() {
    let dir = tempdir();
    let home = dir.path();
    old_home(home);
    let d = dirs(home);
    let plan = plenty(&d).plan().unwrap().unwrap();
    let err = plenty(&d)
        .on_step(Box::new(|n| {
            if n == 1 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }))
        .run()
        .unwrap_err();
    assert!(matches!(err, MigrateError::Interrupted(1)));
    // The kill came after the second rename, before the marker recorded it.
    let next = &plan.moves[1];
    fs::create_dir_all(next.to.parent().unwrap()).unwrap();
    fs::rename(&next.from, &next.to).unwrap();
    plenty(&d).run().unwrap();
    check_migrated(home);
}

#[test]
fn a_kill_during_the_backup_starts_over() {
    let dir = tempdir();
    let home = dir.path();
    old_home(home);
    let d = dirs(home);
    let partial = home.join("backup/19990101-000000");
    write(partial.join("retroarch/half"), b"half");
    let marker = Marker {
        state: State::BackingUp,
        backup: partial.clone(),
        moves: plenty(&d).plan().unwrap().unwrap().moves,
        error: None,
    };
    plenty(&d).write_marker(&marker).unwrap();
    plenty(&d).run().unwrap();
    assert!(!exists(&partial));
    check_migrated(home);
}

#[test]
fn a_failed_move_puts_everything_back() {
    let dir = tempdir();
    let home = dir.path();
    old_home(home);
    let before = snapshot(home);
    let d = dirs(home);
    let plan = plenty(&d).plan().unwrap().unwrap();
    // Something takes the place of a later move while the migration runs.
    let obstacle = plan.moves[3].to.clone();
    let err = plenty(&d)
        .on_step(Box::new(move |n| {
            if n == 2 {
                write(obstacle.clone(), b"in the way");
            }
            ControlFlow::Continue(())
        }))
        .run()
        .unwrap_err();
    assert!(matches!(err, MigrateError::Exists(_)), "{err}");
    assert!(err.data_untouched());
    let now: BTreeMap<_, _> = snapshot(home)
        .into_iter()
        .filter(|(p, _)| {
            p.starts_with("retroarch")
                || p.starts_with("dolphin")
                || p.starts_with("config")
                || p.starts_with("data")
        })
        .collect();
    let then: BTreeMap<_, _> = before
        .into_iter()
        .filter(|(p, _)| {
            p.starts_with("retroarch")
                || p.starts_with("dolphin")
                || p.starts_with("config")
                || p.starts_with("data")
        })
        .collect();
    assert_eq!(now, then, "the old layout is back, byte for byte");
    let marker = plenty(&d).read_marker().unwrap().unwrap();
    assert_eq!(marker.state, State::RolledBack);
    assert!(marker.moves.iter().all(|m| !m.done));

    // With the obstacle gone, the next start migrates.
    fs::remove_file(&plan.moves[3].to).unwrap();
    plenty(&d).run().unwrap();
    check_migrated(home);
}

#[test]
fn an_existing_destination_is_refused_before_anything() {
    let dir = tempdir();
    let home = dir.path();
    old_home(home);
    write(
        home.join("profiles/default/emulators/retroarch/retroarch.cfg"),
        b"new",
    );
    let before = snapshot(home);
    let d = dirs(home);
    let err = plenty(&d).run().unwrap_err();
    assert!(matches!(err, MigrateError::Exists(_)), "{err}");
    assert_eq!(snapshot(home), before);
}

#[test]
fn too_little_space_is_refused_before_anything() {
    let dir = tempdir();
    let home = dir.path();
    old_home(home);
    let before = snapshot(home);
    let d = dirs(home);
    let size = Migrator::new(&d).plan().unwrap().unwrap().backup_bytes;
    let err = Migrator::new(&d)
        .with_free_space(size * 2 - 1)
        .run()
        .unwrap_err();
    assert!(
        matches!(err, MigrateError::NotEnoughSpace { size: s, .. } if s == size),
        "{err}"
    );
    assert!(err.to_string().contains("twice its size"));
    assert_eq!(snapshot(home), before);
    assert!(Migrator::new(&d).with_free_space(size * 2).run().is_ok());
}

#[test]
fn a_library_alone_is_upgraded_with_a_backup() {
    let dir = tempdir();
    let home = dir.path();
    old_library(&home.join("data/library.sqlite3"));
    let d = dirs(home);
    let Outcome::Migrated(summary) = plenty(&d).run().unwrap() else {
        panic!("expected a migration");
    };
    assert!(summary.moves.is_empty());
    assert!(summary.backup.join("data/library.sqlite3").is_file());
    assert_eq!(history(home), (3738, true));
}

#[test]
fn stale_paths_need_a_whole_folder_name() {
    assert!(names_folder("x = \"/h/retroarch/saves\"", "/h/retroarch"));
    assert!(names_folder("x = \"/h/retroarch\"", "/h/retroarch"));
    assert!(!names_folder("x = \"/h/retroarch-other\"", "/h/retroarch"));
}

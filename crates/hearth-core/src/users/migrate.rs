//! Moving an install from before profiles (round D) into `profiles/default/`.
//!
//! Before profiles, RetroArch and Dolphin had one folder each (`<home>/retroarch`,
//! `<home>/dolphin`), the controls files were in `<config>/controls`, and the library kept one
//! play history. The migration:
//!
//! 1. plans every move from what exists (battery saves of known RetroArch cores into
//!    `saves/<system>/`, Dolphin's memory cards into `saves/gc/`, its Wii NAND into
//!    `saves/wii/nand`, RetroArch's BIOS folder into the shared `emulators/retroarch/system`,
//!    then the rest of both emulator folders and the controls files into the profile), and
//!    refuses if a destination exists, a move would cross filesystems, or the free space is
//!    under twice the backup;
//! 2. copies what can't be regenerated (saves, states, controls, emulator config files and
//!    the library database; not caches, logs or BIOS files, which are moved unchanged) into
//!    `backup/<timestamp>/`, writes `SHA256SUMS` there and checks the copy against the
//!    originals (sizes and hashes);
//! 3. renames, one move at a time, recording each in the marker
//!    (`profiles/migration.toml`, written atomically) so a killed run resumes where it
//!    stopped: a move whose source is gone and destination present was done;
//! 4. writes `profile.toml`, upgrades the library (its history goes to `default`) and scans
//!    the moved emulator configs for paths that still name the old folders.
//!
//! Any error during the moves undoes the moves already made, in reverse, and leaves the
//! marker `rolled_back`: the old layout is back as it was, and the next start tries again.
//! A finished migration leaves the marker `done`, and every later start does nothing.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::ops::ControlFlow;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ProfileDirs, ProfileError, ProfileId, sync_parent, write_atomic};
use crate::library::{Library, LibraryError};
use crate::paths::DataDirs;

/// RetroArch save folders (named after the core, with RetroArch's default sorting) whose saves
/// belong to one system. Saves of other cores stay in the profile's RetroArch folder.
const RETROARCH_CORE_SYSTEMS: &[(&str, &str)] = &[("Mupen64Plus-Next", "n64")];

/// Under the RetroArch folder: regenerated or replaceable, so not backed up (but moved).
/// `system` is the BIOS folder, moved to the shared folder unchanged.
const RETROARCH_NOT_BACKED_UP: &[&str] = &["system", "logs", "cache", "thumbnails", "autoconfig"];

/// Under Dolphin's user folder: caches (shaders, covers), logs and dumps.
const DOLPHIN_NOT_BACKED_UP: &[&str] = &["Cache", "Logs", "Dump"];

/// The library database and SQLite's side files.
const DB_FILES: &[&str] = &["", "-wal", "-shm"];

/// Text files larger than this aren't scanned for stale paths.
const SCAN_LIMIT: u64 = 1 << 20;

/// Errors of the migration.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    /// The disk holding the backup lacks room for twice its size.
    #[error(
        "not enough free space for the backup: it is {size} bytes and needs {needed} free \
         (twice its size), only {free} are free"
    )]
    NotEnoughSpace {
        /// Size of what would be copied.
        size: u64,
        /// Free space wanted.
        needed: u64,
        /// Free space found.
        free: u64,
    },
    /// A move would cross filesystems (a copy, not a rename).
    #[error("{from} and {to} are on different filesystems: moving would copy, not rename")]
    CrossDevice {
        /// Source.
        from: PathBuf,
        /// Destination.
        to: PathBuf,
    },
    /// A destination already exists.
    #[error("{0} already exists; not overwriting it")]
    Exists(PathBuf),
    /// A source to move is gone although the marker says it wasn't moved yet.
    #[error("{0} is missing (neither it nor its destination exists)")]
    Missing(PathBuf),
    /// The backup copy differs from the original.
    #[error("the backup copy of {0} doesn't match the original")]
    Verify(PathBuf),
    /// A file or folder couldn't be read, written or moved.
    #[error("{path}: {source}")]
    Io {
        /// File or folder.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The marker file isn't valid.
    #[error("{path}: {message}")]
    Marker {
        /// Marker file.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
    /// The library couldn't be upgraded.
    #[error("upgrading the library: {0}")]
    Library(#[from] LibraryError),
    /// `profile.toml` couldn't be written.
    #[error(transparent)]
    Profile(#[from] ProfileError),
    /// The step hook stopped the run (tests: a kill at that point).
    #[error("migration stopped after step {0}")]
    Interrupted(usize),
    /// A move failed and undoing the moves failed too: the data is partly moved.
    #[error("{error}; undoing the moves failed too ({rollback}); backup in {backup}")]
    RollbackFailed {
        /// What stopped the migration.
        error: String,
        /// What stopped the rollback.
        rollback: String,
        /// The verified backup.
        backup: PathBuf,
    },
}

impl MigrateError {
    /// Whether the old data is still in place, untouched (everything but a failed rollback
    /// or a stop in the middle).
    pub fn data_untouched(&self) -> bool {
        !matches!(self, Self::RollbackFailed { .. } | Self::Interrupted(_))
    }
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> MigrateError + '_ {
    move |source| MigrateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// One planned rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Move {
    /// Old place.
    pub from: PathBuf,
    /// New place.
    pub to: PathBuf,
    /// What it is, for the log.
    pub what: String,
    /// Renamed already.
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    /// Copying into `backup`; a restart throws that copy away and starts over.
    BackingUp,
    /// Backup verified, renaming.
    Moving,
    /// Finished.
    Done,
    /// A move failed and every move was undone.
    RolledBack,
}

/// `profiles/migration.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    state: State,
    backup: PathBuf,
    moves: Vec<Move>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Something left out of the backup (moved unchanged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotBackedUp {
    /// File or folder.
    pub path: PathBuf,
    /// Its size.
    pub bytes: u64,
}

/// A moved emulator file that still names an old folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleRef {
    /// File.
    pub file: PathBuf,
    /// Line (1-based).
    pub line: usize,
    /// Hearth rewrites the file at every launch (`retroarch.cfg`), so it fixes itself.
    pub regenerated: bool,
}

/// What a migration would copy, before doing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The renames, in order.
    pub moves: Vec<Move>,
    /// Files to back up: (original, path inside the backup).
    pub backup_files: Vec<(PathBuf, PathBuf)>,
    /// Their total size.
    pub backup_bytes: u64,
    /// Left out of the backup.
    pub not_backed_up: Vec<NotBackedUp>,
    /// RetroArch save folders of unknown cores (they stay in the profile's RetroArch folder).
    pub unattributed: Vec<PathBuf>,
    /// The library database needs its upgrade to per-profile history.
    pub library_upgrade: bool,
}

/// What a finished migration did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The verified backup.
    pub backup: PathBuf,
    /// Files copied and checked.
    pub backed_up_files: usize,
    /// Their size.
    pub backed_up_bytes: u64,
    /// Left out of the backup (moved unchanged).
    pub not_backed_up: Vec<NotBackedUp>,
    /// The renames.
    pub moves: Vec<Move>,
    /// RetroArch save folders of unknown cores, left in the profile's RetroArch folder.
    pub unattributed: Vec<PathBuf>,
    /// Old paths still named by moved files.
    pub stale: Vec<StaleRef>,
    /// The run picked up an interrupted one.
    pub resumed: bool,
}

impl Summary {
    /// Logs the summary, one line per item.
    pub fn log(&self) {
        tracing::info!(
            backup = %self.backup.display(),
            files = self.backed_up_files,
            bytes = self.backed_up_bytes,
            resumed = self.resumed,
            "profiles migration: backup verified (sizes and SHA-256)"
        );
        for item in &self.not_backed_up {
            tracing::info!(
                path = %item.path.display(),
                bytes = item.bytes,
                "profiles migration: not backed up (cache, log or BIOS; moved unchanged)"
            );
        }
        for m in &self.moves {
            tracing::info!(
                from = %m.from.display(),
                to = %m.to.display(),
                "profiles migration: moved {}",
                m.what
            );
        }
        for dir in &self.unattributed {
            tracing::warn!(
                path = %dir.display(),
                "profiles migration: saves of an unknown RetroArch core, left in the profile's \
                 RetroArch folder"
            );
        }
        for stale in &self.stale {
            if stale.regenerated {
                tracing::info!(
                    file = %stale.file.display(),
                    line = stale.line,
                    "profiles migration: names an old folder; Hearth rewrites it at launch"
                );
            } else {
                tracing::warn!(
                    file = %stale.file.display(),
                    line = stale.line,
                    "profiles migration: still names an old folder"
                );
            }
        }
        tracing::info!(
            moves = self.moves.len(),
            stale = self.stale.iter().filter(|s| !s.regenerated).count(),
            "profiles migration: done; the old folders are now in profiles/default"
        );
    }
}

/// The result of [`Migrator::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing from before profiles (a new install, or migrated already).
    NothingToDo,
    /// Migrated now.
    Migrated(Summary),
}

/// Called after each step (0: backup verified; n: the n-th move recorded). `Break` stops the
/// run on the spot, as a kill would.
pub type StepHook<'a> = Box<dyn FnMut(usize) -> ControlFlow<()> + 'a>;

/// Moves an install from before profiles into `profiles/default/`.
pub struct Migrator<'a> {
    dirs: &'a DataDirs,
    hook: Option<StepHook<'a>>,
    free_space: Option<u64>,
}

impl fmt::Debug for Migrator<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Migrator")
            .field("dirs", &self.dirs)
            .finish_non_exhaustive()
    }
}

/// The old places, made absolute.
struct Legacy {
    retroarch: PathBuf,
    dolphin: PathBuf,
    controls: PathBuf,
    db: PathBuf,
    /// The forms the old folders may be written in inside config files.
    names: Vec<String>,
}

impl<'a> Migrator<'a> {
    /// A migrator for the install in `dirs`.
    pub fn new(dirs: &'a DataDirs) -> Self {
        Self {
            dirs,
            hook: None,
            free_space: None,
        }
    }

    /// Calls `hook` after each step.
    pub fn on_step(mut self, hook: StepHook<'a>) -> Self {
        self.hook = Some(hook);
        self
    }

    /// Pretends the disk has `bytes` free (tests).
    #[cfg(test)]
    fn with_free_space(mut self, bytes: u64) -> Self {
        self.free_space = Some(bytes);
        self
    }

    fn profile(&self) -> ProfileDirs {
        self.dirs.profile(&ProfileId::default_profile())
    }

    fn legacy(&self) -> Legacy {
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let retroarch = abs(self.dirs.legacy_retroarch());
        let dolphin = abs(self.dirs.legacy_dolphin());
        let mut names = Vec::new();
        for (given, absolute) in [
            (self.dirs.legacy_retroarch(), &retroarch),
            (self.dirs.legacy_dolphin(), &dolphin),
        ] {
            names.push(absolute.to_string_lossy().into_owned());
            if given != absolute.as_path() {
                names.push(given.to_string_lossy().into_owned());
            }
        }
        Legacy {
            retroarch,
            dolphin,
            controls: abs(&self.dirs.shared_controls()),
            db: abs(&self.dirs.library_db()),
            names,
        }
    }

    fn marker_path(&self) -> PathBuf {
        self.dirs.migration_marker()
    }

    fn read_marker(&self) -> Result<Option<Marker>, MigrateError> {
        let path = self.marker_path();
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .map(Some)
                .map_err(|err| MigrateError::Marker {
                    path,
                    message: err.message().to_owned(),
                }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(io_err(&path)(err)),
        }
    }

    fn write_marker(&self, marker: &Marker) -> Result<(), MigrateError> {
        let path = self.marker_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        let text = toml::to_string(marker).map_err(|err| MigrateError::Marker {
            path: path.clone(),
            message: err.to_string(),
        })?;
        write_atomic(&path, text.as_bytes()).map_err(io_err(&path))
    }

    fn step(&mut self, n: usize) -> Result<(), MigrateError> {
        match self.hook.as_mut().map(|hook| hook(n)) {
            Some(ControlFlow::Break(())) => Err(MigrateError::Interrupted(n)),
            _ => Ok(()),
        }
    }

    /// What a migration would do now, without changing anything. `None` if there is nothing
    /// to migrate or one is under way (see the marker).
    pub fn plan(&self) -> Result<Option<Plan>, MigrateError> {
        if self
            .read_marker()?
            .is_some_and(|m| m.state != State::RolledBack)
        {
            return Ok(None);
        }
        let legacy = self.legacy();
        let user = self.profile();
        let mut moves = Vec::new();
        let mut unattributed = Vec::new();
        if exists(&legacy.retroarch) {
            plan_retroarch(&legacy.retroarch, &user, &mut moves, &mut unattributed)?;
        }
        if exists(&legacy.dolphin) {
            plan_dolphin(&legacy.dolphin, &user, &mut moves)?;
        }
        for sub in ["systems", "games"] {
            let from = legacy.controls.join(sub);
            if exists(&from) {
                moves.push(Move {
                    from,
                    to: user.controls().join(sub),
                    what: format!("controls files ({sub})"),
                    done: false,
                });
            }
        }
        let library_upgrade = library_version(&legacy.db)?.is_some_and(|v| v < 4);
        if moves.is_empty() && !library_upgrade {
            return Ok(None);
        }

        let mut backup_files = Vec::new();
        let mut not_backed_up = Vec::new();
        let mut sources: Vec<(PathBuf, PathBuf, &[&str])> = Vec::new();
        if exists(&legacy.retroarch) {
            sources.push((
                legacy.retroarch.clone(),
                "retroarch".into(),
                RETROARCH_NOT_BACKED_UP,
            ));
        }
        if exists(&legacy.dolphin) {
            sources.push((
                legacy.dolphin.clone(),
                "dolphin".into(),
                DOLPHIN_NOT_BACKED_UP,
            ));
        }
        for sub in ["systems", "games"] {
            let from = legacy.controls.join(sub);
            if exists(&from) {
                sources.push((from, Path::new("config/controls").join(sub), &[]));
            }
        }
        for (root, name, skip) in sources {
            collect(&root, &name, skip, &mut backup_files, &mut not_backed_up)?;
        }
        if exists(&legacy.db) {
            for suffix in DB_FILES {
                let mut file = legacy.db.as_os_str().to_owned();
                file.push(suffix);
                let file = PathBuf::from(file);
                if exists(&file) {
                    let name = Path::new("data").join(file.file_name().unwrap_or_default());
                    backup_files.push((file, name));
                }
            }
        }
        let mut backup_bytes = 0;
        for (file, _) in &backup_files {
            backup_bytes += fs::symlink_metadata(file).map_err(io_err(file))?.len();
        }
        Ok(Some(Plan {
            moves,
            backup_files,
            backup_bytes,
            not_backed_up,
            unattributed,
            library_upgrade,
        }))
    }

    /// Migrates, resumes an interrupted migration, or does nothing.
    pub fn run(mut self) -> Result<Outcome, MigrateError> {
        let marker = self.read_marker()?;
        let (mut marker, plan, resumed) = match marker {
            Some(m) if m.state == State::Done => {
                let legacy = self.legacy();
                if [&legacy.retroarch, &legacy.dolphin]
                    .iter()
                    .any(|p| exists(p))
                {
                    tracing::warn!(
                        "profiles migration done earlier, but an old emulator folder exists \
                         again; leaving it alone"
                    );
                }
                return Ok(Outcome::NothingToDo);
            }
            Some(m) if m.state == State::Moving => {
                tracing::info!(
                    backup = %m.backup.display(),
                    "profiles migration: resuming an interrupted run"
                );
                (m, None, true)
            }
            other => {
                if let Some(m) = other.filter(|m| m.state == State::BackingUp) {
                    self.discard_partial_backup(&m.backup)?;
                    fs::remove_file(self.marker_path()).map_err(io_err(&self.marker_path()))?;
                }
                let Some(plan) = self.plan()? else {
                    return Ok(Outcome::NothingToDo);
                };
                self.check(&plan)?;
                let marker = self.back_up(&plan)?;
                (marker, Some(plan), false)
            }
        };
        self.step(0)?;
        if let Err(err) = self.make_moves(&mut marker) {
            return Err(self.roll_back(&mut marker, err));
        }
        let summary = match self.finish(&marker, plan.as_ref(), resumed) {
            Ok(summary) => summary,
            Err(err) => return Err(self.roll_back(&mut marker, err)),
        };
        marker.state = State::Done;
        self.write_marker(&marker)?;
        Ok(Outcome::Migrated(summary))
    }

    /// Refuses what a migration can't do safely: an existing destination, a move across
    /// filesystems, too little space for the backup.
    fn check(&self, plan: &Plan) -> Result<(), MigrateError> {
        for m in &plan.moves {
            if exists(&m.to) {
                return Err(MigrateError::Exists(m.to.clone()));
            }
            let (from, to) = (device(&m.from)?, device(&m.to)?);
            if from != to {
                return Err(MigrateError::CrossDevice {
                    from: m.from.clone(),
                    to: m.to.clone(),
                });
            }
        }
        let free = match self.free_space {
            Some(free) => free,
            None => free_space(self.dirs.backup())?,
        };
        let needed = plan.backup_bytes.saturating_mul(2);
        if free < needed {
            return Err(MigrateError::NotEnoughSpace {
                size: plan.backup_bytes,
                needed,
                free,
            });
        }
        Ok(())
    }

    /// Copies the backup set, writes `SHA256SUMS`, checks every copy against its original.
    fn back_up(&self, plan: &Plan) -> Result<Marker, MigrateError> {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let mut dir = self.dirs.backup().join(&stamp);
        let mut n = 1;
        while exists(&dir) {
            n += 1;
            dir = self.dirs.backup().join(format!("{stamp}-{n}"));
        }
        let dir = std::path::absolute(&dir).unwrap_or(dir);
        let mut marker = Marker {
            state: State::BackingUp,
            backup: dir.clone(),
            moves: plan.moves.clone(),
            error: None,
        };
        self.write_marker(&marker)?;
        fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        let mut sums = String::new();
        for (file, name) in &plan.backup_files {
            let copy = dir.join(name);
            if let Some(parent) = copy.parent() {
                fs::create_dir_all(parent).map_err(io_err(parent))?;
            }
            let meta = fs::symlink_metadata(file).map_err(io_err(file))?;
            if meta.file_type().is_symlink() {
                let target = fs::read_link(file).map_err(io_err(file))?;
                std::os::unix::fs::symlink(&target, &copy).map_err(io_err(&copy))?;
                if fs::read_link(&copy).map_err(io_err(&copy))? != target {
                    return Err(MigrateError::Verify(file.clone()));
                }
                continue;
            }
            let original = hash_file(file)?;
            fs::copy(file, &copy).map_err(io_err(&copy))?;
            {
                let out = fs::File::options()
                    .write(true)
                    .open(&copy)
                    .map_err(io_err(&copy))?;
                if let Ok(modified) = meta.modified() {
                    out.set_modified(modified).map_err(io_err(&copy))?;
                }
                out.sync_all().map_err(io_err(&copy))?;
            }
            let copied = hash_file(&copy)?;
            if copied != original || copied.1 != meta.len() {
                return Err(MigrateError::Verify(file.clone()));
            }
            sums.push_str(&format!("{}  {}\n", original.0, name.display()));
        }
        let sums_file = dir.join("SHA256SUMS");
        write_atomic(&sums_file, sums.as_bytes()).map_err(io_err(&sums_file))?;
        sync_tree(&dir)?;
        marker.state = State::Moving;
        self.write_marker(&marker)?;
        tracing::info!(
            backup = %dir.display(),
            files = plan.backup_files.len(),
            bytes = plan.backup_bytes,
            "profiles migration: backup written and verified"
        );
        Ok(marker)
    }

    /// Removes the copy of a backup that was interrupted (only inside the backup folder).
    fn discard_partial_backup(&self, dir: &Path) -> Result<(), MigrateError> {
        let backups = std::path::absolute(self.dirs.backup())
            .unwrap_or_else(|_| self.dirs.backup().to_path_buf());
        if dir.starts_with(&backups) && dir != backups && exists(dir) {
            tracing::info!(
                backup = %dir.display(),
                "profiles migration: removing an interrupted backup copy"
            );
            fs::remove_dir_all(dir).map_err(io_err(dir))?;
        }
        Ok(())
    }

    fn make_moves(&mut self, marker: &mut Marker) -> Result<(), MigrateError> {
        for i in 0..marker.moves.len() {
            if marker.moves[i].done {
                continue;
            }
            let Move { from, to, .. } = marker.moves[i].clone();
            match (exists(&from), exists(&to)) {
                (true, false) => {
                    if let Some(parent) = to.parent() {
                        fs::create_dir_all(parent).map_err(io_err(parent))?;
                    }
                    fs::rename(&from, &to).map_err(io_err(&from))?;
                    sync_parent(&to).map_err(io_err(&to))?;
                    sync_parent(&from).map_err(io_err(&from))?;
                }
                // Renamed just before a kill, not recorded yet.
                (false, true) => {}
                (true, true) => return Err(MigrateError::Exists(to)),
                (false, false) => return Err(MigrateError::Missing(from)),
            }
            marker.moves[i].done = true;
            self.write_marker(marker)?;
            self.step(i + 1)?;
        }
        Ok(())
    }

    /// Undoes the moves made, newest first, after `err`. Returns the error to report.
    fn roll_back(&self, marker: &mut Marker, err: MigrateError) -> MigrateError {
        if matches!(err, MigrateError::Interrupted(_)) {
            return err;
        }
        tracing::error!("profiles migration failed: {err}; undoing the moves");
        let mut failure = None;
        for m in marker.moves.iter_mut().rev().filter(|m| m.done) {
            let undone = (|| {
                if exists(&m.to) && !exists(&m.from) {
                    if let Some(parent) = m.from.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::rename(&m.to, &m.from)?;
                    sync_parent(&m.from)?;
                }
                Ok::<_, io::Error>(())
            })();
            match undone {
                Ok(()) => m.done = false,
                Err(rollback) => {
                    failure = Some(format!("{}: {rollback}", m.to.display()));
                    break;
                }
            }
        }
        marker.error = Some(err.to_string());
        match failure {
            None => {
                marker.state = State::RolledBack;
                if let Err(marker_err) = self.write_marker(marker) {
                    tracing::error!("profiles migration: can't record the rollback: {marker_err}");
                }
                tracing::info!("profiles migration: every move undone; the old data is in place");
                err
            }
            Some(rollback) => {
                // Stays `moving`: the next start resumes forward.
                if let Err(marker_err) = self.write_marker(marker) {
                    tracing::error!("profiles migration: can't record progress: {marker_err}");
                }
                MigrateError::RollbackFailed {
                    error: err.to_string(),
                    rollback,
                    backup: marker.backup.clone(),
                }
            }
        }
    }

    /// After the moves: `profile.toml`, the library upgrade, the stale path scan.
    fn finish(
        &self,
        marker: &Marker,
        plan: Option<&Plan>,
        resumed: bool,
    ) -> Result<Summary, MigrateError> {
        let user = self.profile();
        user.ensure()?;
        let legacy = self.legacy();
        if exists(&legacy.db) {
            Library::open_upgrading(&legacy.db)?;
        }
        let stale = scan_stale(&user.emulators(), &legacy.names)?;
        let (backed_up_files, backed_up_bytes, not_backed_up, unattributed) = match plan {
            Some(plan) => (
                plan.backup_files.len(),
                plan.backup_bytes,
                plan.not_backed_up.clone(),
                plan.unattributed.clone(),
            ),
            None => {
                let (files, bytes) = backup_totals(&marker.backup);
                (files, bytes, Vec::new(), Vec::new())
            }
        };
        Ok(Summary {
            backup: marker.backup.clone(),
            backed_up_files,
            backed_up_bytes,
            not_backed_up,
            moves: marker.moves.clone(),
            unattributed,
            stale,
            resumed,
        })
    }
}

/// RetroArch: battery saves of known cores into `saves/<system>/`, the BIOS folder to the
/// shared folder, then the rest of the folder.
fn plan_retroarch(
    root: &Path,
    user: &ProfileDirs,
    moves: &mut Vec<Move>,
    unattributed: &mut Vec<PathBuf>,
) -> Result<(), MigrateError> {
    let saves = root.join("saves");
    for dir in sorted_entries(&saves)? {
        if !dir.is_dir() {
            continue;
        }
        let core = dir.file_name().unwrap_or_default().to_string_lossy();
        let Some((_, system)) = RETROARCH_CORE_SYSTEMS.iter().find(|(c, _)| *c == core) else {
            unattributed.push(user.retroarch().join("saves").join(&*core));
            continue;
        };
        for file in sorted_entries(&dir)? {
            let name = file.file_name().unwrap_or_default().to_owned();
            moves.push(Move {
                to: user.saves().join(system).join(&name),
                what: format!("{system} save {}", name.to_string_lossy()),
                from: file,
                done: false,
            });
        }
    }
    let system = root.join("system");
    if exists(&system) {
        moves.push(Move {
            from: system,
            to: user.retroarch_system(),
            what: "RetroArch BIOS folder (shared by every profile)".into(),
            done: false,
        });
    }
    moves.push(Move {
        from: root.to_path_buf(),
        to: user.retroarch(),
        what: "RetroArch folder (states, config, remaps)".into(),
        done: false,
    });
    Ok(())
}

/// Dolphin: memory cards (GCI folders and raw cards) into `saves/gc/`, the Wii NAND into
/// `saves/wii/nand`, then the rest of the user folder.
fn plan_dolphin(
    root: &Path,
    user: &ProfileDirs,
    moves: &mut Vec<Move>,
) -> Result<(), MigrateError> {
    let gc = root.join("GC");
    for entry in sorted_entries(&gc)? {
        let name = entry
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if entry.is_dir() {
            // Dolphin's default folders use "JAP"; a configured GCI path gets "JPN"
            // (`RegionDirectoryStyle::Modern`).
            let region = if name == "JAP" { "JPN" } else { name.as_str() };
            for card in ["Card A", "Card B"] {
                let from = entry.join(card);
                if exists(&from) {
                    moves.push(Move {
                        from,
                        to: user.gc_saves().join(card).join(region),
                        what: format!("GameCube memory card folder {card} ({name})"),
                        done: false,
                    });
                }
            }
        } else if (name.starts_with("MemoryCardA.") || name.starts_with("MemoryCardB."))
            && name.ends_with(".raw")
        {
            moves.push(Move {
                to: user.gc_saves().join(&name),
                what: format!("GameCube memory card {name}"),
                from: entry,
                done: false,
            });
        }
    }
    let nand = root.join("Wii");
    if exists(&nand) {
        moves.push(Move {
            from: nand,
            to: user.wii_nand(),
            what: "Wii NAND (Wii saves)".into(),
            done: false,
        });
    }
    moves.push(Move {
        from: root.to_path_buf(),
        to: user.dolphin(),
        what: "Dolphin user folder (config, states, cache)".into(),
        done: false,
    });
    Ok(())
}

/// Every file under `root` (symlinks as themselves) with its name in the backup, except the
/// top-level entries in `skip`, which are listed with their size instead.
fn collect(
    root: &Path,
    name: &Path,
    skip: &[&str],
    files: &mut Vec<(PathBuf, PathBuf)>,
    not_backed_up: &mut Vec<NotBackedUp>,
) -> Result<(), MigrateError> {
    if !root.is_dir() {
        files.push((root.to_path_buf(), name.to_path_buf()));
        return Ok(());
    }
    for top in sorted_entries(root)? {
        let top_name = top.file_name().unwrap_or_default();
        if skip.iter().any(|s| top_name == *s) {
            not_backed_up.push(NotBackedUp {
                bytes: tree_size(&top)?,
                path: top,
            });
            continue;
        }
        for entry in walkdir::WalkDir::new(&top)
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry.map_err(|err| MigrateError::Io {
                path: top.clone(),
                source: err.into(),
            })?;
            if entry.file_type().is_dir() {
                continue;
            }
            let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
            files.push((entry.path().to_path_buf(), name.join(rel)));
        }
    }
    Ok(())
}

/// Total size of the files under `path`.
fn tree_size(path: &Path) -> Result<u64, MigrateError> {
    let mut total = 0;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry.map_err(|err| MigrateError::Io {
            path: path.to_path_buf(),
            source: err.into(),
        })?;
        if !entry.file_type().is_dir() {
            total += entry.metadata().map_or(0, |m| m.len());
        }
    }
    Ok(total)
}

/// Syncs every folder under `dir` (the files were synced as they were written).
fn sync_tree(dir: &Path) -> Result<(), MigrateError> {
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry.map_err(|err| MigrateError::Io {
            path: dir.to_path_buf(),
            source: err.into(),
        })?;
        if entry.file_type().is_dir() {
            fs::File::open(entry.path())
                .and_then(|d| d.sync_all())
                .map_err(io_err(entry.path()))?;
        }
    }
    sync_parent(dir).map_err(io_err(dir))
}

/// SHA-256 (hex) and size of a file.
fn hash_file(path: &Path) -> Result<(String, u64), MigrateError> {
    let mut file = fs::File::open(path).map_err(io_err(path))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let hex = hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            s.push_str(&format!("{b:02x}"));
            s
        });
    Ok((hex, size))
}

/// Files listed in a backup's `SHA256SUMS` and their total size (for the summary of a
/// resumed run).
fn backup_totals(backup: &Path) -> (usize, u64) {
    let sums = fs::read_to_string(backup.join("SHA256SUMS")).unwrap_or_default();
    sums.lines()
        .filter_map(|line| line.split_once("  "))
        .fold((0, 0), |(n, bytes), (_, name)| {
            let size = fs::symlink_metadata(backup.join(name)).map_or(0, |m| m.len());
            (n + 1, bytes + size)
        })
}

/// Moved emulator files (text, up to 1 MiB) that still name an old folder.
fn scan_stale(root: &Path, names: &[String]) -> Result<Vec<StaleRef>, MigrateError> {
    let mut found = Vec::new();
    if !root.is_dir() {
        return Ok(found);
    }
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|err| MigrateError::Io {
            path: root.to_path_buf(),
            source: err.into(),
        })?;
        if !entry.file_type().is_file() || entry.metadata().map_or(true, |m| m.len() > SCAN_LIMIT) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let regenerated = entry.file_name() == "retroarch.cfg";
        for (i, line) in text.lines().enumerate() {
            if names.iter().any(|name| names_folder(line, name)) {
                found.push(StaleRef {
                    file: entry.path().to_path_buf(),
                    line: i + 1,
                    regenerated,
                });
            }
        }
    }
    Ok(found)
}

/// Whether `line` names the folder `name` (itself or something inside it).
fn names_folder(line: &str, name: &str) -> bool {
    line.match_indices(name).any(|(at, _)| {
        matches!(
            line[at + name.len()..].chars().next(),
            None | Some('/' | '"' | '\'' | ' ')
        )
    })
}

/// The library's schema version, if the file exists.
fn library_version(db: &Path) -> Result<Option<i32>, MigrateError> {
    if !exists(db) {
        return Ok(None);
    }
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(LibraryError::from)?;
    let version = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(LibraryError::from)?;
    Ok(Some(version))
}

/// Entries of `dir` sorted by name (none if it doesn't exist).
fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>, MigrateError> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(io_err(dir)(err)),
    };
    let mut entries = read
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_err(dir))?;
    entries.sort();
    Ok(entries)
}

/// Whether something (even a dangling symlink) is at `path`.
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The filesystem holding `path`, or its nearest existing parent.
fn device(path: &Path) -> Result<u64, MigrateError> {
    let mut at = path;
    loop {
        match fs::metadata(at) {
            Ok(meta) => return Ok(meta.dev()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => match at.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => at = parent,
                _ => return Ok(fs::metadata(".").map_err(io_err(path))?.dev()),
            },
            Err(err) => return Err(io_err(path)(err)),
        }
    }
}

/// Free bytes for an unprivileged user on the filesystem of `path` (or its nearest parent).
fn free_space(path: &Path) -> Result<u64, MigrateError> {
    let mut at = path;
    while !exists(at) {
        match at.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => at = parent,
            _ => {
                at = Path::new(".");
                break;
            }
        }
    }
    let stat = nix::sys::statvfs::statvfs(at).map_err(|errno| MigrateError::Io {
        path: at.to_path_buf(),
        source: io::Error::from(errno),
    })?;
    Ok(stat.blocks_available().saturating_mul(stat.fragment_size()))
}

/// The files under `root` with their contents, for comparing trees in tests and checks.
pub fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let content = if entry.file_type().is_symlink() {
            fs::read_link(entry.path())
                .map(|t| t.into_os_string().into_encoded_bytes())
                .unwrap_or_default()
        } else if entry.file_type().is_file() {
            fs::read(entry.path()).unwrap_or_default()
        } else {
            b"<dir>".to_vec()
        };
        files.insert(rel.to_path_buf(), content);
    }
    files
}

#[cfg(test)]
mod tests;

//! Temporary directories for tests, kept under `target/` so tests never write elsewhere.

use std::path::PathBuf;

/// A fresh temporary directory under `target/test-tmp/`, removed on drop.
pub fn tempdir() -> tempfile::TempDir {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
    std::fs::create_dir_all(&base).expect("create target/test-tmp");
    tempfile::tempdir_in(base).expect("create temp dir")
}

//! The size gate of the hand-written Rust code (`AGENTS.md`): the build
//! fails when a code file of the package has [`LIMIT`] physical lines or
//! more. Clippy limits each function (`too-many-lines-threshold` in
//! `clippy.toml`) but has no limit for a whole file, so every `cargo
//! build`, `check`, `clippy`, and `test` counts the lines here, and CI with
//! them.
//!
//! Every line counts: code, comments, blank lines, and inline tests. The
//! gate reads the Rust files under [`ROOTS`] with the standard library
//! only; vendored data such as `contract/openapi.json` is not code and is
//! not read.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The fewest physical lines that a code file may not have.
const LIMIT: usize = 1000;

/// The files and directories of the hand-written Rust code, relative to the
/// package, where the build script runs. A missing root is skipped.
const ROOTS: [&str; 5] = ["build.rs", "src", "tests", "benches", "examples"];

fn main() -> ExitCode {
    let mut files = Vec::new();
    for root in ROOTS.map(Path::new) {
        match add_rust_files(root, &mut files) {
            Ok(true) => println!("cargo::rerun-if-changed={}", root.display()),
            Ok(false) => {}
            Err(error) => {
                eprintln!("size gate: {}: {error}", root.display());
                return ExitCode::FAILURE;
            }
        }
    }
    files.sort();
    let mut oversized = 0_usize;
    for file in &files {
        match fs::read_to_string(file) {
            Ok(text) => {
                let lines = text.lines().count();
                if lines >= LIMIT {
                    eprintln!(
                        "size gate: {}: {lines} lines; split it into cohesive modules below {LIMIT} lines",
                        file.display()
                    );
                    oversized += 1;
                }
            }
            Err(error) => {
                eprintln!("size gate: {}: {error}", file.display());
                return ExitCode::FAILURE;
            }
        }
    }
    if oversized == 0 {
        ExitCode::SUCCESS
    } else {
        eprintln!("size gate: {oversized} code file(s) with {LIMIT} lines or more");
        ExitCode::FAILURE
    }
}

/// Adds the Rust files at `path`, a file or a directory read recursively,
/// to `files`; false when `path` does not exist.
///
/// # Errors
///
/// When `path` or an entry under it is not readable.
fn add_rust_files(path: &Path, files: &mut Vec<PathBuf>) -> io::Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            add_rust_files(&entry?.path(), files)?;
        }
    } else if path.extension().is_some_and(|extension| extension == "rs") {
        files.push(path.to_owned());
    }
    Ok(true)
}

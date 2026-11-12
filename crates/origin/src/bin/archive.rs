//! `archive` binary: the archive command line (`archive::cli`) with the
//! node reader's line filters and the origin's parser wired in.
//!
//! Replay needs `BLEH_ARCHIVE_REGISTRY` (the instrument registry the reader
//! writes, `registry.json`); `BLEH_ARCHIVE_BOOK_START=empty` starts books
//! empty (a first recorded day) instead of waiting for L4 snapshots.

use std::process::ExitCode;

fn main() -> ExitCode {
    match origin::hooks::env_hooks() {
        Ok(hooks) => archive::cli::run(hooks),
        Err(e) => {
            eprintln!("archive: {e}");
            ExitCode::FAILURE
        }
    }
}

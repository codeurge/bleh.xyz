//! `archive` binary entry point.
//!
//! The node reader's line filters and raw parser are wired in by the
//! integrator through [`archive::cli::Hooks`]; without them, filtered
//! capture and replay report that they are not wired.

use std::process::ExitCode;

fn main() -> ExitCode {
    archive::cli::run(archive::cli::Hooks::default())
}

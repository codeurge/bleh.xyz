//! `hl-synth`: writes the synthetic Hyperliquid golden set.
//!
//! ```text
//! hl-synth [--seed N] [--blocks N] [--out DIR] [--check]
//! ```
//!
//! Defaults: seed 42, 60 blocks, `fixtures/synthetic/hl`. `--check`
//! compares the generated files with those on disk instead of writing, and
//! exits non-zero on any difference.

use std::path::PathBuf;
use std::process::ExitCode;

use hl_synth::{Config, generate};

fn usage() -> ExitCode {
    eprintln!("usage: hl-synth [--seed N] [--blocks N] [--out DIR] [--check]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut cfg = Config::default();
    let mut out = PathBuf::from("fixtures/synthetic/hl");
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => match args.next().and_then(|v| v.parse().ok()) {
                Some(v) => cfg.seed = v,
                None => return usage(),
            },
            "--blocks" => match args.next().and_then(|v| v.parse().ok()) {
                Some(v) => cfg.blocks = v,
                None => return usage(),
            },
            "--out" => match args.next() {
                Some(v) => out = PathBuf::from(v),
                None => return usage(),
            },
            "--check" => check = true,
            "-h" | "--help" => {
                eprintln!("usage: hl-synth [--seed N] [--blocks N] [--out DIR] [--check]");
                return ExitCode::SUCCESS;
            }
            _ => return usage(),
        }
    }
    let files = match generate(cfg) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("hl-synth: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut differences = 0usize;
    for (rel, content) in &files {
        let path = out.join(rel);
        if check {
            if std::fs::read(&path).ok().as_deref() != Some(content.as_slice()) {
                eprintln!("differs: {}", path.display());
                differences += 1;
            }
            continue;
        }
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!("hl-synth: {}: {e}", parent.display());
            return ExitCode::FAILURE;
        }
        if let Err(e) = std::fs::write(&path, content) {
            eprintln!("hl-synth: {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    if check && differences > 0 {
        eprintln!(
            "hl-synth: {differences} file(s) differ; regenerate with `cargo run -p hl-synth`"
        );
        return ExitCode::FAILURE;
    }
    let total: usize = files.values().map(Vec::len).sum();
    println!(
        "hl-synth: {} {} files ({total} bytes) under {}",
        if check { "checked" } else { "wrote" },
        files.len(),
        out.display()
    );
    ExitCode::SUCCESS
}

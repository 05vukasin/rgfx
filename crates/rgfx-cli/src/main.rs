//! Entry point for the `rgfx` binary.
//!
//! Kept intentionally thin: parse arguments with `clap`, then hand off to
//! [`rgfx_cli::run`]. All real logic lives in the library so it can be unit-tested.

use clap::Parser;
use rgfx_cli::Cli;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    rgfx_cli::run(cli)
}

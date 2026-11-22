use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
struct Options {
    #[arg(long, default_value = "patenti.sqlite3")]
    db: PathBuf,
}

fn main() -> Result<()> {
    zeropatenti::gui::run(&Options::parse().db)
}

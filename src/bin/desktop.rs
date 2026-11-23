#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
struct Options {
    #[arg(long, help = "Percorso del database SQLite")]
    db: Option<PathBuf>,
}

fn main() -> Result<()> {
    let db = Options::parse()
        .db
        .map(Ok)
        .unwrap_or_else(zeropatenti::datasets::desktop_profile)?;
    let result = zeropatenti::gui::run(&db);
    #[cfg(target_os = "windows")]
    if let Err(error) = &result {
        tinyfiledialogs::message_box_ok(
            "ZeroPatenti",
            &format!("{error:#}"),
            tinyfiledialogs::MessageBoxIcon::Error,
        );
    }
    result
}

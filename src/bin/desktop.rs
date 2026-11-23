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
    let db = Options::parse().db.unwrap_or_else(|| {
        let local = PathBuf::from("patenti.sqlite3");
        if local.exists() {
            return local;
        }
        // A portable distribution may already include the user's database beside the executable.
        std::env::current_exe()
            .ok()
            .map(|exe| exe.with_file_name("patenti.sqlite3"))
            .filter(|db| db.is_file())
            .unwrap_or(local)
    });
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

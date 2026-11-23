//! Local SQLite datasets and the published A/B editions.
use crate::{catalog, exam::catalog::bank_digest, storage};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use sha2::Digest;
mod editions;
pub use editions::*;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub const AB: &str = "ListatoAB";
pub const LEGACY: &str = "AB-7020";
pub const AB_DIGEST: &str = "3ca98f76f335244f59687d73e7d2dc522f566545e2ace3fa79d8c5927c174021";
pub fn label(version: &str) -> String {
    year_of_version(version)
        .map(|year| format!("A/B {year}"))
        .unwrap_or_else(|| version.to_owned())
}
pub fn warning(year: u16, latest: u16) -> String {
    format!("Edizione A/B {year} obsoleta. Per il conseguimento usa A/B {latest}.")
}
pub fn known_latest(db: &Connection, dir: &Path) -> Result<u16> {
    Ok(*available_years(db, dir)?
        .last()
        .context("Catalogo A/B vuoto")?)
}
pub fn desktop_profile() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let exe = std::env::current_exe()?;
    let mut roots = vec![cwd.as_path()];
    if let Some(dir) = exe.parent() {
        roots.push(dir);
        if let Some(project) = dir
            .ancestors()
            .find(|p| p.join("Cargo.toml").is_file() && p.join("DB").is_dir())
        {
            roots.push(project);
        }
    }
    for root in roots {
        let profile = root.join("progressi.sqlite3");
        if profile.is_file() {
            return Ok(profile);
        }
        let legacy = root.join("patenti.sqlite3");
        if legacy.is_file() {
            return Ok(legacy);
        }
        if root.join("DB").is_dir() {
            return Ok(profile);
        }
    }
    Ok(cwd.join("progressi.sqlite3"))
}

pub fn folder(profile: &Path) -> PathBuf {
    profile.with_file_name("DB")
}

pub fn scan(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut files = vec![];
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| {
                    ["sqlite3", "sqlite", "db"]
                        .iter()
                        .any(|e| ext.eq_ignore_ascii_case(e))
                })
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

pub fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS dataset_published(year INTEGER PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS dataset_editions(year INTEGER PRIMARY KEY, version TEXT UNIQUE NOT NULL,digest TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS dataset_images_installed(
        version TEXT NOT NULL,path TEXT NOT NULL,bytes BLOB NOT NULL,PRIMARY KEY(version,path));
        CREATE TABLE IF NOT EXISTS dataset_imports(version TEXT PRIMARY KEY, filename TEXT NOT NULL);")?;
    Ok(())
}

pub fn installed(db: &Connection, version: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM questions WHERE source_version=? LIMIT 1)",
        [version],
        |r| r.get(0),
    )?)
}

// Import only learning material, never progress, application settings or SQL from a dataset.
pub fn import(db: &mut Connection, path: &Path) -> Result<String> {
    let mut src = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    src.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")?;
    let src = src.transaction()?;
    let table = |name: &str| -> Result<bool> {
        Ok(src.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?)",
            [name],
            |r| r.get(0),
        )?)
    };
    ensure!(
        table("questions")?,
        "Il file non contiene un dataset ZeroPatenti"
    );
    let versions = src
        .prepare("SELECT DISTINCT source_version FROM questions LIMIT 2")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        versions.len() == 1,
        "Il dataset deve contenere una sola edizione"
    );
    let original = &versions[0];
    let year = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(year_of_file);
    let selected = year.map(edition).unwrap_or_else(|| original.clone());
    let version = &selected;
    let digest = bank_digest(&src, original)?;
    if version == AB {
        ensure!(
            digest == AB_DIGEST,
            "Contenuto del listato A/B 2026 inatteso"
        );
    } else if let Some(edition) = catalog::load()?
        .listati
        .into_iter()
        .find(|b| b.versione == *version)
        .and_then(|b| b.exam)
    {
        ensure!(
            digest == edition.digest,
            "Il dataset non coincide con l’edizione a catalogo"
        );
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing = installed(&tx, version)?;
    if existing {
        ensure!(
            bank_digest(&tx, version)? == digest,
            "Esiste già un dataset diverso con questa versione"
        );
    } else {
        let notes = table("study_notes")?;
        let sql = if notes {
            "SELECT external_id,group_id,topic_id,topic,question_text,correct_answer,image_path,brief,detail,attribution FROM questions LEFT JOIN study_notes USING(card_id)"
        } else {
            "SELECT external_id,group_id,topic_id,topic,question_text,correct_answer,image_path,NULL,NULL,NULL FROM questions"
        };
        let questions = src
            .prepare(sql)?
            .query_map([], |r| {
                Ok(storage::SourceQuestion {
                    id: r.get(0)?,
                    quesito_id: r.get(1)?,
                    argomento_id: r.get(2)?,
                    argomento_nome: r.get(3)?,
                    testo: r.get(4)?,
                    risposta: r.get(5)?,
                    figura: r.get(6)?,
                    spiegazione: r.get(7)?,
                    approfondimento: r.get(8)?,
                    fonte_spiegazione: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        storage::import_questions_in(&tx, version, questions)?;
    }
    let imported: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM dataset_imports WHERE version=?)",
        [version],
        |r| r.get(0),
    )?;
    if !imported {
        let embedded = if table("figure")? {
            Some("SELECT png FROM figure WHERE percorso=?")
        } else if table("dataset_images")? {
            Some("SELECT bytes FROM dataset_images WHERE path=?")
        } else {
            None
        };
        let mut lookup = embedded.map(|sql| src.prepare(sql)).transpose()?;
        let mut insert = tx.prepare("INSERT INTO dataset_images_installed VALUES (?,?,?)")?;
        let sidecar = path.with_extension("");
        let root = if sidecar.is_dir() {
            sidecar
        } else {
            path.parent()
                .context("Cartella immagini mancante")?
                .to_owned()
        };
        let mut names =
            src.prepare("SELECT DISTINCT image_path FROM questions WHERE image_path IS NOT NULL")?;
        for name in names.query_map([], |r| r.get::<_, String>(0))? {
            let name = name?;
            ensure!(
                name.starts_with("figures/")
                    && !name.contains(['\\', ':', '\0'])
                    && name
                        .split('/')
                        .all(|p| !p.is_empty() && p != "." && p != ".."),
                "Percorso figura non valido"
            );
            let bytes: Vec<u8> = if let Some(stmt) = &mut lookup {
                stmt.query_row([&name], |r| r.get(0))?
            } else {
                let full = root.join(&name).canonicalize()?;
                ensure!(
                    full.starts_with(root.canonicalize()?),
                    "Figura esterna al dataset"
                );
                fs::read(full)?
            };
            insert.execute(params![version, name, bytes])?;
        }
        tx.execute(
            "INSERT INTO dataset_imports VALUES (?,?)",
            params![
                version,
                path.file_name()
                    .and_then(|s| s.to_str())
                    .context("Nome file non valido")?
            ],
        )?;
    }
    if let Some(y) = year {
        tx.execute("INSERT INTO dataset_editions(year,version,digest) VALUES (?,?,?) ON CONFLICT(year) DO UPDATE SET digest=excluded.digest WHERE version=excluded.version",params![y,version,digest])?;
    }
    tx.commit()?;
    Ok(version.clone())
}

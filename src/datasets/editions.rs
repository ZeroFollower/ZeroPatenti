use super::*;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use sha1::Sha1;
use std::collections::{BTreeMap, BTreeSet};

const TREE: &str =
    "https://api.github.com/repos/lucatortiglione/ZeroPatenti/git/trees/main?recursive=1";

pub fn year_of_file(name: &str) -> Option<u16> {
    let n = name.strip_prefix("AB_")?.strip_suffix(".sqlite3")?;
    if n.len() != 4 || !n.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}
pub fn year_of_version(v: &str) -> Option<u16> {
    match v {
        AB => Some(2026),
        LEGACY => Some(2024),
        _ => year_of_file(&format!("{v}.sqlite3")),
    }
}
pub fn edition(year: u16) -> String {
    match year {
        2026 => AB.to_owned(),
        2024 => LEGACY.to_owned(),
        _ => format!("AB_{year:04}"),
    }
}

pub fn latest_installed(db: &Connection) -> Result<Option<u16>> {
    Ok(db.query_row(
        "SELECT MAX(year) FROM (
            SELECT year FROM dataset_editions
            UNION ALL SELECT 2024 WHERE EXISTS(SELECT 1 FROM questions WHERE source_version=?1)
            UNION ALL SELECT 2026 WHERE EXISTS(SELECT 1 FROM questions WHERE source_version=?2)
        )",
        params![LEGACY, AB],
        |r| r.get(0),
    )?)
}

pub fn local_years(dir: &Path) -> Result<BTreeMap<u16, PathBuf>> {
    let mut list = BTreeMap::new();
    // Prefer the user's direct copy if a published archive of the same year also exists.
    for folder in [dir.join("obsoleto"), dir.to_path_buf()] {
        for file in scan(&folder)? {
            if let Some(y) = file
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(year_of_file)
            {
                list.insert(y, file);
            }
        }
    }
    Ok(list)
}

pub fn available_years(db: &Connection, dir: &Path) -> Result<BTreeSet<u16>> {
    let mut years = BTreeSet::from([2024, 2026]);
    years.extend(local_years(dir)?.into_keys());
    let mut stmt =
        db.prepare("SELECT year FROM dataset_editions UNION SELECT year FROM dataset_published")?;
    for year in stmt.query_map([], |r| r.get::<_, u16>(0))? {
        years.insert(year?);
    }
    Ok(years)
}

pub fn remember_published(db: &mut Connection, years: Option<&[u16]>) -> Result<()> {
    if let Some(years) = years {
        let tx = db.transaction()?;
        tx.execute("DELETE FROM dataset_published", [])?;
        for year in years {
            tx.execute("INSERT INTO dataset_published VALUES (?)", [year])?;
        }
        tx.commit()?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct Item {
    path: String,
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct Tree {
    tree: Vec<Item>,
    truncated: bool,
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into()
}
fn online(client: &ureq::Agent) -> Result<BTreeMap<u16, Item>> {
    let bytes = client
        .get(TREE)
        .header("User-Agent", "ZeroPatenti")
        .call()?
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_vec()?;
    let tree: Tree = serde_json::from_slice(&bytes)?;
    ensure!(!tree.truncated, "Indice GitHub incompleto");
    let mut editions = BTreeMap::new();
    for item in tree.tree {
        let Some(relative) = item.path.strip_prefix("DB/") else {
            continue;
        };
        let name = relative.strip_prefix("obsoleto/").unwrap_or(relative);
        if item.kind == "blob"
            && let Some(y) = year_of_file(name)
        {
            ensure!(
                item.sha.len() == 40 && item.sha.bytes().all(|c| c.is_ascii_hexdigit()),
                "Identità Git non valida"
            );
            ensure!(
                editions.insert(y, item).is_none(),
                "Due dataset GitHub per lo stesso anno"
            );
        }
    }
    Ok(editions)
}
pub fn published_years() -> Result<Vec<u16>> {
    Ok(online(&agent())?.into_keys().collect())
}

fn blob_id(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()));
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub struct Resolved {
    pub year: u16,
    pub latest: u16,
    pub path: Option<PathBuf>,
    pub published: Option<Vec<u16>>,
}

// Re-read the remote index for every new default exam. A missing cache file does not
// downgrade an edition already imported into the personal database.
pub fn resolve(dir: &Path, installed: Option<u16>, requested: Option<u16>) -> Result<Resolved> {
    let local = local_years(dir)?;
    let client = agent();
    let response = online(&client);
    let published = response
        .as_ref()
        .ok()
        .map(|items| items.keys().copied().collect());
    let remote = response.unwrap_or_default();
    let latest = local
        .keys()
        .chain(remote.keys())
        .copied()
        .chain(installed)
        .max();
    let year = requested
        .or(latest)
        .context("Nessun dataset A/B disponibile in DB o su GitHub")?;
    let record = remote.get(&year);
    let cached = local.get(&year);
    if let Some(path) = cached {
        // An explicitly chosen historical local copy may also contain personal progress.
        let matches_catalog = match record {
            Some(item) if requested.is_none() => blob_id(&fs::read(path)?) == item.sha,
            _ => true,
        };
        if matches_catalog {
            return Ok(Resolved {
                year,
                latest: latest.unwrap_or(year),
                path: Some(path.clone()),
                published,
            });
        }
    }
    if let Some(item) = record {
        let url = format!(
            "https://api.github.com/repos/lucatortiglione/ZeroPatenti/git/blobs/{}",
            item.sha
        );
        let bytes = client
            .get(&url)
            .header("User-Agent", "ZeroPatenti")
            .header("Accept", "application/vnd.github.raw+json")
            .call()?
            .body_mut()
            .with_config()
            .limit(100 * 1024 * 1024)
            .read_to_vec()?;
        ensure!(
            blob_id(&bytes) == item.sha,
            "Il download del dataset non coincide con il file su GitHub"
        );
        let target = dir.join(
            item.path
                .strip_prefix("DB/")
                .context("Percorso GitHub inatteso")?,
        );
        let parent = target.parent().context("Cartella DB mancante")?;
        fs::create_dir_all(parent)?;
        if target.is_file() {
            // Preserve a different local file with the same name (it may contain progress).
            bail!(
                "Il file {} differisce da quello pubblicato. Sposta la copia locale per scaricare la versione pubblicata.",
                target.display()
            );
        }
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(&bytes)?;
        match tmp.persist_noclobber(&target) {
            Ok(_) => (),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure!(
                    blob_id(&fs::read(&target)?) == item.sha,
                    "Conflitto sulla copia locale del dataset"
                );
            }
            Err(e) => return Err(e.error.into()),
        }
        return Ok(Resolved {
            year,
            latest: latest.unwrap_or(year),
            path: Some(target),
            published,
        });
    }
    if installed == Some(year) {
        return Ok(Resolved {
            year,
            latest: latest.unwrap_or(year),
            path: None,
            published,
        });
    }
    bail!("AB_{year}.sqlite3 non è disponibile in DB o su GitHub")
}

pub fn installed_digest(db: &Connection, v: &str) -> Result<Option<String>> {
    Ok(db
        .query_row(
            "SELECT digest FROM dataset_editions WHERE version=?",
            [v],
            |r| r.get(0),
        )
        .optional()?)
}

use super::*;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    version: String,
    files: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq)]
pub(super) struct ImportSelection {
    file: PathBuf,
    root: PathBuf,
    version: String,
    digest: String,
}

struct Package {
    selection: ImportSelection,
    questions: Vec<storage::SourceQuestion>,
    signature: Option<VerifiedManifest>,
}

struct VerifiedManifest {
    digest: String,
    figures: BTreeMap<String, String>,
}

fn parse_key(value: &str) -> Result<VerifyingKey> {
    let mut bytes = [0; 32];
    hex::decode_to_slice(value.trim(), &mut bytes)
        .context("La chiave Ed25519 deve contenere 64 cifre esadecimali")?;
    let key = VerifyingKey::from_bytes(&bytes).context("Chiave pubblica Ed25519 non valida")?;
    ensure!(!key.is_weak(), "Chiave pubblica Ed25519 non ammessa");
    Ok(key)
}

pub(super) fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    metadata.is_symlink()
}

fn present(file: &Path) -> Result<bool> {
    match fs::symlink_metadata(file) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':', '\0'])
        && name.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.ends_with([' ', '.'])
        })
}

fn package_file(root: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        safe_relative(name),
        "Percorso non valido nel pacchetto: {name}"
    );
    let mut file = root.to_owned();
    for part in name.split('/') {
        file.push(part);
        let meta = fs::symlink_metadata(&file)
            .with_context(|| format!("File del pacchetto mancante: {name}"))?;
        ensure!(
            !is_link(&meta),
            "Collegamento non ammesso nel pacchetto: {name}"
        );
    }
    let real = file
        .canonicalize()
        .with_context(|| format!("File del pacchetto mancante: {name}"))?;
    ensure!(
        real.starts_with(root) && real.is_file(),
        "File esterno al pacchetto o non valido: {name}"
    );
    Ok(real)
}

impl Package {
    fn load(file: &Path, assets: &str, version: &str, key: &str) -> Result<Self> {
        let parent = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        let name = file
            .file_name()
            .and_then(|v| v.to_str())
            .context("Scegli il file dei quesiti")?;
        let file = package_file(&parent, name)?;
        let root = if assets.trim().is_empty() {
            parent.clone()
        } else {
            Path::new(assets).canonicalize()?
        };
        ensure!(root.is_dir(), "Cartella delle immagini inesistente");
        let manifest = parent.join("manifest.json");
        let sig = parent.join("manifest.sig");
        let signed = present(&manifest)? || present(&sig)?;
        let bytes = fs::read(&file)?;
        let digest = hex::encode(Sha256::digest(&bytes));
        let questions = storage::parse_package_questions(&file, &bytes)?;
        let figures: BTreeSet<&str> = questions
            .iter()
            .filter_map(|q| q.figura.as_deref())
            .collect();
        for figure in &figures {
            ensure!(
                safe_relative(figure),
                "Percorso figura non valido: {figure}"
            );
        }
        let signature = if signed {
            ensure!(
                root == parent,
                "Per i pacchetti firmati, scegli la cartella contenente il manifest come origine delle immagini"
            );
            ensure!(
                !key.is_empty(),
                "Configura la chiave pubblica attendibile dell’editore prima di verificare la firma"
            );
            let key = parse_key(key)?;
            let manifest = fs::read(package_file(&parent, "manifest.json")?)?;
            let sig = fs::read(package_file(&parent, "manifest.sig")?)?;
            let signature = Signature::from_slice(&sig).context("Firma Ed25519 illeggibile")?;
            key.verify_strict(&manifest, &signature)
                .context("Firma digitale del manifest non valida")?;
            let identity = hex::encode(Sha256::digest(&manifest));
            let mut manifest: Manifest =
                serde_json::from_slice(&manifest).context("Manifest illeggibile")?;
            ensure!(
                manifest.format == 1 && manifest.version == version,
                "Versione del manifest incompatibile con {version}"
            );
            ensure!(
                matches!(name, "questions.json" | "questions.csv") && !figures.contains(name),
                "Nome del listato non valido"
            );
            ensure!(
                manifest.files.get(name) == Some(&digest),
                "Impronta SHA-256 dei quesiti non valida"
            );
            ensure!(
                manifest.files.len() == figures.len() + 1,
                "Il manifest deve elencare tutti e soli i file utilizzati"
            );
            for figure in figures {
                let expected = manifest
                    .files
                    .get(figure)
                    .with_context(|| format!("Figura assente dal manifest: {figure}"))?;
                let mut reader = fs::File::open(package_file(&parent, figure)?)?;
                let mut hash = Sha256::new();
                std::io::copy(&mut reader, &mut hash)?;
                ensure!(
                    expected == &hex::encode(hash.finalize()),
                    "Impronta SHA-256 non valida per {figure}"
                );
            }
            manifest.files.remove(name);
            Some(VerifiedManifest {
                digest: identity,
                figures: manifest.files,
            })
        } else {
            None
        };
        Ok(Self {
            selection: ImportSelection {
                file,
                root,
                version: version.to_owned(),
                digest,
            },
            questions,
            signature,
        })
    }
}

pub(super) fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS gui_signed_packages(
        bank_version TEXT PRIMARY KEY, question_file TEXT NOT NULL, manifest_digest TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS gui_signed_figures(
            bank_version TEXT NOT NULL REFERENCES gui_signed_packages(bank_version) ON DELETE CASCADE,
            path TEXT NOT NULL, digest TEXT NOT NULL, PRIMARY KEY(bank_version,path));
        CREATE TABLE IF NOT EXISTS gui_bank_trust(id INTEGER PRIMARY KEY CHECK(id=1), public_key TEXT NOT NULL);
        CREATE TRIGGER IF NOT EXISTS gui_package_changed AFTER UPDATE ON questions BEGIN
            DELETE FROM gui_signed_packages WHERE bank_version IN (OLD.source_version, NEW.source_version);
        END;
        CREATE TRIGGER IF NOT EXISTS gui_package_extended AFTER INSERT ON questions BEGIN
            DELETE FROM gui_signed_packages WHERE bank_version=NEW.source_version;
        END;
        CREATE TRIGGER IF NOT EXISTS gui_package_pruned AFTER DELETE ON questions BEGIN
            DELETE FROM gui_signed_packages WHERE bank_version=OLD.source_version;
        END;")?;
    Ok(())
}

#[derive(Debug, PartialEq)]
pub(super) struct ArchivedEdition {
    version: String,
    questions: i64,
    protected: bool,
}

fn archive(db: &Connection, active: &str) -> Result<Vec<ArchivedEdition>> {
    let current: BTreeSet<_> = catalog::load()?
        .listati
        .into_iter()
        .map(|e| e.versione)
        .collect();
    // Retain the whole edition when any question has progress, so its historical totals,
    // results and media remain available. CLI imports and old databases share these tables.
    let mut query = db.prepare(
        "SELECT source_version, COUNT(*),
        MAX(EXISTS(SELECT 1 FROM study_card_views v WHERE v.card_id=q.card_id)
        OR EXISTS(SELECT 1 FROM card_state c WHERE c.card_id=q.card_id)
        OR EXISTS(SELECT 1 FROM review_log l WHERE l.card_id=q.card_id))
        OR EXISTS(SELECT 1 FROM exam_sessions s WHERE s.bank_version=q.source_version)
        FROM questions q GROUP BY source_version ORDER BY source_version",
    )?;
    let rows = query.query_map([], |r| {
        Ok(ArchivedEdition {
            version: r.get(0)?,
            questions: r.get(1)?,
            protected: r.get(2)?,
        })
    })?;
    let mut items = Vec::new();
    for row in rows {
        let item = row?;
        if item.version != active && !current.contains(&item.version) {
            items.push(item);
        }
    }
    Ok(items)
}

fn local_bytes(path: &Path) -> Result<u64> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    if is_link(&metadata) {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut size = 0;
    if metadata.is_dir() {
        for item in fs::read_dir(path)? {
            size += local_bytes(&item?.path())?;
        }
    }
    Ok(size)
}

impl Controller {
    pub(super) fn load_figure(&self, version: &str, relative: &str) -> Result<Image> {
        let (root, signed, digest): (String, bool, Option<String>) = self
            .db
            .query_row(
                "SELECT m.directory, p.bank_version IS NOT NULL, f.digest
             FROM gui_media_roots m
             LEFT JOIN gui_signed_packages p USING(bank_version)
             LEFT JOIN gui_signed_figures f ON f.bank_version=p.bank_version AND f.path=?2
             WHERE m.bank_version=?1",
                params![version, relative],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .context("Controlla la cartella delle immagini")?;
        ensure!(!root.is_empty(), "Controlla la cartella delle immagini");
        let root = Path::new(&root).canonicalize()?;
        if signed {
            let expected = digest.context("Figura assente dal manifest installato")?;
            let bytes = fs::read(package_file(&root, relative)?)?;
            ensure!(
                hex::encode(Sha256::digest(&bytes)) == expected,
                "Impronta SHA-256 della figura non valida"
            );
            return Image::load_from_data(&bytes, None)
                .ok()
                .context("Formato immagine illeggibile");
        }
        let path = root.join(relative).canonicalize()?;
        ensure!(
            path.starts_with(&root) && path.is_file(),
            "Immagine fuori dalla cartella del listato"
        );
        Image::load_from_path(&path)
            .ok()
            .context("Formato immagine illeggibile")
    }

    pub(super) fn bank_management(&mut self, ui: &AppWindow) -> Result<()> {
        self.pending_archive = None;
        self.pending_import = None;
        ui.set_unsigned_pending(false);
        ui.set_cleanup_pending(false);
        ui.set_bank_key_fixed(option_env!("ZEROPATENTI_BANK_PUBLIC_KEY").is_some());
        ui.set_bank_public_key(self.bank_key()?.into());
        ui.set_bank_signature(
            if self.signed_file()?.is_some() {
                "Firma verificata all’importazione"
            } else if ui.get_total() > 0 {
                "Nessuna firma digitale registrata"
            } else {
                "Listato da importare"
            }
            .into(),
        );
        let dbfile = self.db.path().context("Percorso del database mancante")?;
        let mut bytes = 0;
        for suffix in ["", "-wal", "-shm"] {
            bytes += local_bytes(Path::new(&format!("{dbfile}{suffix}")))?;
        }
        #[cfg(target_os = "android")]
        {
            bytes += local_bytes(&self.staged_media)?;
        }
        ui.set_bank_storage(
            format!("{:.2} MiB · {bytes} byte", bytes as f64 / (1024.0 * 1024.0)).into(),
        );
        let editions = archive(&self.db, &self.version)?;
        ui.set_can_cleanup(editions.iter().any(|e| !e.protected));
        ui.set_archive_summary(if editions.is_empty() {
            "Nessuna edizione storica installata.".into()
        } else {
            editions
                .iter()
                .map(|e| {
                    format!(
                        "{} · {} domande · {}",
                        e.version,
                        e.questions,
                        if e.protected {
                            "conservata per i progressi o le prove"
                        } else {
                            "disponibile per la pulizia"
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
                .into()
        });
        Ok(())
    }

    pub(super) fn preview_archive(&mut self, ui: &AppWindow) -> Result<()> {
        let plan: Vec<_> = archive(&self.db, &self.version)?
            .into_iter()
            .filter(|e| !e.protected)
            .collect();
        ensure!(!plan.is_empty(), "Nessuna edizione storica da rimuovere");
        let names = plan
            .iter()
            .map(|e| format!("{} ({} domande)", e.version, e.questions))
            .collect::<Vec<_>>()
            .join(", ");
        ui.set_cleanup_message(format!("Rimuovere dal dispositivo {names}? Le cartelle di origine esterne restano disponibili.").into());
        self.pending_archive = Some(plan);
        ui.set_cleanup_pending(true);
        Ok(())
    }

    pub(super) fn clean_archive(&mut self, ui: &AppWindow) -> Result<()> {
        let expected = self
            .pending_archive
            .take()
            .context("Apri l’anteprima della pulizia")?;
        ui.set_cleanup_pending(false);
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let actual: Vec<_> = archive(&tx, &self.version)?
            .into_iter()
            .filter(|e| !e.protected)
            .collect();
        ensure!(
            expected == actual,
            "Le edizioni sono cambiate. Controlla una nuova anteprima."
        );
        #[cfg(any(target_os = "android", test))]
        let mut retired_media = BTreeSet::new();
        for entry in &actual {
            #[cfg(any(target_os = "android", test))]
            {
                let root: Option<String> = tx.query_row("SELECT directory FROM gui_media_roots WHERE bank_version=? AND directory<>''", [&entry.version], |r| r.get(0)).optional()?;
                if let Some(root) = root {
                    match Path::new(&root).canonicalize() {
                        Ok(path) => {
                            retired_media.insert(path);
                        }
                        Err(e) if e.kind() == ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            tx.execute(
                "DELETE FROM questions WHERE source_version=?",
                [&entry.version],
            )?;
            tx.execute(
                "DELETE FROM gui_signed_packages WHERE bank_version=?",
                [&entry.version],
            )?;
            tx.execute(
                "DELETE FROM gui_media_roots WHERE bank_version=?",
                [&entry.version],
            )?;
        }
        tx.commit()?;
        #[cfg(any(target_os = "android", test))]
        self.remove_unused_media(Some(&retired_media)).context(
            "Edizioni rimosse dal database. La pulizia delle copie private è rimasta incompleta",
        )?;
        // Reclaim unused pages before measuring storage again.
        self.db
            .execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
            .context("Edizioni rimosse. La compattazione del database è rimasta incompleta")?;
        self.bank_management(ui)?;
        ui.set_notice(format!("Edizioni storiche rimosse: {}.", actual.len()).into());
        Ok(())
    }

    fn bank_key(&self) -> Result<String> {
        if let Some(key) = option_env!("ZEROPATENTI_BANK_PUBLIC_KEY") {
            return Ok(key.trim().to_owned());
        }
        Ok(self
            .db
            .query_row(
                "SELECT public_key FROM gui_bank_trust WHERE id=1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default())
    }

    pub(super) fn set_bank_key(&self, ui: &AppWindow) -> Result<()> {
        ensure!(
            option_env!("ZEROPATENTI_BANK_PUBLIC_KEY").is_none(),
            "La distribuzione include una chiave pubblica fissa"
        );
        let key = parse_key(&ui.get_bank_public_key())?;
        let key = hex::encode(key.as_bytes());
        self.db.execute(
            "INSERT OR REPLACE INTO gui_bank_trust VALUES (1, ?)",
            [&key],
        )?;
        ui.set_bank_public_key(key.into());
        ui.set_notice("Chiave pubblica dell’editore salvata.".into());
        Ok(())
    }

    pub(super) fn signed_file(&self) -> Result<Option<(String, String)>> {
        Ok(self.db.query_row("SELECT question_file,manifest_digest FROM gui_signed_packages WHERE bank_version=?", [&self.version], |r| Ok((r.get(0)?,r.get(1)?))).optional()?)
    }

    pub(super) fn verify_bank(&self, ui: &AppWindow) -> Result<()> {
        let rule = verified_rule(&self.db, &ui.get_license(), &ui.get_procedure())?;
        let status = if let Some((name, digest)) = self.signed_file()? {
            let root: String = self.db.query_row(
                "SELECT directory FROM gui_media_roots WHERE bank_version=?",
                [&self.version],
                |r| r.get(0),
            )?;
            let root = Path::new(&root).canonicalize()?;
            let package = Package::load(
                &package_file(&root, &name)?,
                "",
                &self.version,
                &self.bank_key()?,
            )?;
            ensure!(
                package.signature.as_ref().map(|s| &s.digest) == Some(&digest),
                "Il manifest del pacchetto installato è assente o è cambiato"
            );
            "Firma dell’editore, quesiti e figure verificati."
        } else {
            "I quesiti corrispondono al catalogo. Nessuna firma digitale registrata."
        };
        ui.set_notice(format!("Listato {} verificato. {status}", rule.bank_version).into());
        Ok(())
    }

    pub(super) fn import_bank(&mut self, ui: &AppWindow, confirm: bool) -> Result<()> {
        let file = PathBuf::from(ui.get_import_path().trim());
        let version = catalog::version_for(&ui.get_license(), &ui.get_procedure())?;
        let package = Package::load(&file, &ui.get_asset_root(), &version, &self.bank_key()?)?;
        let tx = self.db.transaction()?;
        let count = storage::import_questions_in(&tx, &version, package.questions)?;
        verified_rule(&tx, &ui.get_license(), &ui.get_procedure())?;
        let stored: i64 = tx.query_row(
            "SELECT COUNT(*) FROM questions WHERE source_version=?",
            [&version],
            |r| r.get(0),
        )?;
        ensure!(
            i64::try_from(count)? == stored,
            "Listato incompleto per {version}"
        );
        if package.signature.is_none()
            && (!confirm || self.pending_import.as_ref() != Some(&package.selection))
        {
            self.pending_import = Some(package.selection);
            ui.set_unsigned_pending(true);
            ui.set_notice("Listato senza firma digitale. I quesiti corrispondono al catalogo. Conferma per importare da questa fonte.".into());
            return Ok(()); // Roll back the preview, including when the selected file changed.
        }
        let root = package
            .selection
            .root
            .to_str()
            .context("Percorso immagini illeggibile")?;
        Self::save_media(&tx, &version, root)?;
        if let Some(signature) = &package.signature {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .context("Nome del listato illeggibile")?;
            tx.execute(
                "INSERT OR REPLACE INTO gui_signed_packages VALUES (?,?,?)",
                params![version, name, signature.digest],
            )?;
            let mut insert = tx.prepare("INSERT INTO gui_signed_figures VALUES (?,?,?)")?;
            for (path, digest) in &signature.figures {
                insert.execute(params![version, path, digest])?;
            }
        } else {
            tx.execute(
                "DELETE FROM gui_signed_packages WHERE bank_version=?",
                [&version],
            )?;
        }
        tx.commit()?;
        self.pending_import = None;
        ui.set_unsigned_pending(false);
        self.home(ui)?;
        let signature = if package.signature.is_some() {
            "Firma digitale verificata."
        } else {
            "Pacchetto senza firma."
        };
        ui.set_notice(format!("Importate {count} domande. {signature}").into());
        Ok(())
    }

    #[cfg(not(target_os = "android"))]
    pub(super) fn pick_bank(&self, ui: &AppWindow) -> Result<()> {
        let Some(dir) = tinyfiledialogs::select_folder_dialog("Scegli la cartella del listato", "")
        else {
            return Ok(());
        };
        let dir = Path::new(&dir).canonicalize()?;
        let files: Vec<_> = ["questions.json", "questions.csv"]
            .map(|n| dir.join(n))
            .into_iter()
            .filter(|p| p.is_file())
            .collect();
        ensure!(
            files.len() == 1,
            "Scegli una cartella con un solo listato questions.json o questions.csv"
        );
        ui.set_import_path(files[0].to_string_lossy().as_ref().into());
        ui.set_asset_root(dir.to_string_lossy().as_ref().into());
        Ok(())
    }
}

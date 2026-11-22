use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::{self, File},
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
struct SourceList {
    domande: Vec<SourceQuestion>,
}

#[derive(Debug, Deserialize)]
pub struct SourceQuestion {
    pub id: i64,
    pub quesito_id: i64,
    pub argomento_id: i64,
    pub argomento_nome: String,
    pub testo: String,
    pub risposta: bool,
    pub figura: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CsvQuestion {
    id: i64,
    quesito_id: i64,
    argomento_id: i64,
    argomento_nome: String,
    testo: String,
    risposta: String,
    #[serde(default)]
    figura: Option<String>,
}

/// Solo file normalizzati verificati: i PDF ministeriali richiedono un estrattore
/// dedicato e controlli di qualità prima dell'importazione.
pub fn load_normalized_questions(file: &Path) -> Result<Vec<SourceQuestion>> {
    let ext = file
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "json" => read_json(file),
        "csv" => read_csv(file),
        _ => bail!("Formato non supportato: usa un JSON/CSV già normalizzato e verificato"),
    }
}

pub fn ensure_imported(conn: &Connection, version: &str) -> Result<()> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM questions WHERE source_version=?1",
        [version],
        |r| r.get(0),
    )?;
    ensure!(
        total > 0,
        "Banca dati {} non ancora importata. Usa import-for-license con il JSON/CSV verificato.",
        version
    );
    Ok(())
}

pub fn init_database(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS questions (
            card_id TEXT PRIMARY KEY,
            source_version TEXT NOT NULL,
            external_id INTEGER NOT NULL,
            group_id INTEGER NOT NULL,
            topic_id INTEGER NOT NULL,
            topic TEXT NOT NULL,
            question_text TEXT NOT NULL,
            correct_answer INTEGER NOT NULL CHECK(correct_answer IN (0,1)),
            image_path TEXT,
            UNIQUE(source_version, external_id)
        );
        CREATE INDEX IF NOT EXISTS idx_questions_version
          ON questions(source_version);
        CREATE TABLE IF NOT EXISTS study_card_views (
            card_id TEXT PRIMARY KEY NOT NULL REFERENCES questions(card_id) ON DELETE CASCADE,
            first_seen_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS card_state (
            card_id TEXT PRIMARY KEY REFERENCES questions(card_id) ON DELETE CASCADE,
            stability REAL NOT NULL,
            stability_fast REAL NOT NULL,
            difficulty REAL NOT NULL,
            last_review_at INTEGER NOT NULL,
            due_at INTEGER NOT NULL,
            repetitions INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_state_due ON card_state(due_at);
        CREATE TABLE IF NOT EXISTS review_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            card_id TEXT NOT NULL REFERENCES questions(card_id),
            reviewed_at INTEGER NOT NULL,
            answer_correct INTEGER NOT NULL CHECK(answer_correct IN (0,1)),
            rating INTEGER NOT NULL CHECK(rating BETWEEN 1 AND 4),
            elapsed_days REAL NOT NULL,
            predicted_interval_days REAL NOT NULL,
            due_at INTEGER NOT NULL,
            fsrs_version TEXT NOT NULL,
            desired_retention REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_review_card ON review_log(card_id, reviewed_at);",
    )?;
    crate::exam::catalog::migrate(conn)?;
    crate::exam::store::migrate(conn)?;
    Ok(())
}

/// Remember the first observed study/review display, including an unrated card.
pub fn mark_seen(conn: &Connection, card_id: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO study_card_views(card_id,first_seen_at) VALUES (?1,?2)
         ON CONFLICT(card_id) DO NOTHING",
        params![card_id, now],
    )?;
    Ok(())
}

pub fn parse_truth(raw: &str) -> Result<bool> {
    match raw.trim().to_lowercase().as_str() {
        "vero" | "v" | "true" | "1" => Ok(true),
        "falso" | "f" | "false" | "0" => Ok(false),
        _ => bail!("Risposta non valida: {raw:?}; usare VERO o FALSO"),
    }
}

pub fn import_questions(
    conn: &mut Connection,
    version: &str,
    questions: Vec<SourceQuestion>,
) -> Result<usize> {
    ensure!(!version.trim().is_empty(), "Versione mancante");
    ensure!(!questions.is_empty(), "Non ci sono domande da importare");
    let mut ids = HashSet::new();
    for q in &questions {
        ensure!(q.id > 0, "ID non valido: {}", q.id);
        ensure!(!q.testo.trim().is_empty(), "Testo vuoto: ID {}", q.id);
        ensure!(ids.insert(q.id), "ID duplicato nel file: {}", q.id);
    }

    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO questions (
                card_id, source_version, external_id, group_id, topic_id, topic,
                question_text, correct_answer, image_path
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
            ON CONFLICT(card_id) DO UPDATE SET
                group_id=excluded.group_id,
                topic_id=excluded.topic_id,
                topic=excluded.topic,
                question_text=excluded.question_text,
                correct_answer=excluded.correct_answer,
                image_path=excluded.image_path",
        )?;
        for q in &questions {
            // Non associare progressi FSRS a una domanda modificata sotto la stessa versione.
            let card_id = format!("{}:{}", version, q.id);
            let old: Option<(String, i64)> = tx
                .query_row(
                    "SELECT question_text, correct_answer FROM questions WHERE card_id=?1",
                    [&card_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((old_text, old_answer)) = old {
                ensure!(
                    old_text == q.testo && old_answer == q.risposta as i64,
                    "Domanda {} cambiata sotto versione {}: importa con una NUOVA versione",
                    q.id,
                    version
                );
            }
            stmt.execute(params![
                card_id,
                version,
                q.id,
                q.quesito_id,
                q.argomento_id,
                q.argomento_nome,
                q.testo,
                q.risposta as i64,
                q.figura.as_deref(),
            ])?;
        }
    }
    tx.commit()?;
    Ok(questions.len())
}

pub fn import_file(conn: &mut Connection, version: &str, file: &Path) -> Result<usize> {
    import_questions(conn, version, load_normalized_questions(file)?)
}

#[derive(Debug)]
pub struct TopicCount {
    pub id: i64,
    pub name: String,
    pub total: i64,
    pub studied: i64,
    pub due: i64,
}

pub fn topic_counts(conn: &Connection, version: &str) -> Result<Vec<TopicCount>> {
    let mut stmt = conn.prepare(
        "SELECT q.topic_id, q.topic, COUNT(*) AS total,
                SUM(CASE WHEN s.card_id IS NOT NULL THEN 1 ELSE 0 END) AS studied,
                SUM(CASE WHEN s.card_id IS NOT NULL AND s.due_at<=?2 THEN 1 ELSE 0 END) AS due
         FROM questions q LEFT JOIN card_state s ON s.card_id=q.card_id
         WHERE q.source_version=?1
         GROUP BY q.topic_id,q.topic ORDER BY q.topic_id",
    )?;
    Ok(stmt
        .query_map(params![version, Utc::now().timestamp()], |r| {
            Ok(TopicCount {
                id: r.get(0)?,
                name: r.get(1)?,
                total: r.get(2)?,
                studied: r.get(3)?,
                due: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn read_json(file: &Path) -> Result<Vec<SourceQuestion>> {
    let body = fs::read_to_string(file).with_context(|| format!("Lettura {}", file.display()))?;
    let source: SourceList = serde_json::from_str(&body)?;
    Ok(source.domande)
}

pub fn read_csv(file: &Path) -> Result<Vec<SourceQuestion>> {
    let mut reader = csv::Reader::from_path(file)?;
    let mut questions = Vec::new();
    for row in reader.deserialize::<CsvQuestion>() {
        let row = row?;
        questions.push(SourceQuestion {
            id: row.id,
            quesito_id: row.quesito_id,
            argomento_id: row.argomento_id,
            argomento_nome: row.argomento_nome,
            testo: row.testo,
            risposta: parse_truth(&row.risposta)?,
            figura: row.figura.filter(|s| !s.trim().is_empty()),
        });
    }
    Ok(questions)
}

#[derive(Debug)]
pub struct Progress {
    pub total: i64,
    pub studied: i64,
    pub due: i64,
    pub reviews: i64,
}

pub fn progress(conn: &Connection, version: &str) -> Result<Progress> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM questions WHERE source_version=?1",
        [version],
        |r| r.get(0),
    )?;
    let studied: i64 = conn.query_row(
        "SELECT COUNT(*) FROM card_state s JOIN questions q ON q.card_id=s.card_id
         WHERE q.source_version=?1",
        [version],
        |r| r.get(0),
    )?;
    let due: i64 = conn.query_row(
        "SELECT COUNT(*) FROM card_state s JOIN questions q ON q.card_id=s.card_id
         WHERE q.source_version=?1 AND s.due_at<=?2",
        params![version, Utc::now().timestamp()],
        |r| r.get(0),
    )?;
    let reviews: i64 = conn.query_row(
        "SELECT COUNT(*) FROM review_log l JOIN questions q ON q.card_id=l.card_id
         WHERE q.source_version=?1",
        [version],
        |r| r.get(0),
    )?;
    Ok(Progress {
        total,
        studied,
        due,
        reviews,
    })
}
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn write_anki(
    conn: &Connection,
    version: &str,
    output: &Path,
    image_root: Option<PathBuf>,
    media_out: Option<PathBuf>,
) -> Result<usize> {
    ensure!(
        image_root.is_some() == media_out.is_some(),
        "Passa sia --image-root sia --media-out, oppure nessuno dei due"
    );
    if let Some(dir) = &media_out {
        fs::create_dir_all(dir)?;
    }
    let mut w = csv::WriterBuilder::new()
        .delimiter(b'\t')
        .from_writer(File::create(output)?);
    // Primo campo: identificativo stabile per aggiornamenti di note Anki.
    // Dal menu Anki, associa: ID, Front, Back, Tags.
    w.write_record(["ID", "Front", "Back", "Tags"])?;
    let mut stmt = conn.prepare(
        "SELECT card_id,topic,question_text,correct_answer,image_path
         FROM questions WHERE source_version=?1 ORDER BY external_id",
    )?;
    let items = stmt.query_map([version], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)? != 0,
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut count = 0usize;
    for item in items {
        let (id, topic, text, correct, figure) = item?;
        let mut front = escape_html(&text);
        if let (Some(source), Some(dest), Some(relative)) = (&image_root, &media_out, &figure) {
            // Il percorso è trattato come non affidabile: si accetta solo
            // la directory files/figures locale e un nome file semplice.
            let relative_path = Path::new(relative);
            let name = relative_path
                .file_name()
                .context("Nome immagine non valido")?;
            let canonical_root = source.canonicalize()?;
            let canonical_file = source
                .join(relative_path)
                .canonicalize()
                .with_context(|| format!("Immagine non trovata: {relative}"))?;
            ensure!(
                canonical_file.starts_with(&canonical_root),
                "Immagine fuori dalla directory pubblica: {relative}"
            );
            let destination = dest.join(name);
            fs::copy(&canonical_file, &destination)?;
            front.push_str(&format!(
                "<p><img src=\"{}\"></p>",
                escape_html(&name.to_string_lossy())
            ));
        }
        let back = if correct { "VERO" } else { "FALSO" };
        let topic_tag = topic.split_whitespace().collect::<Vec<_>>().join("_");
        let tag = format!("patenti {} {}", version.replace(' ', "_"), topic_tag);
        w.write_record([&id, &front, back, &tag])?;
        count += 1;
    }
    w.flush()?;
    Ok(count)
}

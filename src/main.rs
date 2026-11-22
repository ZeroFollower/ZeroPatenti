use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{TimeZone, Utc};
use clap::{Parser, Subcommand};
use fsrs::{FSRS, MemoryState};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

const AGAIN_DELAY_SECONDS: i64 = 10 * 60;
const SECONDS_PER_DAY: f32 = 86_400.0;

#[derive(Parser)]
#[command(name = "zeropatenti", about = "Quiz per le patenti italiane") ]
struct Cli {
    #[arg(long, global = true, default_value = "patenti.sqlite3")]
    db: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Importa data/questions.json del progetto simulatore-quiz-patente.
    ImportJson {
        file: PathBuf,
        #[arg(long)]
        version: String,
    },
    /// Importa un CSV normalizzato (per altri listati, compreso 7.185).
    ImportCsv {
        file: PathBuf,
        #[arg(long)]
        version: String,
    },
    /// Ripassa domande scadute, poi domande nuove.
    Study {
        #[arg(long)]
        version: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 0.90)]
        retention: f32,
    },
    /// Mostra copertura e scadenze di una versione.
    Stats {
        #[arg(long)]
        version: String,
    },
    /// Esporta note TSV per Anki; non esporta i progressi FSRS.
    ExportAnki {
        #[arg(long)]
        version: String,
        output: PathBuf,
        /// Directory radice delle immagini del repository originale (public/).
        #[arg(long)]
        image_root: Option<PathBuf>,
        /// Directory in cui copiare i media per l'importazione in Anki.
        #[arg(long)]
        media_out: Option<PathBuf>,
    },
}

#[derive(Debug, Deserialize)]
struct SourceList {
    domande: Vec<SourceQuestion>,
}

#[derive(Debug, Deserialize)]
struct SourceQuestion {
    id: i64,
    quesito_id: i64,
    argomento_id: i64,
    argomento_nome: String,
    testo: String,
    risposta: bool,
    figura: Option<String>,
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

#[derive(Debug)]
struct StudyCard {
    id: String,
    topic: String,
    text: String,
    correct: bool,
    figure: Option<String>,
    last_review_at: Option<i64>,
    stability: Option<f32>,
    stability_fast: Option<f32>,
    difficulty: Option<f32>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut conn = Connection::open(&cli.db)
        .with_context(|| format!("Impossibile aprire {}", cli.db.display()))?;
    init_database(&conn)?;

    match cli.command {
        Command::ImportJson { file, version } => {
            let body = fs::read_to_string(&file)
                .with_context(|| format!("Lettura {}", file.display()))?;
            let source: SourceList = serde_json::from_str(&body)
                .context("JSON incompatibile con il formato simulatore-quiz-patente")?;
            import_questions(&mut conn, &version, source.domande)?;
        }
        Command::ImportCsv { file, version } => {
            let mut reader = csv::Reader::from_path(&file)?;
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
            import_questions(&mut conn, &version, questions)?;
        }
        Command::Study { version, limit, retention } => study(&mut conn, &version, limit, retention)?,
        Command::Stats { version } => stats(&conn, &version)?,
        Command::ExportAnki {
            version,
            output,
            image_root,
            media_out,
        } => export_anki(&conn, &version, &output, image_root, media_out)?,
    }
    Ok(())
}

fn init_database(conn: &Connection) -> Result<()> {
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
    Ok(())
}

fn parse_truth(raw: &str) -> Result<bool> {
    match raw.trim().to_lowercase().as_str() {
        "vero" | "v" | "true" | "1" => Ok(true),
        "falso" | "f" | "false" | "0" => Ok(false),
        _ => bail!("Risposta non valida: {raw:?}; usare VERO o FALSO"),
    }
}

fn import_questions(conn: &mut Connection, version: &str, questions: Vec<SourceQuestion>) -> Result<()> {
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
            let old: Option<(String, i64)> = tx.query_row(
                "SELECT question_text, correct_answer FROM questions WHERE card_id=?1",
                [&card_id], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            if let Some((old_text, old_answer)) = old {
                ensure!(old_text == q.testo && old_answer == q.risposta as i64,
                    "Domanda {} cambiata sotto versione {}: importa con una NUOVA versione",
                    q.id, version);
            }
            stmt.execute(params![
                card_id, version, q.id, q.quesito_id,
                q.argomento_id, q.argomento_nome, q.testo, q.risposta as i64,
                q.figura.as_deref(),
            ])?;
        }
    }
    tx.commit()?;
    println!("Importate/aggiornate {} domande per {}. Progressi conservati.", questions.len(), version);
    Ok(())
}

fn due_cards(conn: &Connection, version: &str, limit: usize) -> Result<Vec<StudyCard>> {
    let now = Utc::now().timestamp();
    let mut stmt = conn.prepare(
        "SELECT q.card_id, q.topic, q.question_text, q.correct_answer, q.image_path,
                s.last_review_at, s.stability, s.stability_fast, s.difficulty
         FROM questions q
         LEFT JOIN card_state s ON s.card_id=q.card_id
         WHERE q.source_version=?1 AND (s.card_id IS NULL OR s.due_at<=?2)
         ORDER BY CASE WHEN s.card_id IS NOT NULL THEN 0 ELSE 1 END,
                  CASE WHEN s.card_id IS NOT NULL THEN s.due_at END ASC,
                  RANDOM()
         LIMIT ?3",
    )?;
    let mapped = stmt.query_map(params![version, now, limit as i64], |r| {
        Ok(StudyCard {
            id: r.get(0)?,
            topic: r.get(1)?,
            text: r.get(2)?,
            correct: r.get::<_, i64>(3)? != 0,
            figure: r.get(4)?,
            last_review_at: r.get(5)?,
            stability: r.get(6)?,
            stability_fast: r.get(7)?,
            difficulty: r.get(8)?,
        })
    })?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut input = String::new();
    ensure!(io::stdin().read_line(&mut input)? > 0, "Input terminato");
    Ok(input.trim().to_lowercase())
}

fn study(conn: &mut Connection, version: &str, limit: usize, retention: f32) -> Result<()> {
    ensure!(limit > 0 && limit <= 200, "Il limite deve essere tra 1 e 200");
    ensure!(retention.is_finite() && (0.70..=0.99).contains(&retention), "Retention tra 0.70 e 0.99");
    let cards = due_cards(conn, version, limit)?;
    if cards.is_empty() {
        println!("Nessuna domanda scaduta o nuova per {version}.");
        return Ok(());
    }
    let fsrs = FSRS::default(); // fsrs 7.0.0 => parametri FSRS-7 (34).
    let mut correct_count = 0usize;
    let mut answered_count = 0usize;

    for (index, card) in cards.iter().enumerate() {
        println!("\n[{}/{}] {} – {}", index + 1, cards.len(), card.id, card.topic);
        println!("{}", card.text);
        if let Some(figure) = &card.figure {
            println!("Figura: {figure} (aprila nella directory public/ del dataset)");
        }
        let guess = loop {
            let value = prompt("Risposta [V/F] o [Q] per uscire: ")?;
            match value.as_str() {
                "v" | "vero" => break Some(true),
                "f" | "falso" => break Some(false),
                "q" => break None,
                _ => println!("Digita V, F oppure Q."),
            }
        };
        let Some(guess) = guess else { break };
        answered_count += 1;
        let correct = guess == card.correct;
        if correct { correct_count += 1; }
        println!(
            "{} · Soluzione ufficiale: {}",
            if correct { "✅ CORRETTO" } else { "❌ ERRATO" },
            if card.correct { "VERO" } else { "FALSO" }
        );
        let rating = if correct {
            loop {
                let choice = prompt("Quanto è stato difficile? [1] Da ripassare [2] Difficile [3] Normale [4] Facile (Invio=3): ")?;
                match choice.as_str() {
                    "1" => break 1u8,
                    "2" => break 2u8,
                    "" | "3" => break 3u8,
                    "4" => break 4u8,
                    _ => println!("Inserisci 1, 2, 3, 4 o Invio."),
                }
            }
        } else {
            1u8 // Una risposta errata non può diventare Good o Easy.
        };

        let now = Utc::now().timestamp();
        let previous = match (card.stability, card.stability_fast, card.difficulty) {
            (Some(stability), Some(stability_fast), Some(difficulty)) => Some(MemoryState {
                stability,
                stability_fast,
                difficulty,
            }),
            (None, None, None) => None,
            _ => bail!("Stato di studio incompleto nel database: {}", card.id),
        };
        let elapsed = card.last_review_at
            .map(|at| ((now - at).max(0) as f32) / SECONDS_PER_DAY)
            .unwrap_or(0.0);
        let next = fsrs.next_states_with_elapsed_days(previous, retention, elapsed)
            .context("Calcolo FSRS-7 fallito")?;
        let selected = match rating {
            1 => next.again,
            2 => next.hard,
            3 => next.good,
            4 => next.easy,
            _ => unreachable!(),
        };
        let due_at = if rating == 1 {
            now + AGAIN_DELAY_SECONDS // passo di riapprendimento applicativo
        } else {
            let seconds = (selected.interval.max(1.0) * SECONDS_PER_DAY).round() as i64;
            now + seconds
        };
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO card_state (
                card_id,stability,stability_fast,difficulty,last_review_at,due_at,repetitions
             ) VALUES (?1,?2,?3,?4,?5,?6,1)
             ON CONFLICT(card_id) DO UPDATE SET
                stability=excluded.stability,
                stability_fast=excluded.stability_fast,
                difficulty=excluded.difficulty,
                last_review_at=excluded.last_review_at,
                due_at=excluded.due_at,
                repetitions=repetitions+1",
            params![card.id, selected.memory.stability, selected.memory.stability_fast,
                selected.memory.difficulty, now, due_at],
        )?;
        tx.execute(
            "INSERT INTO review_log (
                card_id,reviewed_at,answer_correct,rating,elapsed_days,
                predicted_interval_days,due_at,fsrs_version,desired_retention
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,'7.0.0',?8)",
            params![card.id, now, correct as i64, rating as i64, elapsed,
                selected.interval, due_at, retention],
        )?;
        tx.commit()?;
        let when = Utc.timestamp_opt(due_at, 0).single()
            .map(|dt| dt.to_rfc3339()).unwrap_or_else(|| due_at.to_string());
        println!("Prossimo ripasso: {when}");
    }
    println!("\nRisposte corrette nella sessione: {correct_count}/{answered_count}.");
    Ok(())
}

fn stats(conn: &Connection, version: &str) -> Result<()> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM questions WHERE source_version=?1", [version], |r| r.get(0)
    )?;
    let studied: i64 = conn.query_row(
        "SELECT COUNT(*) FROM card_state s JOIN questions q ON q.card_id=s.card_id
         WHERE q.source_version=?1", [version], |r| r.get(0)
    )?;
    let due: i64 = conn.query_row(
        "SELECT COUNT(*) FROM card_state s JOIN questions q ON q.card_id=s.card_id
         WHERE q.source_version=?1 AND s.due_at<=?2",
        params![version, Utc::now().timestamp()], |r| r.get(0)
    )?;
    let reviews: i64 = conn.query_row(
        "SELECT COUNT(*) FROM review_log l JOIN questions q ON q.card_id=l.card_id
         WHERE q.source_version=?1", [version], |r| r.get(0)
    )?;
    let coverage = if total == 0 { 0.0 } else { 100.0 * studied as f64 / total as f64 };
    println!("Versione: {version}");
    println!("Domande: {total} · Studiate: {studied} · Nuove: {}", total - studied);
    println!("Scadute: {due} · Ripassi totali: {reviews} · Copertura: {coverage:.1}%");
    Ok(())
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn export_anki(
    conn: &Connection,
    version: &str,
    output: &Path,
    image_root: Option<PathBuf>,
    media_out: Option<PathBuf>,
) -> Result<()> {
    ensure!(image_root.is_some() == media_out.is_some(),
        "Passa sia --image-root sia --media-out, oppure nessuno dei due");
    if let Some(dir) = &media_out { fs::create_dir_all(dir)?; }
    let mut w = csv::WriterBuilder::new().delimiter(b'\t').from_writer(File::create(output)?);
    // Primo campo: identificativo stabile per aggiornamenti di note Anki.
    // Dal menu Anki, associa: ID, Front, Back, Tags.
    w.write_record(["ID", "Front", "Back", "Tags"])?;
    let mut stmt = conn.prepare(
        "SELECT card_id,topic,question_text,correct_answer,image_path
         FROM questions WHERE source_version=?1 ORDER BY external_id",
    )?;
    let items = stmt.query_map([version], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, String>(2)?, r.get::<_, i64>(3)? != 0,
            r.get::<_, Option<String>>(4)?))
    })?;
    let mut count = 0usize;
    for item in items {
        let (id, topic, text, correct, figure) = item?;
        let mut front = escape_html(&text);
        if let (Some(source), Some(dest), Some(relative)) = (&image_root, &media_out, &figure) {
            // Il percorso è trattato come non affidabile: si accetta solo
            // la directory files/figures locale e un nome file semplice.
            let relative_path = Path::new(relative);
            let name = relative_path.file_name().context("Nome immagine non valido")?;
            let canonical_root = source.canonicalize()?;
            let canonical_file = source.join(relative_path).canonicalize()
                .with_context(|| format!("Immagine non trovata: {relative}"))?;
            ensure!(canonical_file.starts_with(&canonical_root),
                "Immagine fuori dalla directory pubblica: {relative}");
            let destination = dest.join(name);
            fs::copy(&canonical_file, &destination)?;
            front.push_str(&format!("<p><img src=\"{}\"></p>",
                escape_html(&name.to_string_lossy())));
        }
        let back = if correct { "VERO" } else { "FALSO" };
        let topic_tag = topic.split_whitespace().collect::<Vec<_>>().join("_");
        let tag = format!("patenti {} {}", version.replace(' ', "_"), topic_tag);
        w.write_record([&id, &front, back, &tag])?;
        count += 1;
    }
    w.flush()?;
    println!("Esportate {count} note in {}", output.display());
    if media_out.is_some() {
        println!("Copia i media esportati nella cartella media del profilo Anki prima di importare il TSV.");
    } else {
        println!("Nota: immagini non incluse. Usa --image-root e --media-out per esportarle.");
    }
    println!("I progressi locali NON vengono esportati in Anki.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truth_parsing() {
        assert!(parse_truth("VERO").unwrap());
        assert!(!parse_truth("Falso").unwrap());
        assert!(parse_truth("altro").is_err());
    }

    #[test]
    fn fsrs7_updates_state() {
        let scheduler = FSRS::default();
        let states = scheduler.next_states_with_elapsed_days(None, 0.90, 0.0).unwrap();
        assert!(states.good.memory.stability.is_finite());
        assert!(states.good.memory.stability_fast.is_finite());
        assert!(states.good.memory.difficulty.is_finite());
        assert!(states.good.interval > 0.0);
    }

    #[test]
    fn import_is_idempotent_and_preserves_progress() {
        let mut db = Connection::open_in_memory().unwrap();
        init_database(&db).unwrap();
        let make = || SourceQuestion {
            id: 1, quesito_id: 7, argomento_id: 2,
            argomento_nome: "Segnali".into(), testo: "Quiz dimostrativo".into(),
            risposta: true, figura: None,
        };
        import_questions(&mut db, "AB-TEST", vec![make()]).unwrap();
        db.execute("INSERT INTO card_state VALUES ('AB-TEST:1',1,1,5,1,2,1)", []).unwrap();
        import_questions(&mut db, "AB-TEST", vec![make()]).unwrap();
        let reviews: i64 = db.query_row(
            "SELECT repetitions FROM card_state WHERE card_id='AB-TEST:1'",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(reviews, 1);
    }
}
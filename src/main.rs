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
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use serde::Deserialize;

mod catalog;

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
    /// Elenca le banche dati ufficiali catalogate e le patenti supportate.
    Catalog,
    /// Importa un listato JSON/CSV normalizzato per la patente indicata.
    ImportForLicense {
        file: PathBuf,
        #[arg(long)]
        license: String,
        #[arg(long, default_value = "conseguimento")]
        procedure: String,
    },
    /// Studia il listato corretto in base alla categoria.
    StudyForLicense {
        #[arg(long)]
        license: String,
        #[arg(long, default_value = "conseguimento")]
        procedure: String,
        #[arg(long = "topic-id", value_delimiter = ',', num_args = 1..)]
        topics: Vec<i64>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 0.90)]
        retention: f32,
    },
    /// Elenca gli argomenti del listato per la patente scelta.
    TopicsForLicense {
        #[arg(long)]
        license: String,
        #[arg(long, default_value = "conseguimento")]
        procedure: String,
    },
    /// Ripassa esclusivamente domande già studiate e scadute.
    ReviewForLicense {
        #[arg(long)]
        license: String,
        #[arg(long, default_value = "conseguimento")]
        procedure: String,
        #[arg(long = "topic-id", value_delimiter = ',', num_args = 1..)]
        topics: Vec<i64>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 0.90)]
        retention: f32,
    },
    /// Statistiche della patente e procedura scelte.
    StatsForLicense {
        #[arg(long)]
        license: String,
        #[arg(long, default_value = "conseguimento")]
        procedure: String,
    },
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
    /// Elenca gli argomenti disponibili di una banca dati.
    Topics {
        #[arg(long)]
        version: String,
    },
    /// Ripassa solo le domande già studiate e scadute del set.
    Review {
        #[arg(long)]
        version: String,
        #[arg(long = "topic-id", value_delimiter = ',', num_args = 1..)]
        topics: Vec<i64>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = 0.90)]
        retention: f32,
    },
    /// Ripassa domande scadute, poi domande nuove.
    Study {
        #[arg(long)]
        version: String,
        #[arg(long = "topic-id", value_delimiter = ',', num_args = 1..)]
        topics: Vec<i64>,
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
        Command::Catalog => catalog::print()?,
        Command::ImportForLicense { file, license, procedure } => {
            let version = catalog::version_for(&license, &procedure)?;
            let questions = load_normalized_questions(&file)?;
            if version == "AB-7020" {
                ensure!(questions.len() == 7020,
                    "Il set A/B da 7.020 affermazioni ne contiene {}. Importazione rifiutata.",
                    questions.len());
            }
            println!("Patente {} ({}) -> banca dati {}", license, procedure, version);
            import_questions(&mut conn, &version, questions)?;
        }
        Command::StudyForLicense { license, procedure, topics, limit, retention } => {
            let version = catalog::version_for(&license, &procedure)?;
            ensure_imported(&conn, &version)?;
            study(&mut conn, &version, limit, retention, &topics, false)?;
        }
        Command::TopicsForLicense { license, procedure } => {
            let version = catalog::version_for(&license, &procedure)?;
            topics(&conn, &version)?;
        }
        Command::ReviewForLicense { license, procedure, topics, limit, retention } => {
            let version = catalog::version_for(&license, &procedure)?;
            ensure_imported(&conn, &version)?;
            study(&mut conn, &version, limit, retention, &topics, true)?;
        }
        Command::StatsForLicense { license, procedure } => {
            let version = catalog::version_for(&license, &procedure)?;
            stats(&conn, &version)?;
        }
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
        Command::Study { version, topics, limit, retention } =>
            study(&mut conn, &version, limit, retention, &topics, false)?,
        Command::Review { version, topics, limit, retention } =>
            study(&mut conn, &version, limit, retention, &topics, true)?,
        Command::Topics { version } => topics(&conn, &version)?,
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

/// Solo file normalizzati verificati: i PDF ministeriali richiedono un estrattore
/// dedicato e controlli di qualità prima dell'importazione.
fn load_normalized_questions(file: &Path) -> Result<Vec<SourceQuestion>> {
    let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "json" => {
            let body = fs::read_to_string(file)
                .with_context(|| format!("Lettura {}", file.display()))?;
            let source: SourceList = serde_json::from_str(&body)?;
            Ok(source.domande)
        }
        "csv" => {
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
        _ => bail!("Formato non supportato: usa un JSON/CSV già normalizzato e verificato"),
    }
}

fn ensure_imported(conn: &Connection, version: &str) -> Result<()> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM questions WHERE source_version=?1", [version], |r| r.get(0)
    )?;
    ensure!(total > 0,
        "Banca dati {} non ancora importata. Usa import-for-license con il JSON/CSV verificato.",
        version);
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

/// Elenca gli argomenti del listato corrente, con conteggi verificabili.
fn topics(conn: &Connection, version: &str) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT q.topic_id, q.topic, COUNT(*) AS total,
                SUM(CASE WHEN s.card_id IS NOT NULL THEN 1 ELSE 0 END) AS studied,
                SUM(CASE WHEN s.card_id IS NOT NULL AND s.due_at<=?2 THEN 1 ELSE 0 END) AS due
         FROM questions q LEFT JOIN card_state s ON s.card_id=q.card_id
         WHERE q.source_version=?1
         GROUP BY q.topic_id,q.topic ORDER BY q.topic_id"
    )?;
    let rows=stmt.query_map(params![version,Utc::now().timestamp()],|r|{
        Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,
            r.get::<_,i64>(3)?,r.get::<_,i64>(4)?))
    })?;
    let mut count=0;
    for row in rows {
        let (id,name,total,studied,due)=row?;
        println!("{id:>2} | {name} | totali {total} | studiate {studied} | da ripassare {due}");
        count+=1;
    }
    ensure!(count>0,"Nessun argomento importato per il listato {version}");
    Ok(())
}

/// Convalida gli ID delle materie per evitare selezioni vuote dovute a errori
/// e controlla la versione: gli ID non sono globali tra edizioni diverse.
fn validate_topics(conn: &Connection, version: &str, selected: &[i64]) -> Result<Vec<i64>> {
    let mut ids=selected.to_vec();
    ids.sort_unstable();
    ids.dedup();
    for id in &ids {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM questions WHERE source_version=?1 AND topic_id=?2",
            params![version,id],|r|r.get(0))?;
        ensure!(exists>0,"Argomento ID {id} inesistente nel listato {version}. Usa il comando topics.");
    }
    Ok(ids)
}

/// Solo domande scadute o nuove in Studio, SOLO scadute già viste in Ripasso.
/// Filtro applicato in SQL prima della selezione: non vengono mai pescate
/// domande di altri argomenti.
fn due_cards(conn: &Connection, version: &str, limit: usize, selected: &[i64], only_reviews: bool)
    -> Result<Vec<StudyCard>> {
    let ids=validate_topics(conn,version,selected)?;
    let now=Utc::now().timestamp();
    let mut query=String::from(
        "SELECT q.card_id,q.topic,q.question_text,q.correct_answer,q.image_path,
                s.last_review_at,s.stability,s.stability_fast,s.difficulty
         FROM questions q LEFT JOIN card_state s ON s.card_id=q.card_id
         WHERE q.source_version=?1 AND ");
    if only_reviews {
        query.push_str("s.card_id IS NOT NULL AND s.due_at<=?2 ");
    } else {
        query.push_str("(s.card_id IS NULL OR s.due_at<=?2) ");
    }
    let mut args=vec![Value::Text(version.to_string()),Value::Integer(now)];
    if !ids.is_empty() {
        let placeholders=(0..ids.len()).map(|i|format!("?{}",i+3)).collect::<Vec<_>>().join(",");
        query.push_str(&format!(" AND q.topic_id IN ({placeholders}) "));
        args.extend(ids.into_iter().map(Value::Integer));
    }
    query.push_str(
        " ORDER BY CASE WHEN s.card_id IS NOT NULL THEN 0 ELSE 1 END,
          CASE WHEN s.card_id IS NOT NULL THEN s.due_at END ASC,
          RANDOM()");
    // LIMIT inline is safe here because usize is parsed and bounded separately.
    query.push_str(&format!(" LIMIT {}",limit));
    let mut stmt=conn.prepare(&query)?;
    let mapped=stmt.query_map(params_from_iter(args),|r|{
        Ok(StudyCard {
            id:r.get(0)?,
            topic:r.get(1)?,
            text:r.get(2)?,
            correct:r.get::<_,i64>(3)?!=0,
            figure:r.get(4)?,
            last_review_at:r.get(5)?,
            stability:r.get(6)?,
            stability_fast:r.get(7)?,
            difficulty:r.get(8)?,
        })
    })?;
    Ok(mapped.collect::<std::result::Result<Vec<_>,_>>()?)
}

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut input = String::new();
    ensure!(io::stdin().read_line(&mut input)? > 0, "Input terminato");
    Ok(input.trim().to_lowercase())
}

fn study(conn: &mut Connection, version: &str, limit: usize, retention: f32, selected_topics: &[i64], only_reviews: bool) -> Result<()> {
    ensure!(limit > 0 && limit <= 200, "Il limite deve essere tra 1 e 200");
    ensure!(retention.is_finite() && (0.70..=0.99).contains(&retention), "Retention tra 0.70 e 0.99");
    let cards = due_cards(conn, version, limit, selected_topics, only_reviews)?;
    if cards.is_empty() {
        println!("Nessuna domanda disponibile per il filtro scelto nel listato {version}.");
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
    fn topic_filter_never_leaks_other_topics() {
        let mut db = Connection::open_in_memory().unwrap();
        init_database(&db).unwrap();
        let questions = vec![
            SourceQuestion { id:1, quesito_id:1, argomento_id:11,
                argomento_nome:"Segnali".into(), testo:"Q1".into(),
                risposta:true, figura:None },
            SourceQuestion { id:2, quesito_id:2, argomento_id:22,
                argomento_nome:"Precedenze".into(), testo:"Q2".into(),
                risposta:false, figura:None },
            SourceQuestion { id:3, quesito_id:3, argomento_id:11,
                argomento_nome:"Segnali".into(), testo:"Q3".into(),
                risposta:true, figura:None },
        ];
        import_questions(&mut db,"FILTER-TEST",questions).unwrap();
        let selected = due_cards(&db,"FILTER-TEST",10,&[11],false).unwrap();
        assert_eq!(selected.len(),2);
        assert!(selected.iter().all(|c|c.topic=="Segnali"));
        assert!(validate_topics(&db,"FILTER-TEST",&[999]).is_err());
        assert_eq!(due_cards(&db,"FILTER-TEST",10,&[11],true).unwrap().len(),0);
        // Un solo ripasso in scadenza per la materia 11, mai la materia 22.
        db.execute("INSERT INTO card_state VALUES ('FILTER-TEST:1',1,1,5,1,2,1)",[]).unwrap();
        db.execute("INSERT INTO card_state VALUES ('FILTER-TEST:2',1,1,5,1,2,1)",[]).unwrap();
        let reviews=due_cards(&db,"FILTER-TEST",10,&[11],true).unwrap();
        assert_eq!(reviews.len(),1);
        assert_eq!(reviews[0].id,"FILTER-TEST:1");
        assert_eq!(due_cards(&db,"FILTER-TEST",10,&[22],true).unwrap().len(),1);
    }

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
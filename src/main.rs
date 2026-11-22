use anyhow::{Context, Result, ensure};
use chrono::{TimeZone, Utc};
use clap::{Parser, Subcommand};
use rusqlite::Connection;
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};
use zeropatenti::{
    catalog,
    review::{apply_review, due_cards},
    storage::*,
};

#[derive(Parser)]
#[command(name = "zeropatenti", about = "Quiz per le patenti italiane")]
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut conn = Connection::open(&cli.db)
        .with_context(|| format!("Impossibile aprire {}", cli.db.display()))?;
    init_database(&conn)?;

    match cli.command {
        Command::Catalog => catalog::print()?,
        Command::ImportForLicense {
            file,
            license,
            procedure,
        } => {
            let version = catalog::version_for(&license, &procedure)?;
            let questions = load_normalized_questions(&file)?;
            if version == "AB-7020" {
                ensure!(
                    questions.len() == 7020,
                    "Il set A/B da 7.020 affermazioni ne contiene {}. Importazione rifiutata.",
                    questions.len()
                );
            }
            println!(
                "Patente {} ({}) -> banca dati {}",
                license, procedure, version
            );
            report_import(&mut conn, &version, questions)?;
        }
        Command::StudyForLicense {
            license,
            procedure,
            topics,
            limit,
            retention,
        } => {
            let version = catalog::version_for(&license, &procedure)?;
            ensure_imported(&conn, &version)?;
            study(&mut conn, &version, limit, retention, &topics, false)?;
        }
        Command::TopicsForLicense { license, procedure } => {
            let version = catalog::version_for(&license, &procedure)?;
            topics(&conn, &version)?;
        }
        Command::ReviewForLicense {
            license,
            procedure,
            topics,
            limit,
            retention,
        } => {
            let version = catalog::version_for(&license, &procedure)?;
            ensure_imported(&conn, &version)?;
            study(&mut conn, &version, limit, retention, &topics, true)?;
        }
        Command::StatsForLicense { license, procedure } => {
            let version = catalog::version_for(&license, &procedure)?;
            stats(&conn, &version)?;
        }
        Command::ImportJson { file, version } => {
            report_import(&mut conn, &version, read_json(&file)?)?;
        }
        Command::ImportCsv { file, version } => {
            report_import(&mut conn, &version, read_csv(&file)?)?;
        }
        Command::Study {
            version,
            topics,
            limit,
            retention,
        } => study(&mut conn, &version, limit, retention, &topics, false)?,
        Command::Review {
            version,
            topics,
            limit,
            retention,
        } => study(&mut conn, &version, limit, retention, &topics, true)?,
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
fn report_import(
    conn: &mut Connection,
    version: &str,
    questions: Vec<SourceQuestion>,
) -> Result<()> {
    let count = import_questions(conn, version, questions)?;
    println!("Importate/aggiornate {count} domande per {version}. Progressi conservati.");
    Ok(())
}

fn topics(conn: &Connection, version: &str) -> Result<()> {
    let rows = topic_counts(conn, version)?;
    ensure!(
        !rows.is_empty(),
        "Nessun argomento importato per il listato {version}"
    );
    for TopicCount {
        id,
        name,
        total,
        studied,
        due,
    } in rows
    {
        println!("{id:>2} | {name} | totali {total} | studiate {studied} | da ripassare {due}");
    }
    Ok(())
}
fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut input = String::new();
    ensure!(io::stdin().read_line(&mut input)? > 0, "Input terminato");
    Ok(input.trim().to_lowercase())
}

fn study(
    conn: &mut Connection,
    version: &str,
    limit: usize,
    retention: f32,
    selected_topics: &[i64],
    only_reviews: bool,
) -> Result<()> {
    ensure!(
        limit > 0 && limit <= 200,
        "Il limite deve essere tra 1 e 200"
    );
    ensure!(
        retention.is_finite() && (0.70..=0.99).contains(&retention),
        "Retention tra 0.70 e 0.99"
    );
    let cards = due_cards(conn, version, limit, selected_topics, only_reviews)?;
    if cards.is_empty() {
        println!("Nessuna domanda disponibile per il filtro scelto nel listato {version}.");
        return Ok(());
    }
    let mut correct_count = 0usize;
    let mut answered_count = 0usize;

    for (index, card) in cards.iter().enumerate() {
        println!(
            "\n[{}/{}] {} – {}",
            index + 1,
            cards.len(),
            card.id,
            card.topic
        );
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
        if correct {
            correct_count += 1;
        }
        println!(
            "{} · Soluzione ufficiale: {}",
            if correct {
                "✅ CORRETTO"
            } else {
                "❌ ERRATO"
            },
            if card.correct { "VERO" } else { "FALSO" }
        );
        let rating = if correct {
            loop {
                let choice = prompt(
                    "Quanto è stato difficile? [1] Da ripassare [2] Difficile [3] Normale [4] Facile (Invio=3): ",
                )?;
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
        let tx = conn.transaction()?;
        let due_at = apply_review(
            &tx,
            &card.id,
            correct,
            rating,
            Utc::now().timestamp(),
            retention,
        )?
        .due_at;
        tx.commit()?;
        let when = Utc
            .timestamp_opt(due_at, 0)
            .single()
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| due_at.to_string());
        println!("Prossimo ripasso: {when}");
    }
    println!("\nRisposte corrette nella sessione: {correct_count}/{answered_count}.");
    Ok(())
}

fn stats(conn: &Connection, version: &str) -> Result<()> {
    let Progress {
        total,
        studied,
        due,
        reviews,
    } = progress(conn, version)?;
    let coverage = if total == 0 {
        0.0
    } else {
        100.0 * studied as f64 / total as f64
    };
    println!("Versione: {version}");
    println!(
        "Domande: {total} · Studiate: {studied} · Nuove: {}",
        total - studied
    );
    println!("Scadute: {due} · Ripassi totali: {reviews} · Copertura: {coverage:.1}%");
    Ok(())
}

fn export_anki(
    conn: &Connection,
    version: &str,
    output: &Path,
    image_root: Option<PathBuf>,
    media_out: Option<PathBuf>,
) -> Result<()> {
    let has_media = media_out.is_some();
    let count = write_anki(conn, version, output, image_root, media_out)?;
    println!("Esportate {count} note in {}", output.display());
    if has_media {
        println!(
            "Copia i media esportati nella cartella media del profilo Anki prima di importare il TSV."
        );
    } else {
        println!("Nota: immagini non incluse. Usa --image-root e --media-out per esportarle.");
    }
    println!("I progressi locali NON vengono esportati in Anki.");
    Ok(())
}

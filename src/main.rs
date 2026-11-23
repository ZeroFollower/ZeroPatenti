use anyhow::{Context, Result, ensure};
use chrono::{TimeZone, Utc};
use clap::{Args, Parser, Subcommand};
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
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct StudyLength {
    /// Numero di domande della sessione.
    #[arg(long, default_value_t = 20)]
    limit: usize,
    /// Prosegui fino a Q o all’esaurimento delle domande disponibili.
    #[arg(long, conflicts_with = "limit")]
    unlimited: bool,
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
        #[command(flatten)]
        length: StudyLength,
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
        #[command(flatten)]
        length: StudyLength,
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
        #[command(flatten)]
        length: StudyLength,
        #[arg(long, default_value_t = 0.90)]
        retention: f32,
    },
    /// Ripassa domande scadute, poi domande nuove.
    Study {
        #[arg(long)]
        version: String,
        #[arg(long = "topic-id", value_delimiter = ',', num_args = 1..)]
        topics: Vec<i64>,
        #[command(flatten)]
        length: StudyLength,
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
    let profile = cli
        .db
        .map(Ok)
        .unwrap_or_else(zeropatenti::datasets::desktop_profile)?;
    let mut conn = Connection::open(&profile)
        .with_context(|| format!("Impossibile aprire {}", profile.display()))?;
    init_database(&conn)?;

    let for_license = |db: &mut Connection, license: &str, procedure: &str| -> Result<String> {
        let base = catalog::version_for(license, procedure)?;
        use zeropatenti::datasets as banks;
        if base != banks::LEGACY {
            return Ok(base);
        }
        let latest = banks::resolve(&banks::folder(&profile), banks::latest_installed(db)?, None)?;
        banks::remember_published(db, latest.published.as_deref())?;
        let known = banks::known_latest(db, &banks::folder(&profile))?;
        if latest.year < known {
            eprintln!("{}", banks::warning(latest.year, known));
        }
        if latest.published.is_none() {
            eprintln!(
                "GitHub non raggiungibile. Uso il dataset locale A/B {}.",
                latest.year
            );
        }
        let version = banks::edition(latest.year);
        if let Some(file) = latest.path
            && banks::installed_digest(db, &version)?.is_none()
        {
            banks::import(db, &file)
        } else {
            Ok(version)
        }
    };
    let explicit = match &cli.command {
        Command::Study { version, .. }
        | Command::Review { version, .. }
        | Command::Topics { version }
        | Command::Stats { version }
        | Command::ExportAnki { version, .. }
        | Command::ImportCsv { version, .. }
        | Command::ImportJson { version, .. } => zeropatenti::datasets::year_of_version(version),
        Command::ImportForLicense {
            license, procedure, ..
        } => (catalog::version_for(license, procedure)? == zeropatenti::datasets::LEGACY)
            .then_some(2024),
        _ => None,
    };
    if let Some(year) = explicit {
        use zeropatenti::datasets as banks;
        if let Ok(years) = banks::published_years() {
            banks::remember_published(&mut conn, Some(&years))?;
        }
        let latest = banks::known_latest(&conn, &banks::folder(&profile))?;
        if year < latest {
            eprintln!("{}", banks::warning(year, latest));
        }
    }
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
            length,
            retention,
        } => {
            let version = for_license(&mut conn, &license, &procedure)?;
            ensure_imported(&conn, &version)?;
            study(&mut conn, &version, length, retention, &topics, false)?;
        }
        Command::TopicsForLicense { license, procedure } => {
            let version = for_license(&mut conn, &license, &procedure)?;
            topics(&conn, &version)?;
        }
        Command::ReviewForLicense {
            license,
            procedure,
            topics,
            length,
            retention,
        } => {
            let version = for_license(&mut conn, &license, &procedure)?;
            ensure_imported(&conn, &version)?;
            study(&mut conn, &version, length, retention, &topics, true)?;
        }
        Command::StatsForLicense { license, procedure } => {
            let version = for_license(&mut conn, &license, &procedure)?;
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
            length,
            retention,
        } => study(&mut conn, &version, length, retention, &topics, false)?,
        Command::Review {
            version,
            topics,
            length,
            retention,
        } => study(&mut conn, &version, length, retention, &topics, true)?,
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
    length: StudyLength,
    retention: f32,
    selected_topics: &[i64],
    only_reviews: bool,
) -> Result<()> {
    ensure!(
        (1..=200).contains(&length.limit),
        "Il limite deve essere tra 1 e 200"
    );
    ensure!(
        retention.is_finite() && (0.70..=0.99).contains(&retention),
        "Retention tra 0.70 e 0.99"
    );
    let mut batch = if length.unlimited {
        Vec::new()
    } else {
        due_cards(conn, version, length.limit, selected_topics, only_reviews)?
    }
    .into_iter();
    let total = batch.len();
    let mut correct_count = 0usize;
    let mut answered_count = 0usize;

    loop {
        let next = if length.unlimited {
            due_cards(conn, version, 1, selected_topics, only_reviews)?.pop()
        } else {
            batch.next()
        };
        let Some(card) = next else {
            if answered_count == 0 || length.unlimited {
                println!("Nessuna domanda disponibile per il filtro scelto nel listato {version}.");
            }
            if answered_count == 0 {
                return Ok(());
            }
            break;
        };
        let number = answered_count + 1;
        if length.unlimited {
            println!("\n[{number}] {} – {}", card.id, card.topic);
        } else {
            println!("\n[{number}/{total}] {} – {}", card.id, card.topic);
        }
        println!("{}", card.text);
        if let Some(figure) = &card.figure {
            println!("Figura: {figure} (aprila nella directory public/ del dataset)");
        }
        mark_seen(conn, &card.id, Utc::now().timestamp())?;
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

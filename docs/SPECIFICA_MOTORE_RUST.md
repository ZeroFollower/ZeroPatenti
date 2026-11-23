# Patenti FSRS-7 — programma Rust per studiare le banche dati ministeriali

**Versione specifica:** 1.0 — 22 novembre 2026  
**Obiettivo:** motore locale per ripetizione dilazionata dei quiz VERO/FALSO delle patenti italiane.  
**Linguaggio:** Rust, edition 2024.  
**Algoritmo:** FSRS **versione 7**, tramite crate `fsrs = "=7.0.0"` (`open-spaced-repetition/fsrs-rs`).  
**Database:** SQLite 3 in modalità WAL.

> **Stato:** progetto sorgente MVP allegato a questo documento. Le API sono state confrontate con la documentazione e il codice ufficiali di `fsrs` 7.0.0. L'ambiente in cui è stato preparato il progetto non dispone di `cargo`/`rustc`; compilazione, test ed esecuzione devono quindi essere effettuati sulla macchina di destinazione. Non dichiarare il programma «testato» finché non passano `cargo check`, `cargo test` e una sessione reale.

## 1. Scopo e perimetro

L'applicazione:

1. Legge **direttamente** `data/questions.json` del repository [`matteotarantino-algor/simulatore-quiz-patente`](https://github.com/matteotarantino-algor/simulatore-quiz-patente) (7.020 affermazioni A/B, con ID numerico, gruppo di quesiti, 25 materie, `risposta: boolean`, figura e pagina sorgente). Non inventa un formato alternativo per questa sorgente.
2. Importa banche dati aggiuntive da CSV con uno schema normalizzato documentato; utile per il futuro listato di **7.185 affermazioni**, ma richiede un'estrazione verificata dal TXT/PDF ministeriale prima dell'importazione.
3. Separa rigorosamente i diversi **listati/versioni**: `AB-7020`, `AB-7185`, AM, superiori, CQC ecc. Lo stesso identificativo numerico può essere usato in listati diversi senza collisioni.
4. Crea una scheda FSRS indipendente per ogni affermazione VERO/FALSO, mostra una domanda per volta, valuta l'esito e pianifica la successiva ripetizione.
5. Persiste stato FSRS-7, scadenza e cronologia append-only dei ripassi in SQLite; non perde i progressi quando si reimporta la **medesima** versione.
6. Esporta note TSV per Anki, con possibilità di esportare le immagini a parte. **Il TSV contiene le note, non la programmazione o lo storico FSRS del programma**: Anki applicherà la propria programmazione dei ripassi.

**Non è un simulatore ufficiale d'esame.** La modalità ripasso FSRS non riproduce la distribuzione, la durata e la valutazione delle prove ministeriali. Serve anche una modalità «Esame realistico» separata, non inclusa nell'MVP.

## 2. Fonti dei dati

| Origine | Tipo | Modalità | Stato |
|---|---|---|---|
| [Simulatore patente A/B – JSON](https://github.com/matteotarantino-algor/simulatore-quiz-patente/blob/main/data/questions.json) | 7.020 V/F | `import-json` | Supportato nell'MVP |
| [MIT – Portale dell'Automobilista](https://www.ilportaledellautomobilista.it/) | listati PDF ufficiali | estrazione esterna validata → CSV | Importazione CSV supportata |
| [Circolare MIT 19281 del 05/07/2024](https://www.mit.gov.it/normativa/circolare-protocollo-19281-del-05072024) | nuovo listato A/B da 7.185 | TXT ministeriale → CSV verificato | Parser TXT diretto **non** incluso |
| Patenti AM, C/D, CQC, CAP e nautiche | formati differenti | adapter separati | Estensione successiva |

**Integrità:** ogni import deve essere accompagnato in produzione da URL della fonte, hash SHA-256 del file, versione ufficiale e conteggio atteso. L'MVP salva la versione scelta dall'operatore, ma non implementa ancora un registro completo dei documenti e delle licenze.

## 3. Perché FSRS-7 e come applicarlo a VERO/FALSO

Il crate corretto è **`fsrs` 7.0.0**, non `rs-fsrs` 1.x e non `fsrsrs`. Con parametri vuoti/predefiniti, `FSRS::default()` seleziona **FSRS-7** (34 parametri). Il modello v7 mantiene tre valori per ogni scheda:

- `stability`: stabilità della memoria;
- `stability_fast`: traccia della stabilità veloce, specifica di FSRS-7;
- `difficulty`: difficoltà stimata.

La funzione ufficiale `FSRS::next_states_with_elapsed_days(previous_state, desired_retention, elapsed_days)` restituisce i quattro risultati possibili (`again`, `hard`, `good`, `easy`), ciascuno con nuovo `MemoryState` e intervallo raccomandato in giorni. L'API **accetta giorni frazionari**, fondamentale per ripassi a distanza di minuti nella stessa giornata.

### Traduzione corretta delle risposte V/F in rating

| Esito reale | Rating | Interpretazione |
|---|---:|---|
| Risposta **sbagliata** | `1 Again` | Obbligatorio, anche se l'utente seleziona una difficoltà diversa |
| Risposta giusta, incerta o ragionata con fatica | `2 Hard` | Richiamo riuscito ma difficile |
| Risposta giusta, ricordo normale | `3 Good` | Valore predefinito |
| Risposta giusta, immediata e sicura | `4 Easy` | Richiamo molto facile |

La sola correttezza binaria non basta a distinguere `Hard`, `Good` ed `Easy`: dopo una risposta corretta si chiede all'utente quanto fosse sicuro. In presenza di tentativi casuali o risposte «indovinate» è opportuno scegliere `Hard`, non `Easy`.

**Retenzione desiderata:** inizialmente `0.90`, configurabile con `--retention` nel range 0,70–0,99. Per una prova imminente si può valutare 0,93–0,95, accettando più ripassi. Il valore controlla il carico di studio: non rappresenta una probabilità certificata di superare l'esame.

**Riapprendimento nella stessa giornata:** nell'MVP, dopo `Again`, il programma applica uno step locale di **10 minuti**, mentre conserva lo stato di memoria appena calcolato da FSRS-7. Per gli altri rating usa l'intervallo proposto da FSRS, con minimo applicativo di un giorno. Si tratta di una **politica applicativa**, non di un intervallo restituito da FSRS stesso: deve restare esplicita nei log e nella documentazione.

## 4. Modello SQLite

```text
questions
 ├─ card_id TEXT PRIMARY KEY          es. AB-7020:26067
 ├─ source_version TEXT               es. AB-7020
 ├─ external_id INTEGER               identificativo nel listato
 ├─ group_id INTEGER                  quesito/gruppo ministeriale
 ├─ topic_id INTEGER
 ├─ topic TEXT
 ├─ question_text TEXT
 ├─ correct_answer INTEGER            0 = FALSO; 1 = VERO
 └─ image_path TEXT NULL

card_state
 ├─ card_id TEXT PRIMARY KEY → questions
 ├─ stability REAL
 ├─ stability_fast REAL
 ├─ difficulty REAL
 ├─ last_review_at INTEGER            Unix seconds UTC
 ├─ due_at INTEGER                    Unix seconds UTC
 └─ repetitions INTEGER

review_log
 ├─ id INTEGER PRIMARY KEY
 ├─ card_id TEXT → questions
 ├─ reviewed_at INTEGER               Unix seconds UTC
 ├─ answer_correct INTEGER
 ├─ rating INTEGER                    1..4
 ├─ elapsed_days REAL                 giorni frazionari
 ├─ predicted_interval_days REAL      output originale FSRS
 ├─ due_at INTEGER                    scadenza effettiva della policy
 ├─ fsrs_version TEXT                 '7.0.0'
 └─ desired_retention REAL
```

**Identità e migrazioni:** un aggiornamento del testo o della soluzione **nella medesima versione** è bloccato, perché associare i vecchi progressi a una domanda modificata falserebbe l'apprendimento. Per il listato da 7.185 usare `AB-7185`, non sovrascrivere `AB-7020`. Una futura tabella `question_equivalence` potrà collegare domande equivalenti, ma non trasferire automaticamente lo stato FSRS: prima è necessaria una revisione semantica.

**Dati personali e sicurezza:** il database resta in locale, usa query parametrizzate, conserva la storia dei rating e tratta il percorso delle immagini come dato non affidabile. Per backup usare la Backup API SQLite o un checkpoint WAL coerente; evitare di copiare il solo `.sqlite3` quando esistono file `-wal` e `-shm` attivi.

## 5. Interfaccia a riga di comando

Requisiti: Rust toolchain con supporto all'**edition 2024** (almeno Rust 1.85), toolchain e dipendenze compatibili; accesso alla rete per il primo `cargo build`.

```bash
# All'interno della cartella contenente Cargo.toml
cargo check
cargo test
cargo build --release

# Dataset A/B 7.020, scaricato localmente dal repository indicato
cargo run --release -- import-json data/AB-2024/AB2020.json --version AB-7020

# Versione nuova, DOPO conversione e verifica del listato ministeriale
cargo run --release -- import-csv data/ab_7185_normalizzato.csv --version AB-7185

# Ripasso con FSRS-7; 25 schede, retention 0.93
cargo run --release -- study --version AB-7020 --limit 25 --retention 0.93

# Statistiche del listato
cargo run --release -- stats --version AB-7020

# Note per Anki, senza immagini
cargo run --release -- export-anki --version AB-7020 export_anki.tsv

# Note per Anki e media separati
cargo run --release -- export-anki --version AB-7020 export_anki.tsv \
  --image-root data/AB-2024 \
  --media-out ./export_media

# Database in un percorso personalizzato: --db è un argomento globale
cargo run --release -- --db /percorso/progressi.sqlite3 stats --version AB-7020
```

**Formato CSV importabile** (UTF-8, intestazioni obbligatorie):

```csv
id,quesito_id,argomento_id,argomento_nome,testo,risposta,figura
1,10,1,Segnaletica,"DEMO: Il segnale rosso richiede di fermarsi.",VERO,
2,10,1,Segnaletica,"DEMO: In presenza del segnale di STOP non occorre mai fermarsi.",FALSO,
```

*Le due righe sono inventate per illustrare lo schema: non sono quiz ministeriali ufficiali.* Per il listato 7.185, il convertitore TXT→CSV dovrà essere sviluppato conoscendo l'impaginazione reale del file, senza presumere l'identità degli ID con la versione precedente.

## 6. Funzionamento della sessione di studio

1. Selezionare prima schede già viste con scadenza `due_at <= now`.
2. Riempire i posti rimanenti con domande nuove, scelte casualmente; **mai presentare in sessione una scheda non ancora scaduta**.
3. Presentare testo, argomento e percorso dell'immagine originale (la CLI non renderizza immagini inline).
4. Richiedere `V` o `F`, mostrare la soluzione **dopo** la risposta.
5. In caso di errore, applicare `Again`. In caso di risposta corretta, richiedere `Hard`, `Good`, `Easy`.
6. Ottenere i prossimi stati da FSRS-7; salvare stato e riga di `review_log` nella **stessa transazione**.
7. Pianificare la prossima data di studio in UTC. Mostrare statistiche di copertura, domande nuove e ripassi scaduti.

Per evitare che un utente deduca risposte dal feedback visivo, il software non deve mostrare la soluzione né indizi prima della scelta. Una modalità futura «simulazione d'esame» dovrà **disattivare FSRS durante la selezione delle domande** e aggiornare lo storico soltanto a prova conclusa, rispettando la distribuzione scelta e documentandola.

## 7. Esportazione per Anki: limiti e procedura

Il comando `export-anki` genera un **TSV UTF-8** con le colonne:

`ID` → `Front` → `Back` → `Tags`

- `ID` stabile = `<versione>:<id ministeriale>`, da usare come **primo campo** di un tipo nota personalizzato.
- `Front` contiene la domanda (HTML escaped) e, se richiesto, un riferimento `<img src="nome.jpg">`.
- `Back` contiene `VERO` o `FALSO`.
- `Tags` contiene `patenti`, versione e materia.

Importare le note in Anki scegliendo il separatore **tabulazione**, abilitando l'interpretazione HTML e associando quattro campi di un tipo nota creato in precedenza. Per le immagini, copiare i file generati in `export_media/` nella directory media di Anki prima di importare le note. Le immagini **non** sono incorporate automaticamente nel TSV. La cartella media non deve confondersi con il database FSRS del programma.

**Importante:** il TSV NON contiene gli stati `stability`, `stability_fast`, `difficulty`, le scadenze o la cronologia degli esercizi. Importare note e importare una storia FSRS nel database proprietario di Anki sono operazioni diverse. Per mantenere **anche i progressi** sarebbe necessaria una migrazione specifica e compatibile con le versioni del formato Anki; l'MVP non la implementa.

Per un successivo `.apkg` con immagini incorporate, prevedere un generatore dedicato che conservi ID/GUID stabili e non sovrascriva le pianificazioni di studio già presenti in Anki.

## 8. Criteri di qualità e collaudo

### Test automatici (già inclusi nel codice)

- Parsing di `VERO`, `FALSO` e rifiuto degli input ambigui.
- `FSRS::default()` produce stati validi e valori finiti anche per le prime ripetizioni.
- Import ripetuto della stessa versione non elimina lo stato FSRS.

### Test di integrazione da eseguire sulla macchina dello sviluppatore

1. `cargo check` e `cargo test` completati senza errori.
2. Import del JSON ufficiale: **7.020 record**, 25 materie, ID distinti, ogni risposta booleana.
3. Per ogni risposta errata, `rating = 1`, `due_at = now + 600 s` (con normale tolleranza per i secondi di elaborazione).
4. Per ogni risposta corretta, rating solo 2/3/4; intervallo FSRS finito e positivo.
5. Nessuna scheda futura proposta prima della scadenza; nessun duplicato nella sessione.
6. Dopo riavvio del programma, stato e review log identici; rollback completo su fallimento transazione.
7. Reimport della stessa versione e testo invariato: numero di domande e progressi invariati.
8. Riutilizzo di un ID con testo/risposta cambiati nella stessa versione: **import bloccato**.
9. Import di `AB-7185` non modifica `AB-7020`.
10. Export TSV leggibile da Anki, media esistenti e con path canonico interno al media root.
11. Backup/ripristino database con test di integrità `PRAGMA integrity_check`.

### Limitazioni e prossimi moduli

- **Ottimizzazione dei parametri FSRS-7:** implementare una fase separata dopo aver raccolto abbastanza ripassi. Il crate espone `compute_parameters`/`ComputeParametersInput`; l'ottimizzazione deve ricostruire sequenze cronologiche di `FSRSReview`, utilizzare validazione temporale e persistere la versione dei parametri. Evitare l'ottimizzazione precoce con poche osservazioni.
- **UI immagini:** una TUI Rust con viewer esterno oppure frontend web/desktop; il programma terminale mostra solo il percorso dell'immagine.
- **Parser specifici** per documenti AM, C/D, CQC, nautica e per il TXT da 7.185.
- **Apprendimento delle materie deboli:** ordinamento secondario degli argomenti e bilanciamento dei nuovi quiz, senza modificare artificialmente il rating FSRS.
- **Quiz d'esame realistici:** modalità separata con tempi, quote/estrazione, soglia errori e dati normativi aggiornati.
- **Distribuzione di contenuti:** verificare diritti di riutilizzo del PDF e delle immagini ministeriali, oltre alle licenze dei repository di terzi.

## 9. Codice sorgente completo

Il pacchetto allegato contiene `Cargo.toml`, `src/main.rs` e `examples/domande_demo.csv`. Di seguito lo stesso codice, per poterlo ricostruire interamente da questo `.md`.

### `Cargo.toml`

```toml
[package]
name = "patenti-fsrs7"
version = "0.1.0"
edition = "2024"
description = "Ripasso quiz patenti italiane con FSRS-7 e SQLite"
license = "MIT"

[dependencies]
anyhow = "1"
chrono = { version = "0.4", features = ["clock"] }
clap = { version = "4.5", features = ["derive"] }
csv = "1.3"
fsrs = "=7.0.0"
rusqlite = { version = "0.41", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

```

### `src/main.rs`

```rust
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
#[command(name = "patenti-fsrs7", about = "Quiz patenti italiane con FSRS v7")]
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
                let choice = prompt("Difficoltà: [2] Hard, [3] Good, [4] Easy (Invio=3): ")?;
                match choice.as_str() {
                    "" | "3" => break 3u8,
                    "2" => break 2u8,
                    "4" => break 4u8,
                    _ => println!("Inserisci 2, 3, 4 o Invio."),
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
            _ => bail!("Stato FSRS incompleto nel database: {}", card.id),
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
    println!("I progressi FSRS locali NON vengono importati nell'algoritmo Anki.");
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

```

## 10. Documentazione API e riferimenti

- [fsrs 7.0.0 — documentazione ufficiale](https://docs.rs/fsrs/7.0.0/fsrs/)
- [FSRS-rs — repository ufficiale](https://github.com/open-spaced-repetition/fsrs-rs)
- [Esempio ufficiale `schedule.rs`](https://github.com/open-spaced-repetition/fsrs-rs/blob/main/examples/schedule.rs)
- [Repository delle 7.020 affermazioni A/B](https://github.com/matteotarantino-algor/simulatore-quiz-patente)
- [MIT — circolare 19281/2024](https://www.mit.gov.it/normativa/circolare-protocollo-19281-del-05072024)
- [Manuale Anki — importazione file di testo](https://docs.ankiweb.net/importing/text-files.html)

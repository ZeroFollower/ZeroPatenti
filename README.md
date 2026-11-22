# ZeroPatenti

**ZeroPatenti** è un progetto open-source in preparazione per studiare i quiz teorici delle patenti italiane, funzionante offline e pensato per **desktop e Android**.

## Stato del progetto

**v0.1 / base tecnica** — il repository contiene un **motore CLI Rust** per importazione di domande, studio con ripasso dilazionato, salvataggio SQLite e export Anki. La **GUI Slint desktop/Android è pianificata, ma non ancora implementata**. Il mockup non è uno screenshot del software funzionante.

## Scelte tecnologiche

- **Rust 2024**: applicazione e motore condiviso
- **`fsrs` 7.0.0**: scheduling dei ripassi, invisibile nell'interfaccia utente
- **SQLite (`rusqlite`)**: domande, stati e storico dei ripassi
- **Slint**: toolkit UI previsto per Windows, Linux e Android
- **CSV/JSON + TSV**: import banca dati ed export Anki

## Esperienza di studio decisa

1. Leggi una domanda e rispondi **VERO/FALSO**.
2. Visualizza immediatamente l'esito e la soluzione.
3. In caso di errore, ripasso automatico.
4. Se la risposta è corretta, valuta **Da ripassare / Difficile / Normale / Facile**.
5. Il sistema pianifica la domanda successiva in modo personalizzato.
6. **Simulazione esame** indipendente: niente risposte o punteggi mostrati durante la prova e nessun aggiornamento dei ripassi nel corso dell'esame.

**Nella GUI non devono comparire sigle o nomi degli algoritmi di ripetizione.**

## Selezione delle banche dati per patente

Ogni categoria usa esclusivamente la banca dati corretta: `AM` ha un set
separato, `A1/A2/A/B1/B/BE` condividono il listato `A/B`, e i listati
di **revisione** non vengono mai mescolati al conseguimento.

Il catalogo versionato si trova in [`data/listati.json`](data/listati.json).
Le patenti superiori e le CQC rimangono non selezionabili finché non saranno
validati i filtri dei sottoinsiemi per categoria.

```bash
cargo run -- catalog

# Seleziona automaticamente AB-7020 per la patente B
cargo run -- import-for-license data/questions.json --license B
cargo run -- study-for-license --license B --limit 30
cargo run -- stats-for-license --license B

# Revisione e conseguimento sono due archivi separati
cargo run -- study-for-license --license AM --procedure revisione
```

L'importazione automatica da PDF ministeriale e l'aggiornamento remoto dei
listati **non sono ancora implementati**: i comandi accettano JSON/CSV
normalizzati da verificare rispetto alla fonte ufficiale, inclusi immagini e
soluzioni. Nessuna banca dati completa è attualmente inclusa nel repository.

## Versioni aggiornate e controlli

Usiamo versioni **stabili** e recenti, con proposte quotidiane di aggiornamento
Cargo e aggiornamenti settimanali delle GitHub Actions mediante Dependabot.
Ogni aggiornamento deve superare i controlli CI prima di essere integrato.
Il catalogo ministeriale segue un ciclo di aggiornamento indipendente:
pubblicazione di un nuovo PDF non implica automaticamente sua entrata in uso.

Vedi [Politica aggiornamenti](docs/POLITICA_AGGIORNAMENTI.md).

## Requisiti

- Rust stable aggiornato, toolchain Cargo con edizione 2024.
- Attualmente è disponibile la CLI; la GUI Slint e il pacchetto Android saranno aggiunti in una fase successiva.

## Prova locale (CLI)

```bash
cargo check
cargo test

# Import del JSON di simulatore-quiz-patente
cargo run -- import-json /percorso/questions.json --version AB-7020

# Oppure import da CSV, con esempio incluso
cargo run -- import-csv examples/domande_demo.csv --version DEMO

# Sessione di studio
cargo run -- study --version DEMO --limit 10 --retention 0.90

# Statistiche
cargo run -- stats --version DEMO

# Esportazione schede Anki
cargo run -- export-anki --version DEMO patenti.tsv
```

Il JSON A/B contiene 7.020 affermazioni con campi `id`, `quesito_id`, `argomento_id`, `argomento_nome`, `testo`, `risposta` e `figura` (opzionale): <https://github.com/matteotarantino-algor/simulatore-quiz-patente>. Il database completo **non è incluso** in questo repository: i listati ufficiali e i relativi diritti/condizioni d'uso devono essere verificati alla fonte.

## Struttura

```text
.
├── Cargo.toml
├── src/main.rs                 # CLI Rust + scheduler + SQLite
├── examples/domande_demo.csv   # Dataset dimostrativo, non ufficiale
├── docs/
│   ├── ARCHITETTURA_GUI_SLINT.md
│   ├── SPECIFICA_MOTORE_RUST.md
└── .github/workflows/ci.yml    # CI Rust
```

## Principi di sicurezza e qualità

- Le risposte ministeriali sono autoritative rispetto alle spiegazioni generate.
- Le diverse versioni dei listati non devono condividere automaticamente le cronologie.
- Ripassi e simulazioni d'esame restano modalità logicamente separate.
- Storage locale, nessuna telemetria né account obbligatorio.
- Gli ID restano stabili per esportazioni Anki ripetibili; i progressi Anki e quelli locali non vengono sincronizzati automaticamente.
- Non caricare in GitHub dati personali, database di studio, segreti o APK firmati.

## Documentazione

- [Architettura GUI Slint](docs/ARCHITETTURA_GUI_SLINT.md)
- [Specifica motore e API](docs/SPECIFICA_MOTORE_RUST.md)

## Licenze

Il repository non contiene attualmente una licenza per il codice: sceglierla **prima di qualsiasi distribuzione pubblica**. Per l'integrazione di Slint verificare le condizioni della licenza applicabile; per i quiz ministeriali, le immagini e gli archivi di terzi verificare separatamente le condizioni di riutilizzo.

> Stato verifiche: codice analizzato, ma compilazione/test Rust non eseguiti in questo ambiente (toolchain Rust non disponibile).
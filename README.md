# ZeroPatenti

**ZeroPatenti** è un progetto open-source in preparazione per studiare i quiz teorici delle patenti italiane, funzionante offline e pensato per **desktop e Android**.

## Stato del progetto

La CLI Rust e la GUI Slint per desktop e Android condividono la banca dati SQLite. Dalla GUI puoi studiare, ripassare, svolgere una prova e consultare la correzione e le statistiche. Nelle prove personalizzate puoi scegliere gli argomenti, il numero di domande, il criterio di estrazione e il timer.

## Scelte tecnologiche

- **Rust 2024**: applicazione e motore condiviso
- **`fsrs` 7.0.0**: scheduling dei ripassi, invisibile nell'interfaccia utente
- **SQLite (`rusqlite`)**: domande, stati e storico dei ripassi
- **Slint**: interfaccia per Windows, Linux e Android
- **CSV/JSON + TSV**: import banca dati ed export Anki

## Esperienza di studio decisa

Al primo avvio puoi inserire il giorno dell’esame oppure scegliere **Data ancora non definita**. Entrambe le scelte vengono ricordate. In **Impostazioni → Data dell’esame** puoi aggiungere o cambiare la data per la patente e la procedura selezionate. Con una data, la dashboard mostra il conto alla rovescia e quante domande nuove restano da affrontare oggi. **Inizia a studiare** dà la precedenza ai ripassi scaduti.

Con **Studio continuo** puoi proseguire anche dopo l’obiettivo del giorno. Per limitarti ad alcuni argomenti, scegli **Senza limite** in **Studio e ripasso**. A ogni avanzamento, Studio propone la carta con la scadenza più vecchia tra i ripassi già dovuti. Quando li hai completati, passa ai quesiti nuovi. Ogni valutazione aggiorna con FSRS la scadenza della carta. Ripasso propone solo le carte già scadute. Nella CLI la stessa opzione è `--unlimited`, per esempio `zeropatenti study-for-license --license B --unlimited`.

In **Studio e ripasso** puoi cercare e selezionare gli argomenti e ingrandire le figure. I listati JSON e CSV possono includere i campi editoriali facoltativi `spiegazione`, `approfondimento` e `fonte_spiegazione`.

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
Per il conseguimento A/B, ZeroPatenti confronta gli anni nei nomi `AB_<anno>.sqlite3` presenti nella cartella `DB`, nel [catalogo su GitHub](https://github.com/lucatortiglione/ZeroPatenti/tree/main/DB) e fra le edizioni già importate. Per ogni nuova simulazione seleziona l’anno più recente, scarica il dataset se necessario e verifica che corrisponda al file pubblicato. Senza connessione usa l’edizione disponibile in locale e segnala la modalità offline. Le edizioni precedenti restano selezionabili con l’avviso **obsoleto**. Per pubblicare un aggiornamento aggiungi un dataset con una sola edizione alla cartella `DB` su `main`, ad esempio `DB/AB_2027.sqlite3`. GitHub Actions aggiorna automaticamente gli stati nell’indice.

In **Banche dati** l’app rileva i file SQLite presenti in `DB` accanto al profilo. Seleziona **Importa** accanto al nome per importare le domande. Sono supportati i dataset ZeroPatenti con una sola edizione nella tabella `questions`. Le immagini possono essere incorporate oppure trovarsi in `DB/<nome del dataset>/figures/`. I progressi di altri utenti non vengono importati.

Il profilo personale si chiama `progressi.sqlite3`. Le installazioni con un precedente `patenti.sqlite3` possono continuare a usarlo. L’opzione desktop `--db` sceglie un profilo specifico. Su Android il profilo resta nella cartella privata dell’app. Nel database attivo, `card_state` contiene lo stato di ogni domanda valutata e `review_log` lo storico dei ripassi.
Le patenti superiori e le CQC rimangono non selezionabili finché non saranno
validati i filtri dei sottoinsiemi per categoria.

```bash
cargo run -- catalog

# Seleziona e prepara automaticamente il dataset A/B più recente per la patente B
cargo run -- study-for-license --license B --limit 30
cargo run -- stats-for-license --license B

# Revisione e conseguimento sono due archivi separati
cargo run -- study-for-license --license AM --procedure revisione
```

Per preparare una banca per le simulazioni, scarica il PDF italiano dal campo
`fonte` del listato scelto nel catalogo e usa una cartella distinta per ogni edizione.
Il normalizzatore controlla il digest del PDF ed estrae domande, soluzioni e figure.
Il motore ricontrolla il contenuto importato prima di iniziare una prova.

```bash
python -m pip install PyMuPDF
python scripts/normalize_bank.py listato.pdf banca
cargo run --features gui --bin zeropatenti-gui
```

Apri **Banche dati**, seleziona il listato e premi **Importa listato e immagini**. Importa `banca/questions.json` e usa `banca` come cartella delle immagini. Mantienila disponibile per vedere le figure offline. Puoi scegliere un database desktop con `--db percorso.sqlite3`; il predefinito è lo stesso della CLI.

In **Banche dati → Chiave pubblica dell’editore**, salva la chiave Ed25519 ricevuta dall’editore tramite un canale attendibile. Quando trova `manifest.json` o `manifest.sig`, la GUI richiede entrambi e controlla firma, edizione e SHA-256 dei quesiti e di tutte le figure referenziate. **Verifica listato** ripete il controllo sui file locali. Durante lo studio e gli esami l’app ricontrolla ogni figura firmata prima di mostrarla. Per i listati precedenti la GUI verifica i quesiti rispetto al catalogo e richiede **Conferma importazione senza firma**. La pulizia mostra in anteprima le edizioni fuori catalogo e conserva quelle con progressi o prove.

Per gli editori, lo script seguente richiede Python 3.11+ e OpenSSL 3. La chiave privata Ed25519 va custodita fuori dalla cartella distribuita. Lo script produce il manifest e la firma binaria di 64 byte, e stampa la chiave pubblica da comunicare separatamente. Una distribuzione dell’app può fissare la chiave pubblica impostando `ZEROPATENTI_BANK_PUBLIC_KEY` (64 cifre esadecimali) durante la compilazione.

```bash
python scripts/sign_bank.py banca --version AB-7020 --private-key /percorso/privato/editore.pem
```

Su Android usa **Scegli la cartella del listato** per selezionare `AB-2024` con `AB2020.json` e `figures/`, poi premi **Importa**. Sono supportati anche i pacchetti con `questions.json` o `questions.csv`. L'app conserva una copia privata. Per generare l'APK di sviluppo usa JDK 21, SDK Android 35 e NDK r30. Imposta `JAVA_HOME`, `ANDROID_HOME` e `ANDROID_NDK_ROOT` sulle rispettive cartelle:

```bash
rustup target add aarch64-linux-android
cargo install cargo-apk2 --version 1.5.0 --locked
cargo apk2 build --lib --features gui --target aarch64-linux-android
```

Con **Interrompi e salva** puoi riprendere dallo stesso quesito. Le omesse contano come errori per l'esito. Alla consegna di una personalizzata, ogni risposta data aggiorna una sola volta il ripasso. Riapri i risultati da **Statistiche e risultati**, dove le prove ufficiali e ciascuna strategia personalizzata hanno serie distinte.

Per AM e A/B conseguimento la simulazione ufficiale usa le quote per argomento dell'edizione supportata. Per la revisione AM e A/B è disponibile la simulazione personalizzata. La modalità ufficiale di revisione richiede una distribuzione per argomento verificata.

## Versioni aggiornate e controlli

Usiamo versioni **stabili** e recenti, con proposte quotidiane di aggiornamento
Cargo e aggiornamenti settimanali delle GitHub Actions mediante Dependabot.
Ogni aggiornamento deve superare i controlli CI prima di essere integrato.
Il catalogo ministeriale segue un ciclo di aggiornamento indipendente:
pubblicazione di un nuovo PDF non implica automaticamente sua entrata in uso.

Vedi [Politica aggiornamenti](docs/POLITICA_AGGIORNAMENTI.md).

## Requisiti

- Rust stable aggiornato, toolchain Cargo con edizione 2024.
- Per compilare la GUI abilita la feature Cargo `gui`. Per eseguire anche i test dell'interfaccia usa `cargo test --features gui`.

## Prova locale (CLI)

```bash
cargo check
cargo test

# Import del pacchetto locale A/B
cargo run -- import-json data/AB-2024/AB2020.json --version AB-7020

# Oppure import da CSV, con esempio incluso
cargo run -- import-csv examples/domande_demo.csv --version DEMO

# Sessione di studio
cargo run -- study --version DEMO --limit 10 --retention 0.90

# Statistiche
cargo run -- stats --version DEMO

# Esportazione schede Anki
cargo run -- export-anki --version DEMO patenti.tsv
```

Il JSON A/B contiene 7.020 affermazioni con campi `id`, `quesito_id`, `argomento_id`, `argomento_nome`, `testo`, `risposta` e `figura` (opzionale): <https://github.com/matteotarantino-algor/simulatore-quiz-patente>. I database SQLite pubblicati sono elencati nell’[indice delle edizioni A/B](DB/README.md).

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


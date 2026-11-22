# ZeroPatenti — politica aggiornamenti e compatibilità

Aggiornato: 22 novembre 2026.

## Regola
Usare la più recente versione **stabile e supportata** di Rust, FSRS, Slint, SQLite, dipendenze Rust, GitHub Actions e standard tecnici pertinenti. Le versioni beta, nightly e release candidate non vanno introdotte senza una valutazione esplicita.

## Automazione
- Dependabot controlla quotidianamente le dipendenze Cargo e settimanalmente GitHub Actions.
- Ogni aggiornamento apre una pull request. Nessun merge o rilascio automatico senza controlli.
- CI con compilazione e test su Linux e Windows. Android avrà controlli specifici quando la GUI sarà implementata.
- Verificare changelog, incompatibilità API, migrazioni SQLite, prestazioni e regressioni FSRS prima del merge.
- Variazioni dell'algoritmo FSRS richiedono test sulla conservazione dello storico di ripasso.
- Aggiornamenti di sicurezza critici hanno precedenza.

## Riproducibilità
- Le versioni nel Cargo.toml sono intervalli compatibili secondo SemVer.
- Per rilasci riproducibili, generare e versionare Cargo.lock; usare cargo build --locked.
- Non perdere mai i progressi SQLite per effetto di aggiornamenti software.

## Listati ufficiali: ciclo separato
- Controllare periodicamente Portale dell'Automobilista e MIT.
- La data di pubblicazione di un PDF non dimostra l'entrata in vigore per l'esame.
- Distinguere categoria, conseguimento/revisione, lingua, edizione e stato ufficiale.
- Conservare URL, SHA-256, data, conteggio, risposte, figure e fonte.
- Attivare nuove edizioni solo dopo verifica; non ereditare progressi di domande modificate.
- Non redistribuire quesiti o immagini senza licenza verificata.

## Riferimenti verificati
- Rust stable: 1.100.0 (12 novembre 2026), https://doc.rust-lang.org/releases.html
- FSRS Rust: 7.0.0, https://docs.rs/fsrs/latest/fsrs/
- Slint: 1.19.0, https://docs.rs/crate/slint/latest
- rusqlite: 0.43.0, https://docs.rs/crate/rusqlite/latest
- Catalogo MIT: https://www.ilportaledellautomobilista.it/web/portale-automobilista/-/quiz-per-le-patenti-am-b-superiori-e-cqc

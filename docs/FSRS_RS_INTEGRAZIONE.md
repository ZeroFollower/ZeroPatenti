# Integrazione FSRS ufficiale in ZeroPatenti

**Sorgente upstream:** https://github.com/open-spaced-repetition/fsrs-rs
**Crate Cargo:** `fsrs = "7.0.0"` (già presente in `Cargo.toml`).

La libreria `fsrs` è il motore ufficiale Rust per scheduling, ricostruzione della memoria e ottimizzazione dei parametri. ZeroPatenti NON incorpora un fork dell'algoritmo e non implementa una formula parallela per stabilità, difficoltà o retrievability.

## Pianificazione

La CLI usa `FSRS::default().next_states_with_elapsed_days(...)` e i valori `memory` e `interval` restituiti dall'upstream.

La conversione da `interval` alla data di scadenza replica la convenzione mostrata in `fsrs-rs/examples/schedule.rs`:

- `interval.round().max(1.0)` per il numero di giorni da aggiungere;
- stessa procedura per Again, Hard, Good, Easy;
- nessun ritardo artificiale di dieci minuti per Again.

L'algoritmo della memoria resta di proprietà del crate `fsrs`; il calcolo calendario (timestamp Unix) è un adattatore applicativo.

## Distinzione dalle funzionalità pianificate

La logica del calendario FSRS non implementa da sola:
- simulazioni ufficiali senza aggiornamento della memoria;
- snapshot di eligibility delle simulazioni personalizzate;
- replay cronologico verificato degli eventi;
- stati condivisi e provvisori nei rami offline;
- ottimizzazione dei parametri su cronologie validate.

Questi sono sottosistemi ancora previsti dal piano di implementazione e non vanno dati per esistenti.

## Criteri di verifica

- `cargo test` deve confermare che `Again` usa il suo `interval` restituito da FSRS;
- `cargo test` deve verificare l'arrotondamento per tutti i quattro rating;
- `cargo clippy --all-targets -- -D warnings` prima del merge;
- confronto periodico delle API dell'upstream in caso di aggiornamento del crate.

La versione `fsrs = "7.0.0"` è una dipendenza Cargo al crate ufficiale, non un URL Git. Questo è intenzionale: Cargo.lock dovrà essere mantenuto per build riproducibili.

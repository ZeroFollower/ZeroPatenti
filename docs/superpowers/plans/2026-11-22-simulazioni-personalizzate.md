# Piano di implementazione delle simulazioni personalizzate

> Per l'esecuzione usare Superpowers Subagent-Driven Development oppure Executing Plans. Spuntare ogni passaggio dopo averne verificato il risultato.

**Obiettivo** — Da desktop o Android un candidato sceglie argomenti e durata, svolge la prova offline, la riprende dopo una pausa e consulta correzione e statistiche. Ogni risposta data nella prova personalizzata alimenta il ripasso una volta sola.

**Architettura** — La CLI, la GUI Slint e l'entry point Android condividono una libreria Rust. SQLite conserva sia lo stato corrente sia gli eventi. Il controller grafico riceve una vista della prova priva delle soluzioni fino alla consegna. [Specifica approvata](../specs/2026-11-22-simulazioni-personalizzate-design.md)

## 1. Motore condiviso e provenienza dei dati

- [x] Verificata su Windows la CLI di partenza con `cargo test --locked` (12 test superati). Preparato un database temporaneo v0.1 tramite `import-csv` e `study` sul CSV demo. Dopo la riapertura, `stats` conta due domande, una studiata e un ripasso. Questi conteggi dovranno sopravvivere alla migrazione.
- [x] Estrarre da `src/main.rs` le operazioni indipendenti dal terminale in `src/lib.rs`, `src/storage.rs` e `src/review.rs`. La CLI mantiene comandi, opzioni e formati esistenti. Esporre `apply_review(tx, card_id, correct, rating, now, retention)` a studio e consegna.
- [x] In `src/exam/catalog.rs` risolvere patente e procedura usando il catalogo esistente. Associare ogni regola a fonte, versione e periodo di applicabilità. Verificare le fonti ministeriali prima di abilitare ogni combinazione. Associare la verifica della banca importata al digest dei contenuti, comprese soluzioni e riferimenti alle figure. Una successiva modifica invalida la verifica.
- [x] Provare con fixture locali un archivio migrato, un listato privo di attestazione, uno alterato dopo la verifica e una regola appartenente a una procedura diversa. Solo la combinazione verificata deve consentire l'avvio.

## 2. Configurazione e selezione

- [x] In `src/exam/selection.rs` definire `select(config, candidates, seed)` e testare prima dell'implementazione argomenti vuoti o sconosciuti, zero domande, disponibilità insufficiente, assenza di duplicati e ripetibilità a parità di input.
- [x] Ordinare la popolazione per ID stabile prima dell'estrazione. Per Casuale usare un campionamento senza reinserimento. Per Equilibrata alternare gli argomenti disponibili saltando quelli esauriti. Per Intelligente ordinare per ripassi scaduti e lacune osservate usando il seed per gli spareggi. Conservare seed, versione dell'algoritmo e ordine estratto.
- [x] Testare la soglia con aritmetica intera, compreso un numero di quesiti che ammette zero errori. Mostrare in configurazione la soglia effettiva e la durata ufficiale della procedura. Distinguere strutturalmente la configurazione ufficiale da quella personalizzata per impedire override.

## 3. Sessione persistente e orologio

- [x] Aggiungere in `src/exam/store.rs` la migrazione additiva per regole, sessioni, quesiti congelati, risposte, eventi, risultati e applicazioni ai ripassi. Imporre i vincoli univoci e gli indici della specifica. Provare la migrazione due volte sul database v0.1.
- [x] Implementare creazione, avvio, navigazione, modifica, cancellazione della risposta, pausa, ripresa e annullamento. Per ogni comando, validare lo stato e scrivere proiezione ed evento nella stessa transazione. Un doppio tocco sulla stessa risposta lascia invariato il conteggio dei cambi.
- [x] In `src/exam/clock.rs` usare tempo monotono e checkpoint mentre la prova è attiva. Chiudere i segmenti al cambio di domanda e all'uscita dal primo piano. Al riavvio ripristinare in pausa dall'ultimo checkpoint. Allo scadere congelare la prova anche se l'utente invia contemporaneamente una risposta.
- [x] Verificare con orologio controllabile e riapertura di un file SQLite il salto tra domande, una pausa lunga, il crash tra due checkpoint, un errore di scrittura a metà transazione e un comando tardivo su una sessione terminale.

## 4. Consegna e statistiche

- [x] In `src/exam/result.rs` implementare `submit(conn, session_id)`. Sotto un'unica transazione di scrittura calcolare esito e dettagli dagli snapshot, applicare i rating 3 e 1 delle risposte date, salvare le chiavi di idempotenza e rendere terminale la sessione. Una nuova consegna restituisce il risultato congelato. Le omesse pesano sull'esito. Una prova ufficiale o annullata non genera ripassi.
- [x] Provare doppia consegna da due connessioni, rollback durante un aggiornamento del ripasso e riapertura dopo la modifica del catalogo. Controllare conteggi, scadenze e numero esatto di righe nello storico.
- [x] In `src/exam/statistics.rs` calcolare dai segmenti osservati tempi per domanda, prima e ultima risposta, cambi utili o dannosi, ritorni e pause. Aggregare risultati per argomento, esito e periodo. Separare serie per tipo di prova e strategia, con filtri per patente, procedura e versione. Un segmento incompleto resta identificabile come tale.
- [x] Confrontare i risultati con fixture di eventi a tempi noti, includendo conteggi, medie, mediane, quesiti mai visti e sessioni annullate. Verificare copertura, ripassi scaduti, andamento e stime di memoria sullo storico corretto. Escludere previsioni di superamento prive di un modello validato.

## 5. Interfaccia e build

- [x] In `ui/app.slint` e `src/gui.rs` collegare selezione banca, configurazione, prova, ripresa, correzione e statistiche al motore condiviso. Rendere accessibili Studio e Ripasso coi rispettivi filtri. Mostrare domanda e figura locale, griglia navigabile, timer e azioni. Chiedere conferma per annullare. Tener separati gli stati della griglia dalla correttezza fino alla consegna.
- [x] Collegare la stessa UI all'eseguibile desktop e all'activity Android. Sul dispositivo usare la directory privata dell'app, prevedere l'import locale della banca e delle figure e mettere in pausa al passaggio in background. Verificare scorrimento e azioni primarie su schermo stretto.
- [x] Eseguire il percorso completo anche tramite UI con un database temporaneo. Verificare ripresa dopo riapertura, filtro errori, riepilogo per argomento e assenza di nomi tecnici dell'algoritmo nelle etichette.
- [x] Eseguire `cargo test`, `cargo test --features gui`, Clippy, build Windows e Linux e generazione APK con SDK/NDK. Avviare l'APK su emulatore o dispositivo e provare background/ripresa. Registrare separatamente verifiche eseguite ed eventuali limitazioni della piattaforma.

## Esito della verifica

La build Windows supera 47 test Rust con il dataset di migrazione abilitato. Su Linux è stata avviata la GUI con X11. La verifica su emulatore Android API 35 x86-64 ha coperto importazione dal selettore di cartelle, avvio di una prova da 10 domande, risposta, navigazione, pausa in background, ripresa e risultato con 2 risposte date e 8 omesse. Una prova ufficiale da 30 domande ha mostrato una figura locale, la seconda pagina della griglia e la conferma di annullamento. L’APK di sviluppo contiene anche il binario ARM64. Il test su dispositivo fisico rimane distinto dalla verifica su emulatore.

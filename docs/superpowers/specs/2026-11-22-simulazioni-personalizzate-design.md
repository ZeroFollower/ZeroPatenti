# ZeroPatenti — Simulazioni personalizzate, persistenza ibrida e statistiche

**Data:** 2026-11-22  
**Stato:** bozza progettuale in attesa di approvazione  
**Metodo:** Superpowers Brainstorming

## 1. Obiettivo
Realizzare simulazioni personalizzate basate esclusivamente sulla versione verificata del listato ministeriale pertinente alla patente scelta, con navigazione, timer, sospensione, annullamento e correzione finale; le risposte della prova personalizzata aggiornano automaticamente e una sola volta lo storico dei ripassi. Fornire statistiche accurate e trasparenti a partire dagli eventi effettivamente raccolti, senza confondere metriche osservate e stime.

## 2. Decisioni già approvate
- Modalità distinte: Studio, Ripasso, Simulazione ufficiale, Simulazione personalizzata.
- Simulazione personalizzata: scegli uno o più argomenti e un numero di domande; mai duplicati nello stesso tentativo.
- Tre criteri di estrazione: **Casuale**, **Equilibrata** (predefinita), **Intelligente** (priorità a lacune e ripassi). Conservare strategia, versione e seed della selezione per replicabilità.
- Timer: **durata ufficiale applicabile alla patente/prova** oppure **nessun limite**. Il timer si ferma quando la simulazione personalizzata viene messa in pausa, riparte dal residuo e viene salvato automaticamente.
- Soglia di superamento: `floor(n_domande_personalizzate * errori_ufficiali / n_domande_ufficiali)`. Omesse = errori per l'esito; le regole ufficiali sono versionate, non dedotte dal numero delle domande.
- Niente correzione durante la prova. Si può cambiare risposta sino alla consegna; correzione dettagliata soltanto dopo.
- **Alla consegna definitiva**: corrette -> rating 3 (Normale / Good); errate -> rating 1 (Da ripassare / Again); omesse -> nessun aggiornamento FSRS. Nessuna applicazione durante la prova. Nessun doppio aggiornamento se l'utente riapre il risultato.
- `Interrompi` salva e mette in pausa, `Riprendi` recupera tutti i dati, `Annulla` elimina logicamente la sessione incompleta senza aggiornare i ripassi; conferma prima dell'annullamento.
- Stack Rust, SQLite e Slint per Windows/Linux/Android; GUI senza riferimenti a FSRS.

## 3. Architetture esaminate
**A — Stato SQLite soltanto.** Più semplice, ma impossibile analizzare in modo affidabile cambi di risposta e storia delle interazioni non memorizzate.  
**B — Stato SQLite + registro append-only degli eventi (SCELTA).** Ripresa veloce, statistiche ricche e audit della consegna; transazioni coerenti.  
**C — Snapshot periodici JSON.** Semplice all'inizio, fragile e poco pratico per aggregazioni statistiche.

L'approccio B non richiede event sourcing integrale: le tabelle normalizzate conservano lo stato operativo corrente; la cronologia degli eventi è la fonte per statistiche cronologiche e diagnostica.

## 4. Schema dati proposto
- `exam_rules`: patente, procedura, fonte normativa, versione, n_domande, durata, errori_massimi, applicabilità.
- `exam_sessions`: `session_id` UUID, tipo (ufficiale/personalizzata), patente, procedimento, versione listato, configurazione *immutabile*, strategia, seed, stato, indice corrente, residuo del timer, tempo attivo, timestamps, `schema_version`.
- `exam_session_questions`: ordine fissato delle domande estratte e snapshot della versione del quesito, unico per sessione; nessuna sostituzione durante l'esame.
- `exam_answers`: risposta corrente per (session_id, question_id), timestamp ultima modifica, numero cambi, stato risposta.
- `exam_events`: `event_id`, `session_id`, `sequence`, `type`, `event_version`, `occurred_at`, `payload_json`, vincolo `UNIQUE(session_id, sequence)`; append-only.
- `exam_results`: correzione congelata, corrette, errate, omesse, punteggio, superamento, tempi e per-argomento; `UNIQUE(session_id)`.
- `review_applications`: idempotency key `(session_id, question_id)` univoca, `review_log_id` associato alla cronologia personale e risultato dell'applicazione.
- `statistics_cache` opzionale: proiezioni/materialized summaries rigenerabili dagli eventi e dai risultati, con versione calcolo.

Eventi minimi: `SessionCreated`, `SessionStarted`, `QuestionDisplayed`, `AnswerSelected`, `AnswerChanged`, `AnswerCleared`, `SessionPaused`, `SessionResumed`, `SessionTimedOut`, `SessionCancelled`, `SessionSubmitted`, `ReviewApplied`. Registrare solo interazioni rilevanti; evitare logging di ogni millisecondo o frame grafico. Ogni evento conserva timestamp UTC e un contatore monotono; la durata attiva è calcolata soltanto in foreground e durante l'esame attivo.

## 5. Transazioni, timer e recupero
Macchina a stati: `created -> active <-> paused -> submitted`; `created/active/paused -> cancelled`. `submitted/cancelled` sono terminali.
- Ogni modifica risposta aggiorna `exam_answers` e aggiunge l'evento nella **stessa transazione SQLite**.
- Timer in esecuzione basato su tempo monotono: si persistono checkpoint di tempo attivo/residuo e stato. In pausa, background Android o ripristino dopo crash, si sospende il timer. Si evita di conteggiare il tempo fuori dall'app come tempo di prova.
- La consegna congela risposte e risultati; invia **una sola volta** le risposte non omesse alla pianificazione, registrando applicazione e aggiornamento del relativo `review_log` in transazione, oppure con procedura ripristinabile e chiavi di idempotenza.
- Non applicare FSRS alle prove **ufficiali** se non deciso separatamente; l'aggiornamento automatico è confermato per la **personalizzata**.
- L'annullamento non distrugge necessariamente i record diagnostici: sessione marcata `cancelled`, risultati e memoria non alterati. La gestione della cancellazione dei dati personali resta disponibile.
- Conservare riferimenti ai quesiti e alla versione del listato originale, indipendentemente dagli aggiornamenti successivi.

## 6. Statistiche: catalogo e condizioni di affidabilità
**Risultati oggettivi per prova**
- Numero di corrette, errate, omesse; percentuale corrette; superata/non superata; margine dalla soglia; durata attiva; tempo residuo; tempo medio/mediano per quesito, per argomento e per esito (quando sono registrati gli eventi di visualizzazione).
- Distribuzione per argomento e accuratezza per argomento; numero domande mai viste in precedenza; tempo tra prima visualizzazione e prima risposta; domande saltate/ritrovate.

**Comportamento**
- Prima risposta vs risposta finale; quanti cambi sono stati benefici o dannosi; ripensamenti; conteggi per categoria; tempo trascorso sui quesiti; sequenze di risposta e pause, evitando di attribuire con certezza motivazioni psicologiche ai tempi.
- Analisi storica: tendenza errori per sessione e argomento, trend dei tempi, numero e durata delle pause, frequenza prove concluse/annullate.

**Preparazione e memoria**
- Copertura della banca dati, quota vista/studiata, ripassi scaduti e completati, tasso di risposta corretta nelle ripetizioni.
- Stime di recupero/padronanza derivate da FSRS-7 e profilo di scadenze, presentate come **stime**, mai come probabilità certe di superare l'esame.
- Stima probabilità di superamento solo se definito un modello validato su esami realistici e con sufficiente storico; riportare versione, intervallo d'incertezza e dimensione campione. La selezione intelligente distorce i risultati: per giudicare la preparazione usare principalmente le prove realistiche e i dati per argomento.

**Qualità e analisi**
- Separare serie ufficiali, personalizzate casuali, equilibrate e intelligenti; mai confrontare le percentuali come fossero campioni identici.
- Disponibilità di filtri temporali e per patente, procedura, listato, argomento, strategia.
- Non raccogliere telemetria esterna né dati di identificazione; statistiche in locale, esportazione e cancellazione opzionali.
- Gestire esplicitamente eventi mancanti, crash e intervalli non osservabili: non inventare tempi o dettagli non registrati.

## 7. GUI Slint proposta
- Configurazione: patente e listato in uso, checkbox argomenti, n_domande, strategia di estrazione, timer ufficiale/off, riepilogo soglia prevista, pulsante Avvia.
- Prova: domanda e immagine, VERO/FALSO (o tipo risposta secondo patente), navigazione precedente/successiva, griglia numerata con stati, timer, Interrompi, Annulla, Consegna.
- Ripresa: card della sessione sospesa con tempo residuo e progresso; Riprendi oppure Annulla.
- Risultati: promosso/bocciato *nella prova personalizzata*, corrette/errate/omesse, correzione per domanda, filtro errori, riepilogo per argomento e analisi ripensamenti.
- Statistiche: dashboard leggera per progressi, trend, argomenti deboli, tempi e scadenze; dettagli avanzati on demand.
- Su Android azioni principali sempre raggiungibili, layout touch e ripresa dopo background.
- Non mostrare FSRS o nomi tecnici di algoritmi nella UI.

## 8. Test e accettazione proposti
- La stessa sessione non contiene duplicati; 3 strategie coerenti con i filtri e la quantità disponibile.
- Configurazione e regole ufficiali con versioni tracciabili; la simulazione ufficiale non accetta personalizzazioni.
- Formula `floor` corretta ai confini, incluso n_piccolo; omesse contano come errori ma non come review.
- Interrompi/riprendi non consuma timer; crash e ripresa non perdono risposte già confermate.
- Lo stesso tentativo consegnato due volte produce **un solo** risultato e un solo aggiornamento FSRS per risposta non omessa.
- Eventi + stato corrente sempre coerenti anche con errori e interruzioni durante una transazione.
- I dati di una patente/versione non si mescolano a un'altra; statistiche e filtri rispettano argomenti e tipi di prova.
- Test Rust + SQLite + UI Slint, con build desktop e Android, prima di dichiarare la funzionalità completa.

## 9. Questioni per approvazione
Confermare la raccolta degli eventi **QuestionDisplayed/AnswerChanged** per tempi e ripensamenti. Per correttezza del timer su Android, un passaggio al background viene trattato come sospensione automatica nella modalità personalizzata; il timer della prova ufficiale resta soggetto alle regole specifiche dell'esame.

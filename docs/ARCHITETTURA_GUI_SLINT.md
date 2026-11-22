# ZeroPatenti — Interfaccia Slint per desktop e Android

**Stato:** specifica approvata; GUI non ancora implementata nel codice sorgente incluso.

## Stack

- Rust per logica, FSRS-7 e persistenza SQLite
- Slint per GUI reattiva (desktop e Android)
- Immagini segnali stradali memorizzate offline
- Compilazione desktop nativa; configurazione Android separata con SDK/NDK

## Vincoli UX vincolanti

1. **Nessuna menzione di FSRS, retention, difficoltà numerica, stability o algoritmo nell'interfaccia grafica.** Questi termini possono apparire solo in log tecnici, documentazione e codice interno.
2. Schermate principali: **Studio, Ripasso, Simulazione esame, Statistiche, Banche dati, Impostazioni**.
3. Nella modalità Studio/Ripasso mostra domanda e figura, poi i due grandi pulsanti **Vero / Falso**.
4. Dopo la risposta, mostra immediatamente esito e soluzione ufficiale.
5. Risposta sbagliata: programma automaticamente **Da ripassare**, senza chiedere valutazioni aggiuntive.
6. Risposta corretta: chiedi **Quanto è stato difficile ricordarla?** con quattro opzioni: **Da ripassare**, **Difficile**, **Normale**, **Facile**. Anche una risposta giusta può essere considerata da ripassare se ottenuta per intuito.
7. **Simulazione esame**: niente feedback durante la prova, niente valutazioni della difficoltà, nessun aggiornamento al ripasso personale salvo scelta esplicita successiva.
8. Desktop: sidebar e pannelli; Android: navigazione compatta, elementi touch grandi, scroll accessibile, immagini ingrandibili.
9. Modalità offline di default, nessuna richiesta di account, database locale per dispositivo; sincronizzazione opzionale solo in una fase separata.
10. Versione del listato obbligatoria in import: quesiti mutati di testo/soluzione non devono ereditare automaticamente la programmazione delle vecchie schede.

## Mappatura valutazioni (interna, mai visibile in UI)

| Etichetta utente | Rating FSRS-7 |
|---|---:|
| Da ripassare | 1 — Again |
| Difficile | 2 — Hard |
| Normale | 3 — Good |
| Facile | 4 — Easy |

Per una risposta errata usare 1 automaticamente. Per una risposta corretta consentire tutte e quattro le valutazioni. Per ripassi ravvicinati il ritardo applicativo configurato (es. dieci minuti) è separato dal calendario a giorni calcolato da FSRS.

## Percorso di implementazione previsto

- [ ] Estrarre le funzionalità da `src/main.rs` in un crate `zeropatenti-core`, con API indipendenti dalla CLI.
- [ ] Creare crate `zeropatenti-ui` con componenti `.slint` riutilizzabili e layout adattivi.
- [ ] Aggiungere `zeropatenti-desktop` e bootstrap Android (cargo-apk2 / android-activity).
- [ ] Integrare immagini locali, caching, accessibilità, navigazione tastiera e touch.
- [ ] Testare Android emulator e dispositivi fisici; verificare la licenza Slint scelta.

## Riferimenti tecnici

- https://slint.dev/docs/
- https://github.com/slint-ui/slint
- https://github.com/open-spaced-repetition/fsrs-rs

Il mockup illustrativo discusso in fase di progettazione non rappresenta un'app già funzionante.

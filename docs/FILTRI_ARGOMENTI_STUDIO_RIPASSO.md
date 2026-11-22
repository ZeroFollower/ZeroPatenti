# ZeroPatenti — Filtri per argomento in Studio e Ripasso

## Obiettivo
Dopo la scelta della **patente** (e della procedura conseguimento/revisione),
l'utente può selezionare **Tutti gli argomenti**, un singolo argomento oppure
più argomenti della banca dati ufficiale attiva.

Il termine "categoria" nella UI di questa funzione indica **argomento/materia
dei quesiti**, distinto dalla **categoria di patente**.

## Comportamento
- **Studio**: propone domande nuove e ripassi scaduti **solo** delle materie selezionate.
- **Ripasso**: propone esclusivamente domande **già viste e scadute** delle materie selezionate.
- **Simulazione esame**: nessun filtro manuale; si applicano soltanto le regole
  di estrazione previste per l'esame ufficiale.
- Cambiare filtro non cancella né azzera alcun progresso: tutti gli stati e gli
  eventi rimangono associati ai rispettivi ID e alla versione del listato.
- Se non esistono domande disponibili per il filtro, mostra un messaggio
  chiaro, senza introdurre quesiti di altre materie.
- Gli ID degli argomenti sono specifici del listato e non si riutilizzano
  automaticamente tra edizioni diverse.

## GUI Slint prevista

Nelle schermate Studio e Ripasso, sopra il pulsante **Inizia**, visualizzare:

**Argomenti** · `Tutti` (predefinito) | `Personalizza`

In `Personalizza`: una lista ricercabile con checkbox, contatore
`Selezionati N / Totali M`, pulsanti **Seleziona tutti** e **Deseleziona tutti**,
e per ogni riga **Totali / Studiate / Da ripassare**. Controllare che la selezione
vuota non possa avviare una sessione personalizzata.

Su Android aprire la selezione in una pagina o pannello a tutta altezza
ottimizzato per il tocco; su desktop in un pannello laterale.
La lista deve essere generata dai dati importati, non contenere nomi
di argomento scritti nel codice.

## Comandi CLI implementati

```bash
# Materie ufficiali con ID validi della banca dati scelta
cargo run -- topics-for-license --license B

# Studio solo della materia con ID 11
cargo run -- study-for-license --license B --topic-id 11 --limit 20

# Studio di più materie (ripeti --topic-id o usa 11,22)
cargo run -- study-for-license --license B --topic-id 11 --topic-id 22

# Ripasso: solo domande già studiate e scadute delle materie selezionate
cargo run -- review-for-license --license B --topic-id 11 --limit 20

# Per listati importati manualmente
cargo run -- topics --version DEMO
cargo run -- study --version DEMO --topic-id 11
cargo run -- review --version DEMO --topic-id 11
```

Se non specifichi `--topic-id`, è selezionato **Tutti gli argomenti**.

## Specifiche tecniche
- `questions.topic_id` e `questions.topic` preesistenti in SQLite.
- Query filtrate nel database **prima** del sort/limit.
- ID dei topic controllati rispetto a `source_version`, deduplicati.
- I dati del catalogo vanno validati rispetto al listato ufficiale.
- FSRS-7 resta interno al programma e non appare nella UI.
- La GUI Slint è progettata, **non ancora implementata**.

## Test
`topic_filter_never_leaks_other_topics`: selezione di topic 11 non restituisce
domande del 22; il ripasso non include domande mai viste; un ID inesistente
genera errore. Per verificare la compilazione e i test serve una build Rust
(locale o GitHub Actions).

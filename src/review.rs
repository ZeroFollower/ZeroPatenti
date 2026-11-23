use anyhow::{Context, Result, ensure};
use chrono::Utc;
use fsrs::{FSRS, MemoryState};
use rusqlite::{
    Connection, OptionalExtension, Transaction, params, params_from_iter, types::Value,
};

const SECONDS_PER_DAY: f32 = 86_400.0;

/// Replica la convenzione di pianificazione dell'esempio ufficiale fsrs-rs:
/// arrotondamento ai giorni e minimo di un giorno, per ogni rating.
/// Non introduce ritardi fissi applicativi per Again.
fn due_from_fsrs_interval(reviewed_at: i64, interval_days: f32) -> Result<i64> {
    ensure!(
        interval_days.is_finite() && interval_days >= 0.0,
        "Intervallo FSRS non valido: {interval_days}"
    );
    let seconds = (interval_days.round().max(1.0) as i64)
        .checked_mul(86_400)
        .context("Intervallo FSRS troppo grande")?;
    reviewed_at
        .checked_add(seconds)
        .context("Scadenza FSRS fuori intervallo")
}
#[derive(Debug)]
pub struct StudyCard {
    pub id: String,
    pub topic: String,
    pub text: String,
    pub correct: bool,
    pub figure: Option<String>,
}

pub struct StudyHelp {
    pub brief: String,
    pub brief_source: String,
    pub detail: String,
    pub detail_source: String,
}

/// Use only the current card and other statements about the same subject and figure.
/// A related statement is quoted as a comparison, never presented as a legal rationale.
pub fn study_help(conn: &Connection, card: &StudyCard) -> Result<StudyHelp> {
    let (version, id, brief, detail, attribution) = conn.query_row(
        "SELECT q.source_version,q.external_id,n.brief,n.detail,n.attribution
         FROM questions q LEFT JOIN study_notes n USING(card_id) WHERE q.card_id=?1",
        [&card.id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    let edition = format!("Listato {version} · domanda {id}");
    let editor = format!(
        "Nota editoriale · {}",
        attribution
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("materiale importato")
    );
    let mut comparisons = conn.prepare_cached(
        "SELECT p.external_id,p.question_text,p.correct_answer
         FROM questions q JOIN questions p ON p.source_version=q.source_version
           AND p.topic_id=q.topic_id AND p.group_id=q.group_id AND p.image_path IS q.image_path
         WHERE q.card_id=?1 AND p.question_text<>q.question_text
         ORDER BY p.correct_answer DESC, ABS(p.external_id-q.external_id), p.external_id LIMIT 2",
    )?;
    let related = comparisons
        .query_map([&card.id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (brief, brief_source) = match (brief, related.first()) {
        (Some(note), _) => (note, editor.clone()),
        (None, Some((other, text, correct))) => {
            let verdict = if *correct { "vera" } else { "falsa" };
            (
                format!(
                    "Nello stesso gruppo di quesiti, il listato indica come {verdict} l’affermazione «{text}»."
                ),
                format!("Confronto dal listato {version} · domanda {other}"),
            )
        }
        (None, None) => {
            let summary = if card.correct {
                format!("Ricorda l’affermazione «{}».", card.text)
            } else {
                format!(
                    "Il listato indica come falsa l’affermazione «{}».",
                    card.text
                )
            };
            (summary, format!("Correzione · {edition}"))
        }
    };
    let (detail, detail_source) = match detail {
        Some(note) => (note, editor),
        None => {
            let mut guidance = reading_prompt(&card.text, card.correct);
            for (other, text, correct) in related {
                let verdict = if correct { "Vero" } else { "Falso" };
                guidance.push_str(&format!("\n\n{verdict} · Domanda {other}\n{text}"));
            }
            (guidance, format!("Guida alla lettura · {edition}"))
        }
    };
    Ok(StudyHelp {
        brief,
        brief_source,
        detail,
        detail_source,
    })
}

fn reading_prompt(text: &str, correct: bool) -> String {
    let lowercase = text.to_lowercase();
    let marker = lowercase
        .split(|c: char| !c.is_alphanumeric())
        .find(|word| {
            matches!(
                *word,
                "sempre"
                    | "mai"
                    | "solo"
                    | "soltanto"
                    | "tutti"
                    | "obbligatorio"
                    | "vietato"
                    | "può"
                    | "deve"
            )
        });
    if let Some(marker) = marker {
        return format!(
            "Rileggi la frase prestando attenzione a «{marker}». A chi o a quale azione si riferisce? Prova a spiegare come cambierebbe la frase togliendo questa parola."
        );
    }
    if correct {
        "Ricostruisci a parole tue l’affermazione vera. Poi rileggi il quesito e controlla di aver conservato il soggetto, l’azione e le eventuali condizioni.".into()
    } else {
        "Quale parte dell’affermazione ritieni errata? Separala dal resto della frase e confronta la tua correzione con il materiale di studio dell’argomento.".into()
    }
}

/// Convalida gli ID delle materie per evitare selezioni vuote dovute a errori
/// e controlla la versione: gli ID non sono globali tra edizioni diverse.
fn validate_topics(conn: &Connection, version: &str, selected: &[i64]) -> Result<Vec<i64>> {
    let mut ids = selected.to_vec();
    ids.sort_unstable();
    ids.dedup();
    for id in &ids {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM questions WHERE source_version=?1 AND topic_id=?2",
            params![version, id],
            |r| r.get(0),
        )?;
        ensure!(
            exists > 0,
            "Argomento ID {id} inesistente nel listato {version}. Usa il comando topics."
        );
    }
    Ok(ids)
}

/// Solo domande scadute o nuove in Studio, SOLO scadute già viste in Ripasso.
/// Filtro applicato in SQL prima della selezione: non vengono mai pescate
/// domande di altri argomenti.
pub fn due_cards(
    conn: &Connection,
    version: &str,
    limit: usize,
    selected: &[i64],
    only_reviews: bool,
) -> Result<Vec<StudyCard>> {
    let ids = validate_topics(conn, version, selected)?;
    let now = Utc::now().timestamp();
    let mut query = String::from(
        "SELECT q.card_id,q.topic,q.question_text,q.correct_answer,q.image_path
         FROM questions q LEFT JOIN card_state s ON s.card_id=q.card_id
         WHERE q.source_version=?1 AND ",
    );
    if only_reviews {
        query.push_str("s.card_id IS NOT NULL AND s.due_at<=?2 ");
    } else {
        query.push_str("(s.card_id IS NULL OR s.due_at<=?2) ");
    }
    let mut args = vec![Value::Text(version.to_string()), Value::Integer(now)];
    if !ids.is_empty() {
        let placeholders = (0..ids.len())
            .map(|i| format!("?{}", i + 3))
            .collect::<Vec<_>>()
            .join(",");
        query.push_str(&format!(" AND q.topic_id IN ({placeholders}) "));
        args.extend(ids.into_iter().map(Value::Integer));
    }
    query.push_str(
        " ORDER BY CASE WHEN s.card_id IS NOT NULL THEN 0 ELSE 1 END,
          CASE WHEN s.card_id IS NOT NULL THEN s.due_at END ASC,
          RANDOM()",
    );
    // LIMIT inline is safe here because usize is parsed and bounded separately.
    query.push_str(&format!(" LIMIT {}", limit));
    let mut stmt = conn.prepare(&query)?;
    let mapped = stmt.query_map(params_from_iter(args), |r| {
        Ok(StudyCard {
            id: r.get(0)?,
            topic: r.get(1)?,
            text: r.get(2)?,
            correct: r.get::<_, i64>(3)? != 0,
            figure: r.get(4)?,
        })
    })?;
    Ok(mapped.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub struct AppliedReview {
    pub log_id: i64,
    pub due_at: i64,
}

pub fn apply_review(
    tx: &Transaction<'_>,
    card_id: &str,
    correct: bool,
    rating: u8,
    now: i64,
    retention: f32,
) -> Result<AppliedReview> {
    ensure!(
        (0.70..=0.99).contains(&retention),
        "Retention tra 0.70 e 0.99"
    );
    ensure!(
        (1..=4).contains(&rating) && (correct || rating == 1),
        "Valutazione non valida"
    );
    let state=tx.query_row("SELECT stability,stability_fast,difficulty,last_review_at FROM card_state WHERE card_id=?1",[card_id],|r| {
        Ok((MemoryState { stability:r.get(0)?, stability_fast:r.get(1)?, difficulty:r.get(2)? }, r.get::<_,i64>(3)?))
    }).optional()?;
    let (previous, elapsed) = state.map_or((None, 0.0), |(m, at)| {
        (
            Some(m),
            now.saturating_sub(at).max(0) as f32 / SECONDS_PER_DAY,
        )
    });
    let fsrs = FSRS::default();

    let next = fsrs
        .next_states_with_elapsed_days(previous, retention, elapsed)
        .context("Calcolo FSRS-7 fallito")?;
    let selected = match rating {
        1 => next.again,
        2 => next.hard,
        3 => next.good,
        4 => next.easy,
        _ => unreachable!(),
    };
    let due_at = due_from_fsrs_interval(now, selected.interval)?;
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
        params![
            card_id,
            selected.memory.stability,
            selected.memory.stability_fast,
            selected.memory.difficulty,
            now,
            due_at
        ],
    )?;
    tx.execute(
        "INSERT INTO review_log (
                card_id,reviewed_at,answer_correct,rating,elapsed_days,
                predicted_interval_days,due_at,fsrs_version,desired_retention
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,'7.0.0',?8)",
        params![
            card_id,
            now,
            correct as i64,
            rating as i64,
            elapsed,
            selected.interval,
            due_at,
            retention
        ],
    )?;
    Ok(AppliedReview {
        log_id: tx.last_insert_rowid(),
        due_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::*;

    #[test]
    fn notes_are_imported_as_separate_editorial_content() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        init_database(&db)?;
        let question: SourceQuestion = serde_json::from_value(serde_json::json!({
            "id": 5, "quesito_id": 1, "argomento_id": 2, "argomento_nome": "Segnali",
            "testo": "Testo del quesito", "risposta": false,
            "spiegazione": "Nota breve", "approfondimento": "Nota estesa",
            "fonte_spiegazione": "Manuale locale"
        }))?;
        import_questions(&mut db, "TEST", vec![question])?;
        let notes = db.query_row(
            "SELECT brief, detail, attribution FROM study_notes WHERE card_id='TEST:5'",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?;
        assert_eq!(
            notes,
            (
                "Nota breve".into(),
                "Nota estesa".into(),
                "Manuale locale".into()
            )
        );
        let card = due_cards(&db, "TEST", 1, &[], false)?.pop().unwrap();
        let help = study_help(&db, &card)?;
        assert_eq!(help.brief, "Nota breve");
        assert_eq!(help.detail, "Nota estesa");
        assert_eq!(help.brief_source, "Nota editoriale · Manuale locale");
        assert_eq!(help.detail_source, help.brief_source);
        Ok(())
    }

    #[test]
    fn reading_help_uses_only_related_statements_with_the_same_figure() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        init_database(&db)?;
        let make = |id, group, figure, text, correct| {
            serde_json::from_value(
                serde_json::json!({"id":id, "quesito_id":group, "argomento_id":2,
                "argomento_nome":"Segnali", "figura":figure, "testo":text, "risposta":correct}),
            )
        };
        import_questions(
            &mut db,
            "VERSIONE",
            vec![
                make(1, 10, "a.jpg", "Si applica sempre", false)?,
                make(2, 10, "a.jpg", "Esempio pertinente", true)?,
                make(3, 10, "b.jpg", "Altra figura", true)?,
                make(4, 11, "a.jpg", "Altro gruppo", true)?,
            ],
        )?;
        import_questions(
            &mut db,
            "ALTRO",
            vec![make(5, 10, "a.jpg", "Altra edizione", true)?],
        )?;
        let card = due_cards(&db, "VERSIONE", 4, &[], false)?
            .into_iter()
            .find(|card| card.id == "VERSIONE:1")
            .unwrap();
        let help = study_help(&db, &card)?;
        assert!(help.brief.contains("Esempio pertinente"));
        assert!(help.brief_source.contains("VERSIONE · domanda 2"));
        assert!(help.detail.contains("«sempre»"));
        assert!(!help.detail.contains("Altra"));
        assert!(!help.detail.contains("Altro"));
        Ok(())
    }

    #[test]
    fn topic_filter_never_leaks_other_topics() {
        let mut db = Connection::open_in_memory().unwrap();
        init_database(&db).unwrap();
        let questions = vec![
            SourceQuestion {
                id: 1,
                quesito_id: 1,
                argomento_id: 11,
                argomento_nome: "Segnali".into(),
                testo: "Q1".into(),
                risposta: true,
                figura: None,
                spiegazione: None,
                approfondimento: None,
                fonte_spiegazione: None,
            },
            SourceQuestion {
                id: 2,
                quesito_id: 2,
                argomento_id: 22,
                argomento_nome: "Precedenze".into(),
                testo: "Q2".into(),
                risposta: false,
                figura: None,
                spiegazione: None,
                approfondimento: None,
                fonte_spiegazione: None,
            },
            SourceQuestion {
                id: 3,
                quesito_id: 3,
                argomento_id: 11,
                argomento_nome: "Segnali".into(),
                testo: "Q3".into(),
                risposta: true,
                figura: None,
                spiegazione: None,
                approfondimento: None,
                fonte_spiegazione: None,
            },
        ];
        import_questions(&mut db, "FILTER-TEST", questions).unwrap();
        let selected = due_cards(&db, "FILTER-TEST", 10, &[11], false).unwrap();
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().all(|c| c.topic == "Segnali"));
        assert!(validate_topics(&db, "FILTER-TEST", &[999]).is_err());
        assert_eq!(
            due_cards(&db, "FILTER-TEST", 10, &[11], true)
                .unwrap()
                .len(),
            0
        );
        // Un solo ripasso in scadenza per la materia 11, mai la materia 22.
        db.execute(
            "INSERT INTO card_state VALUES ('FILTER-TEST:1',1,1,5,1,2,1)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO card_state VALUES ('FILTER-TEST:2',1,1,5,1,2,1)",
            [],
        )
        .unwrap();
        let reviews = due_cards(&db, "FILTER-TEST", 10, &[11], true).unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].id, "FILTER-TEST:1");
        assert_eq!(
            due_cards(&db, "FILTER-TEST", 10, &[22], true)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn truth_parsing() {
        assert!(parse_truth("VERO").unwrap());
        assert!(!parse_truth("Falso").unwrap());
        assert!(parse_truth("altro").is_err());
    }

    #[test]
    fn fsrs7_updates_state() {
        let scheduler = FSRS::default();
        let states = scheduler
            .next_states_with_elapsed_days(None, 0.90, 0.0)
            .unwrap();
        assert!(states.good.memory.stability.is_finite());
        assert!(states.good.memory.stability_fast.is_finite());
        assert!(states.good.memory.difficulty.is_finite());
        assert!(states.good.interval > 0.0);
    }

    #[test]
    fn again_uses_fsrs_rs_interval_instead_of_fixed_ten_minutes() {
        let now = 1_700_000_000i64;
        let fsrs = FSRS::default();
        let next = fsrs.next_states_with_elapsed_days(None, 0.90, 0.0).unwrap();
        let expected = now + (next.again.interval.round().max(1.0) as i64) * 86_400;
        assert_eq!(
            due_from_fsrs_interval(now, next.again.interval).unwrap(),
            expected
        );
        assert!(expected >= now + 86_400);
    }

    #[test]
    fn all_fsrs_ratings_obey_upstream_day_rounding() {
        let now = 1_700_000_000i64;
        let fsrs = FSRS::default();
        let next = fsrs.next_states_with_elapsed_days(None, 0.90, 0.0).unwrap();
        for review in [next.again, next.hard, next.good, next.easy] {
            let expected = now + (review.interval.round().max(1.0) as i64) * 86_400;
            assert_eq!(
                due_from_fsrs_interval(now, review.interval).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn fsrs_due_rejects_invalid_intervals() {
        assert!(due_from_fsrs_interval(1_700_000_000, f32::NAN).is_err());
        assert!(due_from_fsrs_interval(1_700_000_000, -1.0).is_err());
    }

    #[test]
    fn import_is_idempotent_and_preserves_progress() {
        let mut db = Connection::open_in_memory().unwrap();
        init_database(&db).unwrap();
        let make = || SourceQuestion {
            id: 1,
            quesito_id: 7,
            argomento_id: 2,
            argomento_nome: "Segnali".into(),
            testo: "Quiz dimostrativo".into(),
            risposta: true,
            figura: None,
            spiegazione: None,
            approfondimento: None,
            fonte_spiegazione: None,
        };
        import_questions(&mut db, "AB-TEST", vec![make()]).unwrap();
        db.execute(
            "INSERT INTO card_state VALUES ('AB-TEST:1',1,1,5,1,2,1)",
            [],
        )
        .unwrap();
        import_questions(&mut db, "AB-TEST", vec![make()]).unwrap();
        let reviews: i64 = db
            .query_row(
                "SELECT repetitions FROM card_state WHERE card_id='AB-TEST:1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reviews, 1);
    }
}

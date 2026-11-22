use super::catalog::{official_quotas, verified_rule};
use super::selection::{self, Candidate, Strategy};
use anyhow::{Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

pub enum Config {
    Official,
    Custom {
        topics: Vec<i64>,
        count: usize,
        strategy: Strategy,
        timed: bool,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Start,
    Resume,
    Pause,
    Cancel,
    Submit,
    Checkpoint,
    Go(usize),
    Answer(Option<bool>),
}

/// The only question projection needed by the in-progress exam UI.
#[derive(Clone, Debug)]
pub struct QuestionView {
    pub id: String,
    pub text: String,
    pub topic_id: i64,
    pub topic: String,
    pub image_path: Option<String>,
    pub answer: Option<bool>,
    pub changes: u32,
}

#[derive(Clone, Debug)]
pub struct SessionView {
    pub id: String,
    pub kind: String,
    pub license: String,
    pub procedure: String,
    pub bank_version: String,
    pub status: String,
    pub index: usize,
    pub threshold: usize,
    pub active_ms: i64,
    pub remaining_ms: Option<i64>,
    pub questions: Vec<QuestionView>,
}

pub fn migrate(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(include_str!("schema.sql"))?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn event(
    tx: &Transaction<'_>,
    id: &str,
    kind: &str,
    now: i64,
    payload: Value,
) -> Result<()> {
    tx.execute(
        "INSERT INTO exam_events(session_id,sequence,event_type,occurred_at,active_ms,payload_json)
         SELECT session_id,1 + COALESCE((SELECT MAX(sequence) FROM exam_events WHERE session_id=?1),0),
                ?2,?3,active_ms,?4 FROM exam_sessions WHERE session_id=?1",
        params![id,kind,now,payload.to_string()],
    )?;
    Ok(())
}

fn select_official(
    candidates: &[Candidate],
    topics: &[i64],
    quotas: &[usize],
    count: usize,
    seed: u64,
    now: i64,
) -> Result<Vec<String>> {
    ensure!(
        quotas.iter().sum::<usize>() == count,
        "Distribuzione incompatibile con la regola d'esame"
    );
    let shuffled = selection::select(
        candidates,
        topics,
        candidates.len(),
        Strategy::Random,
        seed,
        now,
    )?;
    let topic_by_id: HashMap<_, _> = candidates
        .iter()
        .map(|q| (q.id.as_str(), q.topic_id))
        .collect();
    let mut remaining: HashMap<i64, usize> = (1..).zip(quotas.iter().copied()).collect();
    let mut sheet = Vec::with_capacity(count);
    for id in shuffled {
        let topic = topic_by_id[id.as_str()];
        let slots = remaining
            .get_mut(&topic)
            .ok_or_else(|| anyhow::anyhow!("Argomento fuori dalla distribuzione verificata"))?;
        if *slots > 0 {
            *slots -= 1;
            sheet.push(id);
        }
    }
    ensure!(
        sheet.len() == count,
        "Quesiti insufficienti per la distribuzione ministeriale"
    );
    Ok(sheet)
}

pub fn create(
    conn: &mut Connection,
    license: &str,
    procedure: &str,
    config: &Config,
    seed: u64,
    now: i64,
) -> Result<String> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let rule = verified_rule(&tx, license, procedure)?;
    let candidates = tx
        .prepare(
            "SELECT q.card_id,q.topic_id,s.due_at,
                (SELECT COUNT(*) FROM review_log r WHERE r.card_id=q.card_id AND r.answer_correct=0)
         FROM questions q LEFT JOIN card_state s ON q.card_id=s.card_id
         WHERE q.source_version=?1 ORDER BY q.card_id",
        )?
        .query_map([&rule.bank_version], |r| {
            Ok(Candidate {
                id: r.get(0)?,
                topic_id: r.get(1)?,
                due_at: r.get(2)?,
                wrong_count: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (kind, topics, count, strategy, timed) = match config {
        Config::Official => {
            let all_topics: BTreeSet<_> = candidates.iter().map(|q| q.topic_id).collect();
            (
                "official",
                all_topics.into_iter().collect(),
                rule.question_count,
                Strategy::Random,
                true,
            )
        }
        Config::Custom {
            topics,
            count,
            strategy,
            timed,
        } => ("custom", topics.clone(), *count, *strategy, *timed),
    };
    let quotas = match config {
        Config::Official => Some(official_quotas(&rule.bank_version).ok_or_else(|| {
            anyhow::anyhow!(
                "Distribuzione ministeriale dei quesiti non verificata per questa procedura"
            )
        })?),
        Config::Custom { .. } => None,
    };
    let selected = if let Some(quotas) = quotas {
        select_official(&candidates, &topics, quotas, count, seed, now)?
    } else {
        selection::select(&candidates, &topics, count, strategy, seed, now)?
    };
    let threshold = selection::scaled_threshold(count, rule.question_count, rule.max_errors)?;
    let config_json = json!({"topics":topics,"count":count,"timed":timed,
        "official_count":rule.question_count,"official_errors":rule.max_errors,
        "official_duration_seconds":rule.duration_seconds,"official_topic_quotas":quotas});
    let duration_ms = if timed {
        Some(
            i64::try_from(rule.duration_seconds)?
                .checked_mul(1000)
                .ok_or_else(|| anyhow::anyhow!("Durata della prova eccessiva"))?,
        )
    } else {
        None
    };
    // UUID v4, with its version and RFC variant bits set even on offline devices.
    let id: String = tx.query_row(
        "SELECT lower(hex(randomblob(4))||'-'||hex(randomblob(2))||'-4'||substr(hex(randomblob(2)),2)
            ||'-'||substr('89ab',(random() & 3)+1,1)||substr(hex(randomblob(2)),2)||'-'||hex(randomblob(6)))",
        [], |r|r.get(0))?;
    tx.execute(
        "INSERT INTO exam_sessions(session_id,kind,license,procedure,bank_version,rule_version,
            rule_source,config_json,strategy,seed,algorithm_version,status,threshold,duration_ms,
            remaining_ms,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'created',?12,?13,?13,?14,?14)",
        params![
            id,
            kind,
            rule.license,
            rule.procedure,
            rule.bank_version,
            rule.version,
            rule.source,
            config_json.to_string(),
            strategy.as_str(),
            seed.to_string(),
            selection::ALGORITHM_VERSION,
            i64::try_from(threshold)?,
            duration_ms,
            now
        ],
    )?;
    {
        let mut freeze = tx.prepare(
            "INSERT INTO exam_session_questions(session_id,position,question_id,topic_id,topic,question_text,
                correct_answer,image_path,previously_seen)
             SELECT ?1,?2,card_id,topic_id,topic,question_text,correct_answer,image_path,
                EXISTS(SELECT 1 FROM study_card_views WHERE card_id=?3) OR
                EXISTS(SELECT 1 FROM review_log WHERE card_id=?3) OR
                EXISTS(SELECT 1 FROM exam_events WHERE event_type='QuestionDisplayed'
                    AND json_extract(payload_json,'$.question_id')=?3)
             FROM questions WHERE card_id=?3 AND source_version=?4")?;
        for (index, q) in selected.iter().enumerate() {
            ensure!(
                freeze.execute(params![id, i64::try_from(index)?, q, rule.bank_version])? == 1,
                "Quesito selezionato mancante"
            );
        }
    }
    event(&tx, &id, "SessionCreated", now, json!({}))?;
    tx.commit()?;
    Ok(id)
}

pub fn view(conn: &Connection, id: &str) -> Result<SessionView> {
    if !conn.is_autocommit() {
        return read_view(conn, id);
    }
    let tx = conn.unchecked_transaction()?;
    let session = read_view(&tx, id)?;
    tx.commit()?;
    Ok(session)
}

fn read_view(conn: &Connection, id: &str) -> Result<SessionView> {
    let mut session = conn.query_row(
        "SELECT session_id,kind,license,procedure,bank_version,status,current_index,threshold,
            active_ms,remaining_ms FROM exam_sessions WHERE session_id=?1",
        [id],
        |r| {
            Ok(SessionView {
                id: r.get(0)?,
                kind: r.get(1)?,
                license: r.get(2)?,
                procedure: r.get(3)?,
                bank_version: r.get(4)?,
                status: r.get(5)?,
                index: r.get::<_, u32>(6)? as usize,
                threshold: r.get::<_, u32>(7)? as usize,
                active_ms: r.get(8)?,
                remaining_ms: r.get(9)?,
                questions: Vec::new(),
            })
        },
    )?;
    let mut statement = conn.prepare(
        "SELECT q.question_id,q.question_text,q.topic_id,q.topic,q.image_path,a.answer,COALESCE(a.changes,0)
         FROM exam_session_questions q LEFT JOIN exam_answers a USING(session_id,question_id)
         WHERE q.session_id=?1 ORDER BY position",
    )?;
    session.questions = statement
        .query_map([id], |r| {
            Ok(QuestionView {
                id: r.get(0)?,
                text: r.get(1)?,
                topic_id: r.get(2)?,
                topic: r.get(3)?,
                image_path: r.get(4)?,
                answer: r.get(5)?,
                changes: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(session)
}

pub fn unfinished(conn: &Connection) -> Result<Vec<SessionView>> {
    let tx = conn.unchecked_transaction()?;
    let ids = tx
        .prepare(
            "SELECT session_id FROM exam_sessions WHERE status IN ('created','active','paused')
        ORDER BY updated_at DESC, rowid DESC",
        )?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let sessions = ids
        .iter()
        .map(|id| read_view(&tx, id))
        .collect::<Result<_>>()?;
    tx.commit()?;
    Ok(sessions)
}

fn set_status(tx: &Transaction<'_>, id: &str, status: &str, now: i64) -> Result<()> {
    tx.execute(
        "UPDATE exam_sessions SET status=?2,updated_at=?3 WHERE session_id=?1",
        params![id, status, now],
    )?;
    Ok(())
}

fn display(tx: &Transaction<'_>, id: &str, q: &str, now: i64) -> Result<()> {
    event(tx, id, "QuestionDisplayed", now, json!({"question_id":q}))
}

fn hide(tx: &Transaction<'_>, id: &str, q: &str, now: i64, incomplete: bool) -> Result<()> {
    event(
        tx,
        id,
        "QuestionHidden",
        now,
        json!({"question_id":q,"incomplete":incomplete}),
    )
}

fn save_answer(
    tx: &Transaction<'_>,
    id: &str,
    q: &QuestionView,
    value: Option<bool>,
    now: i64,
) -> Result<()> {
    if value == q.answer {
        return Ok(());
    }
    let kind = match (q.answer, value) {
        (_, None) => "AnswerCleared",
        (None, Some(_)) => "AnswerSelected",
        (Some(_), Some(_)) => "AnswerChanged",
    };
    tx.execute(
        "INSERT INTO exam_answers(session_id,question_id,answer,updated_at) VALUES (?1,?2,?3,?4)
        ON CONFLICT(session_id,question_id) DO UPDATE SET answer=excluded.answer,
            updated_at=excluded.updated_at,changes=exam_answers.changes+1",
        params![id, q.id, value, now],
    )?;
    event(
        tx,
        id,
        kind,
        now,
        json!({"question_id":q.id,"answer":value,"previous":q.answer}),
    )
}

/// `active_ms` is absolute monotonic foreground time from ActiveClock, never wall time.
pub fn command(
    conn: &mut Connection,
    id: &str,
    action: Action,
    active_ms: i64,
    now: i64,
) -> Result<SessionView> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let session = read_view(&tx, id)?;
    if session.status == "submitted" && matches!(action, Action::Submit) {
        return Ok(session);
    }
    ensure!(
        !matches!(session.status.as_str(), "submitted" | "cancelled"),
        "Prova già terminata"
    );
    let active = session.status == "active";
    let question = session
        .questions
        .get(session.index)
        .ok_or_else(|| anyhow::anyhow!("Quesito corrente mancante"))?;
    let submitted = active && advance(&tx, id, active_ms, now)?.0 == "submitted";
    if !submitted {
        match action {
            Action::Start | Action::Resume => {
                let (required, kind) = if matches!(action, Action::Start) {
                    ("created", "SessionStarted")
                } else {
                    ("paused", "SessionResumed")
                };
                ensure!(
                    session.status == required,
                    "Avvio o ripresa non disponibili"
                );
                set_status(&tx, id, "active", now)?;
                event(&tx, id, kind, now, json!({}))?;
                display(&tx, id, &question.id, now)?;
            }
            Action::Cancel => {
                if active {
                    hide(&tx, id, &question.id, now, false)?;
                }
                set_status(&tx, id, "cancelled", now)?;
                event(&tx, id, "SessionCancelled", now, json!({}))?;
            }
            Action::Submit => {
                ensure!(active || session.status == "paused", "Prova non avviata");
                if active {
                    hide(&tx, id, &question.id, now, false)?;
                }
                super::result::finish(&tx, id, now)?;
            }
            Action::Pause => {
                ensure!(active, "Prova non attiva");
                hide(&tx, id, &question.id, now, false)?;
                set_status(&tx, id, "paused", now)?;
                event(&tx, id, "SessionPaused", now, json!({"recovered":false}))?;
            }
            Action::Go(index) => {
                ensure!(active, "Prova non attiva");
                let next = session
                    .questions
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("Posizione non valida"))?;
                if index != session.index {
                    hide(&tx, id, &question.id, now, false)?;
                    tx.execute(
                        "UPDATE exam_sessions SET current_index=?2 WHERE session_id=?1",
                        params![id, i64::try_from(index)?],
                    )?;
                    display(&tx, id, &next.id, now)?;
                }
            }
            Action::Answer(value) => {
                ensure!(active, "Prova non attiva");
                save_answer(&tx, id, question, value, now)?;
            }
            Action::Checkpoint => ensure!(active, "Prova non attiva"),
        }
    }
    let updated = read_view(&tx, id)?;
    tx.commit()?;
    Ok(updated)
}

fn advance(
    tx: &Transaction<'_>,
    id: &str,
    active_ms: i64,
    now: i64,
) -> Result<(String, i64, Option<i64>)> {
    let (status, previous, remaining): (String, i64, Option<i64>) = tx.query_row(
        "SELECT status,active_ms,remaining_ms FROM exam_sessions WHERE session_id=?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if status != "active" {
        return Ok((status, previous, remaining));
    }
    let elapsed = active_ms.saturating_sub(previous).max(0);
    let delta = remaining.map_or(elapsed, |left| elapsed.min(left));
    let current = previous + delta;
    let left = remaining.map(|value| value - delta);
    tx.execute(
        "UPDATE exam_sessions SET active_ms=?2,remaining_ms=?3,updated_at=?4 WHERE session_id=?1",
        params![id, current, left, now],
    )?;
    if left == Some(0) {
        let question: String = tx.query_row("SELECT q.question_id FROM exam_sessions s
            JOIN exam_session_questions q ON q.session_id=s.session_id AND q.position=s.current_index
            WHERE s.session_id=?1", [id], |r|r.get(0))?;
        hide(tx, id, &question, now, false)?;
        event(tx, id, "SessionTimedOut", now, json!({}))?;
        super::result::finish(tx, id, now)?;
        return Ok(("submitted".to_owned(), current, left));
    }
    Ok((status, current, left))
}

/// Checkpoints the clock without loading the questionnaire. A late tick reads the terminal state.
pub fn tick(
    conn: &mut Connection,
    id: &str,
    active_ms: i64,
    now: i64,
) -> Result<(String, i64, Option<i64>)> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timing = advance(&tx, id, active_ms, now)?;
    tx.commit()?;
    Ok(timing)
}

/// Call once on process startup, before starting the foreground clock.
pub fn recover(conn: &mut Connection, now: i64) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let active = tx
        .prepare(
            "SELECT s.session_id,q.question_id FROM exam_sessions s
        JOIN exam_session_questions q ON q.session_id=s.session_id AND q.position=s.current_index
        WHERE s.status='active'",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, q) in active {
        hide(&tx, &id, &q, now, true)?;
        set_status(&tx, &id, "paused", now)?;
        event(&tx, &id, "SessionPaused", now, json!({"recovered":true}))?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn populated(conn: &Connection) -> Result<()> {
        crate::storage::init_database(conn)?;
        conn.execute_batch(
            "
            INSERT INTO questions VALUES
                ('AB-7020:1','AB-7020',1,1,11,'Segnali','Prima',1,NULL),
                ('AB-7020:2','AB-7020',2,1,11,'Segnali','Seconda',0,'due.png');",
        )?;
        super::super::catalog::trust_fixture(conn, "B", "conseguimento")?;
        Ok(())
    }

    fn custom(conn: &mut Connection, timed: bool) -> Result<String> {
        let cfg = Config::Custom {
            topics: vec![11],
            count: 2,
            strategy: Strategy::Balanced,
            timed,
        };
        create(conn, "B", "conseguimento", &cfg, u64::MAX, 100)
    }

    fn event_count(conn: &Connection, kind: &str) -> Result<i64> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM exam_events WHERE event_type=?1",
            [kind],
            |r| r.get(0),
        )?)
    }

    #[test]
    fn navigation_pause_and_real_answer_changes_persist() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        populated(&db)?;
        migrate(&db)?;
        let id = custom(&mut db, true)?;
        let start = command(&mut db, &id, Action::Start, 0, 100)?;
        let limit = start.remaining_ms.unwrap();
        command(&mut db, &id, Action::Answer(Some(false)), 100, 101)?;
        command(&mut db, &id, Action::Answer(Some(false)), 200, 101)?;
        command(&mut db, &id, Action::Answer(Some(true)), 300, 102)?;
        command(&mut db, &id, Action::Answer(None), 400, 102)?;
        command(&mut db, &id, Action::Go(1), 500, 103)?;
        command(&mut db, &id, Action::Pause, 765, 104)?;
        let resumed = command(&mut db, &id, Action::Resume, 999_999, 10000)?;
        assert_eq!(resumed.active_ms, 765);
        assert_eq!(resumed.remaining_ms, Some(limit - 765));
        assert_eq!(resumed.index, 1);
        assert_eq!(resumed.questions[0].changes, 2);
        assert_eq!(resumed.questions[0].answer, None);
        assert_eq!(event_count(&db, "AnswerSelected")?, 1);
        assert_eq!(event_count(&db, "AnswerChanged")?, 1);
        assert_eq!(event_count(&db, "AnswerCleared")?, 1);
        assert_eq!(event_count(&db, "QuestionDisplayed")?, 3);
        assert_eq!(event_count(&db, "QuestionHidden")?, 2);
        Ok(())
    }

    #[test]
    fn frozen_questions_survive_catalog_edits_but_new_creation_rechecks_the_bank() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        populated(&db)?;
        let id = custom(&mut db, false)?;
        let before = view(&db, &id)?;
        let first = &before.questions[0];
        db.execute(
            "UPDATE questions SET question_text='Mutata', correct_answer=1-correct_answer,
            image_path='diversa.png' WHERE card_id=?1",
            [&first.id],
        )?;
        let after = view(&db, &id)?;
        assert_eq!(after.questions[0].text, first.text);
        assert_eq!(after.questions[0].image_path, first.image_path);
        assert!(custom(&mut db, false).is_err());
        assert_eq!(unfinished(&db)?.len(), 1);
        Ok(())
    }

    #[test]
    fn a_study_display_without_a_review_is_already_seen_at_exam_creation() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        populated(&db)?;
        db.execute(
            "INSERT INTO study_card_views(card_id,first_seen_at) VALUES ('AB-7020:2',99)",
            [],
        )?;
        let id = custom(&mut db, false)?;
        let seen: bool = db.query_row(
            "SELECT previously_seen FROM exam_session_questions
            WHERE session_id=?1 AND question_id='AB-7020:2'",
            [&id],
            |row| row.get(0),
        )?;
        assert!(seen);
        Ok(())
    }

    #[test]
    fn official_sheet_respects_topic_quotas_and_keeps_rule_duration() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        crate::storage::init_database(&db)?;
        for topic in 1..=25 {
            for variant in 1..=4 {
                let n = topic * 10 + variant;
                db.execute("INSERT INTO questions VALUES (?1,'AB-7020',?2,?2,?3,'Argomento','Quesito',1,NULL)",
                    params![format!("AB-7020:{n}"),n,topic])?;
            }
        }
        super::super::catalog::trust_fixture(&db, "B", "conseguimento")?;
        let id = create(&mut db, "B", "conseguimento", &Config::Official, 42, 100)?;
        let v = view(&db, &id)?;
        assert_eq!(
            (v.questions.len(), v.threshold, v.remaining_ms),
            (30, 3, Some(1_200_000))
        );
        for topic in 1..=25 {
            let actual = v.questions.iter().filter(|q| q.topic_id == topic).count();
            let expected = if [5, 12, 13, 17, 22].contains(&topic) {
                2
            } else {
                1
            };
            assert_eq!(actual, expected, "argomento {topic}");
        }
        Ok(())
    }

    #[test]
    fn verified_revision_bank_allows_custom_but_requires_an_official_distribution() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        crate::storage::init_database(&db)?;
        for n in 1..=30 {
            db.execute(
                "INSERT INTO questions VALUES (?1,'AB-REV-MIT',?2,?2,1,'Segnali','Prova',1,NULL)",
                params![format!("AB-REV-MIT:{n}"), n],
            )?;
        }
        super::super::catalog::trust_fixture(&db, "B", "revisione")?;
        let custom = Config::Custom {
            topics: vec![1],
            count: 2,
            strategy: Strategy::Balanced,
            timed: false,
        };
        create(&mut db, "B", "revisione", &custom, 42, 100)?;
        assert!(create(&mut db, "B", "revisione", &Config::Official, 42, 100).is_err());
        Ok(())
    }

    #[test]
    fn official_am_uses_three_questions_per_topic_with_its_own_timer() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        crate::storage::init_database(&db)?;
        for topic in 1..=10 {
            for variant in 1..=5 {
                let n = topic * 10 + variant;
                db.execute("INSERT INTO questions VALUES (?1,'AM-MIT-2025',?2,?2,?3,'Argomento','Quesito',1,NULL)",
                    params![format!("AM-MIT-2025:{n}"),n,topic])?;
            }
        }
        super::super::catalog::trust_fixture(&db, "AM", "conseguimento")?;
        let id = create(&mut db, "AM", "conseguimento", &Config::Official, 42, 100)?;
        let session = view(&db, &id)?;
        assert_eq!(session.remaining_ms, Some(1_500_000));
        for topic in 1..=10 {
            assert_eq!(
                session
                    .questions
                    .iter()
                    .filter(|q| q.topic_id == topic)
                    .count(),
                3
            );
        }
        Ok(())
    }

    #[test]
    fn an_event_write_failure_rolls_back_answer_and_checkpoint() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        populated(&db)?;
        let id = custom(&mut db, false)?;
        command(&mut db, &id, Action::Start, 0, 100)?;
        db.execute_batch(
            "CREATE TRIGGER fail_answer BEFORE INSERT ON exam_events
            WHEN NEW.event_type='AnswerSelected' BEGIN SELECT RAISE(ABORT,'disk failure'); END;",
        )?;
        assert!(command(&mut db, &id, Action::Answer(Some(true)), 150, 101).is_err());
        let current = view(&db, &id)?;
        assert_eq!(current.active_ms, 0);
        assert_eq!(current.questions[0].answer, None);
        command(&mut db, &id, Action::Cancel, 199, 102)?;
        assert!(command(&mut db, &id, Action::Answer(Some(false)), 200, 102).is_err());
        assert!(command(&mut db, &id, Action::Submit, 200, 102).is_err());
        assert_eq!(event_count(&db, "ReviewApplied")?, 0);
        Ok(())
    }

    #[test]
    fn restart_recovers_only_committed_foreground_time_and_marks_gap() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("zeropatenti-crash-{}.sqlite3", std::process::id()));
        let mut db = Connection::open(&path)?;
        populated(&db)?;
        let id = custom(&mut db, false)?;
        command(&mut db, &id, Action::Start, 0, 100)?;
        command(&mut db, &id, Action::Answer(Some(true)), 500, 101)?;
        command(&mut db, &id, Action::Checkpoint, 2500, 103)?;
        drop(db);
        let mut reopened = Connection::open(&path)?;
        recover(&mut reopened, 59000)?;
        recover(&mut reopened, 59001)?;
        let v = view(&reopened, &id)?;
        assert_eq!(v.status, "paused");
        assert_eq!(v.questions[0].answer, Some(true));
        assert_eq!(v.active_ms, 2500);
        assert_eq!(v.remaining_ms, None);
        assert_eq!(event_count(&reopened, "SessionPaused")?, 1);
        let gap: bool = reopened.query_row(
            "SELECT json_extract(payload_json,'$.incomplete')
            FROM exam_events WHERE event_type='QuestionHidden'",
            [],
            |r| r.get(0),
        )?;
        assert!(gap);
        drop(reopened);
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn timeout_wins_over_answer_and_submission_is_idempotent() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        populated(&db)?;
        let id = custom(&mut db, true)?;
        let start = command(&mut db, &id, Action::Start, 0, 100)?;
        let limit = start.remaining_ms.unwrap();
        let ended = command(&mut db, &id, Action::Answer(Some(true)), limit + 1, 110)?;
        assert_eq!(ended.status, "submitted");
        assert_eq!(ended.active_ms, limit);
        assert_eq!(ended.questions[0].answer, None);
        command(&mut db, &id, Action::Submit, limit + 1, 111)?;
        assert_eq!(event_count(&db, "SessionTimedOut")?, 1);
        assert_eq!(event_count(&db, "SessionSubmitted")?, 1);
        Ok(())
    }

    #[test]
    fn timer_tick_saves_checkpoint_and_submits_exactly_at_deadline() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        populated(&db)?;
        let id = custom(&mut db, true)?;
        let limit = command(&mut db, &id, Action::Start, 0, 100)?
            .remaining_ms
            .unwrap();
        assert_eq!(
            tick(&mut db, &id, 123, 101)?,
            ("active".into(), 123, Some(limit - 123))
        );
        assert_eq!(
            tick(&mut db, &id, limit, 1300)?,
            ("submitted".into(), limit, Some(0))
        );
        assert_eq!(event_count(&db, "SessionTimedOut")?, 1);
        Ok(())
    }

    #[test]
    fn concurrent_checkpoint_and_expired_answer_close_the_exam_once() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "zeropatenti-deadline-{}.sqlite3",
            std::process::id()
        ));
        let mut db = Connection::open(&path)?;
        populated(&db)?;
        let id = custom(&mut db, true)?;
        let deadline = command(&mut db, &id, Action::Start, 0, 100)?
            .remaining_ms
            .unwrap();
        let mut second = Connection::open(&path)?;
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let clock = scope.spawn(|| {
                barrier.wait();
                command(&mut second, &id, Action::Checkpoint, deadline, 1300)
            });
            barrier.wait();
            let tap = command(&mut db, &id, Action::Answer(Some(true)), deadline, 1300);
            let tick = clock.join().unwrap();
            assert!(tap.is_ok() || tick.is_ok());
        });
        assert_eq!(view(&db, &id)?.status, "submitted");
        assert_eq!(event_count(&db, "AnswerSelected")?, 0);
        assert_eq!(event_count(&db, "SessionTimedOut")?, 1);
        assert_eq!(event_count(&db, "SessionSubmitted")?, 1);
        drop(second);
        drop(db);
        std::fs::remove_file(path)?;
        Ok(())
    }
}

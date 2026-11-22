use anyhow::Result;
use rusqlite::{Connection, params};
use std::sync::{Arc, Barrier};
use zeropatenti::{
    exam::{
        result::{correction, submit},
        statistics::{Filters, dashboard},
    },
    storage::init_database,
};

fn fixture(db: &Connection) -> Result<()> {
    init_database(db)?;
    db.execute_batch(
        "
        INSERT INTO questions VALUES
          ('q1','AB-7020',1,1,11,'Segnali','Prima',1,NULL),
          ('q2','AB-7020',2,1,22,'Strade','Seconda',0,NULL),
          ('q3','AB-7020',3,1,11,'Segnali','Terza',1,'terza.png'),
          ('q4','AB-7020',4,1,11,'Segnali','Quarta',1,NULL),
          ('am','AM-MIT-2025',1,1,11,'Segnali','Altra banca',1,NULL);",
    )?;
    session(db, "s", "custom", "balanced")?;
    Ok(())
}

fn session(db: &Connection, id: &str, kind: &str, strategy: &str) -> Result<()> {
    db.execute("INSERT INTO exam_sessions
        (session_id,kind,license,procedure,bank_version,rule_version,rule_source,config_json,
         strategy,seed,algorithm_version,status,threshold,active_ms,created_at,updated_at)
        VALUES (?1,?2,'B','conseguimento','AB-7020','rule','source','{}',?3,'1',1,'paused',1,3500,100,110)",
        params![id,kind,strategy])?;
    db.execute("INSERT INTO exam_session_questions
        SELECT ?1,external_id-1,card_id,topic_id,topic,question_text,correct_answer,image_path,external_id=2
        FROM questions WHERE source_version='AB-7020' AND external_id<=3",[id])?;
    db.execute(
        "INSERT INTO exam_answers VALUES (?1,'q1',1,110,1),(?1,'q2',1,110,1)",
        [id],
    )?;
    for (seq, (event, question, wall, active, answer, incomplete)) in [
        ("QuestionDisplayed", "q2", 100, 0, None, false),
        ("QuestionHidden", "q2", 101, 500, None, false),
        ("QuestionDisplayed", "q1", 101, 500, None, false),
        ("AnswerSelected", "q1", 101, 1000, Some(false), false),
        ("AnswerChanged", "q1", 102, 1500, Some(true), false),
        ("QuestionHidden", "q1", 103, 2000, None, false),
        ("SessionPaused", "", 103, 2000, None, false),
        ("SessionResumed", "", 108, 2000, None, false),
        ("QuestionDisplayed", "q2", 108, 2000, None, false),
        ("AnswerSelected", "q2", 108, 2500, Some(false), false),
        ("AnswerChanged", "q2", 109, 3000, Some(true), false),
        ("QuestionHidden", "q2", 110, 3500, None, true),
        ("SessionPaused", "", 110, 3500, None, true),
    ]
    .into_iter()
    .enumerate()
    {
        let json =
            serde_json::json!({"question_id":question,"answer":answer,"incomplete":incomplete});
        db.execute("INSERT INTO exam_events
            (session_id,sequence,event_type,occurred_at,active_ms,payload_json) VALUES(?,?,?,?,?,?)",
            params![id, seq as i64+1,event,wall,active,json.to_string()])?;
    }
    Ok(())
}

#[test]
fn custom_submission_freezes_correction_and_applies_each_answer_once() -> Result<()> {
    let path = std::env::temp_dir().join(format!(
        "zeropatenti-submit-{}-{}.db",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    let db = Connection::open(&path)?;
    fixture(&db)?;
    drop(db);
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let path = path.clone();
            std::thread::spawn(move || -> Result<_> {
                let mut db = Connection::open(path)?;
                barrier.wait();
                submit(&mut db, "s")
            })
        })
        .collect();
    for worker in workers {
        let report = worker.join().unwrap()?;
        assert_eq!(
            (report.correct, report.wrong, report.omitted, report.passed),
            (1, 1, 1, false)
        );
        assert_eq!(report.margin(), -1);
    }
    let db = Connection::open(&path)?;
    let applications:Vec<(String,i64,bool)>=db.prepare("SELECT a.question_id,r.rating,s.due_at=r.due_at AND s.repetitions=1
        FROM review_applications a JOIN review_log r ON a.review_log_id=r.id JOIN card_state s ON s.card_id=r.card_id ORDER BY 1")?
        .query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    assert_eq!(
        applications,
        vec![("q1".into(), 3, true), ("q2".into(), 1, true)]
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM review_log", [], |r| r
            .get::<_, i64>(0))?,
        2
    );
    db.execute(
        "UPDATE questions SET question_text='Modificata',correct_answer=0",
        [],
    )?;
    let report = correction(&db, "s", true)?;
    assert_eq!(report.questions.len(), 2);
    assert_eq!(report.questions[1].text, "Terza");
    assert!(report.questions[1].solution);
    assert_eq!(report.questions[1].image_path.as_deref(), Some("terza.png"));
    assert_eq!(report.correct, 1);
    drop(db);
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn failed_second_review_rolls_back_first_review_and_result() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    fixture(&db)?;
    db.execute_batch(
        "CREATE TRIGGER fail_review BEFORE INSERT ON review_log
        WHEN NEW.card_id='q2' BEGIN SELECT RAISE(ABORT,'guasto'); END;",
    )?;
    assert!(submit(&mut db, "s").is_err());
    for table in [
        "card_state",
        "review_applications",
        "review_log",
        "exam_results",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))?,
            0
        );
    }
    assert_eq!(db.query_row("SELECT count(*) FROM exam_events WHERE event_type IN ('SessionSubmitted','ReviewApplied')",[],|r|r.get::<_,i64>(0))?,0);
    assert!(correction(&db, "s", false).is_err());
    db.execute_batch("DROP TRIGGER fail_review")?;
    submit(&mut db, "s")?;
    Ok(())
}

#[test]
fn missing_hides_and_recovery_keep_only_observed_intervals() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    fixture(&db)?;
    db.execute_batch(
        "INSERT INTO exam_events(session_id,sequence,event_type,occurred_at,active_ms,payload_json)
        VALUES ('s',14,'SessionResumed',120,3500,'{}'),
        ('s',15,'QuestionDisplayed',120,3500,'{\"question_id\":\"q3\"}'),
        ('s',16,'SessionPaused',140,3600,'{\"recovered\":true}'),
        ('s',17,'SessionResumed',150,3600,'{}'),
        ('s',18,'QuestionDisplayed',150,3600,'{\"question_id\":\"q3\"}'),
        ('s',19,'QuestionHidden',152,5600,'{\"question_id\":\"q3\"}')",
    )?;
    let c = submit(&mut db, "s")?;
    assert_eq!(c.pauses.completed_seconds, 15);
    assert!(c.questions[2].observation.incomplete);
    assert_eq!(c.questions[2].observation.observed_ms, Some(2000));
    Ok(())
}

#[test]
fn historical_filters_keep_bank_license_date_and_procedure_distinct() -> Result<()> {
    use zeropatenti::exam::store::{Action, command};
    let mut db = Connection::open_in_memory()?;
    fixture(&db)?;
    for (id, license, proc, bank, date) in [
        ("s", "B", "conseguimento", "AB-7020", 123),
        ("later", "B", "conseguimento", "AB-7020", 125),
        ("rev", "B", "revisione", "REV", 123),
        ("old", "B", "conseguimento", "vecchia", 123),
        ("a", "A", "conseguimento", "AB-7020", 123),
    ] {
        if id != "s" {
            session(&db, id, "custom", "balanced")?;
        }
        db.execute(
            "UPDATE exam_sessions SET license=?2,procedure=?3,bank_version=?4 WHERE session_id=?1",
            params![id, license, proc, bank],
        )?;
        command(&mut db, id, Action::Submit, 3500, date)?;
    }
    let f = Filters {
        license: Some("B".into()),
        procedure: Some("conseguimento".into()),
        bank_version: Some("AB-7020".into()),
        from: Some(123),
        until: Some(125),
        ..Default::default()
    };
    let d = dashboard(&db, &f, 130)?;
    assert_eq!(d.series.len(), 1);
    assert_eq!(d.series[0].history.len(), 1);
    assert_eq!(d.series[0].history[0].id, "s");
    assert_eq!(d.memory.reviews, 2);
    Ok(())
}

#[test]
fn observed_times_and_history_respect_topic_series_and_memory_scope() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    fixture(&db)?;
    session(&db, "o", "official", "official")?;
    session(&db, "x", "custom", "intelligent")?;
    db.execute(
        "UPDATE exam_sessions SET status='cancelled' WHERE session_id='x'",
        [],
    )?;
    assert!(submit(&mut db, "x").is_err());
    submit(&mut db, "o")?;
    let c = submit(&mut db, "s")?;
    let a = &c.questions[0].observation;
    assert_eq!(
        (a.observed_ms, a.first_answer_ms, a.final_answer_ms),
        (Some(1500), Some(500), Some(1000))
    );
    assert_eq!(a.beneficial_changes, 1);
    let b = &c.questions[1].observation;
    assert_eq!(
        (b.observed_ms, b.returns, b.harmful_changes, b.incomplete),
        (Some(2000), 1, 1, true)
    );
    assert!(b.skipped);
    assert_eq!(
        (b.first_answer_ms, b.final_answer_ms),
        (Some(1000), Some(1500))
    );
    assert_eq!(c.questions[2].observation.observed_ms, None);
    assert_eq!(c.metrics.timing.samples, 1);
    assert_eq!(c.metrics.timing.median_ms, Some(1500.));
    assert_eq!(c.metrics.previously_unseen, 2);
    assert_eq!(c.pauses.completed_seconds, 5);
    let f = Filters {
        license: Some("B".into()),
        procedure: Some("conseguimento".into()),
        ..Default::default()
    };
    let now = chrono::Utc::now().timestamp() + 86400 * 365;
    let d = dashboard(&db, &f, now)?;
    assert_eq!(d.series.len(), 3);
    assert_eq!(d.series.iter().map(|s| s.cancelled).sum::<usize>(), 1);
    assert_eq!(
        (
            d.memory.total,
            d.memory.seen,
            d.memory.studied,
            d.memory.due,
            d.memory.reviews
        ),
        (4, 2, 2, 2, 2)
    );
    assert!(d.memory.estimated_recall.is_some_and(|n| n > 0. && n < 1.));
    assert_eq!(d.memory.repeat_reviews, 0);
    let tx = db.transaction()?;
    zeropatenti::review::apply_review(&tx, "q1", true, 3, now, 0.90)?;
    tx.commit()?;
    let f = Filters {
        topic_id: Some(11),
        strategy: Some("balanced".into()),
        ..f
    };
    let d = dashboard(&db, &f, now)?;
    assert_eq!(
        (
            d.series.len(),
            d.series[0].topics.len(),
            d.series[0].history.len()
        ),
        (1, 1, 1)
    );
    assert_eq!(d.series[0].topics[&11].metrics.omitted, 1);
    assert_eq!(d.memory.total, 3);
    assert_eq!((d.memory.repeat_reviews, d.memory.correct_repeats), (1, 1));
    let d = dashboard(
        &db,
        &Filters {
            until: Some(1),
            ..f
        },
        now,
    )?;
    assert!(d.series.is_empty());
    assert_eq!(d.memory.reviews, 0);
    Ok(())
}

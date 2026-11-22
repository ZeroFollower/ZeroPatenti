use anyhow::Result;
use rusqlite::Connection;
use zeropatenti::{exam::catalog::verified_rule, review::apply_review, storage::init_database};

#[test]
fn migrated_progress_survives_and_unverified_bank_stays_locked() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    db.execute_batch(include_str!("fixtures/v01.sql"))?;
    init_database(&db)?;
    init_database(&db)?;
    let counters = zeropatenti::storage::progress(&db, "DEMO")?;
    assert_eq!(
        (counters.total, counters.studied, counters.reviews),
        (2, 1, 1)
    );
    assert!(verified_rule(&db, "B", "conseguimento").is_err());
    let tx = db.transaction()?;
    let review = apply_review(&tx, "DEMO:2", false, 1, 123456, 0.90)?;
    assert!(review.log_id > 0 && review.due_at > 123456);
    tx.rollback()?;
    let repetitions: i64 = db.query_row("SELECT repetitions FROM card_state", [], |r| r.get(0))?;
    assert_eq!(repetitions, 1);
    let logs: i64 = db.query_row("SELECT count(*) FROM review_log", [], |r| r.get(0))?;
    assert_eq!(logs, 1);
    Ok(())
}

/// Exercise the real v0.1 A/B distribution and a freshly normalized ministry PDF.
/// Both files are supplied locally because the repository does not redistribute them.
#[test]
#[ignore = "requires ZP_AB_V01_JSON and ZP_AB_VERIFIED_JSON local files"]
fn real_ab_upgrade_keeps_topic_filters_and_review_identity() -> Result<()> {
    use std::{env, path::PathBuf};
    use zeropatenti::storage::{import_file, read_json};

    let old = PathBuf::from(env::var("ZP_AB_V01_JSON")?);
    let normalized = PathBuf::from(env::var("ZP_AB_VERIFIED_JSON")?);
    let mut db = Connection::open_in_memory()?;
    db.execute_batch(include_str!("fixtures/v01.sql"))?;
    import_file(&mut db, "AB-7020", &old)?;
    let tx = db.transaction()?;
    let review = apply_review(&tx, "AB-7020:21574", true, 3, 1_700_000_000, 0.9)?;
    tx.commit()?;
    init_database(&db)?;
    assert!(verified_rule(&db, "B", "conseguimento").is_err());
    assert_eq!(import_file(&mut db, "AB-7020", &normalized)?, 7020);
    assert_eq!(
        verified_rule(&db, "B", "conseguimento")?.bank_version,
        "AB-7020"
    );

    let mut lookup = db.prepare("SELECT topic_id FROM questions WHERE card_id=?1")?;
    for original in read_json(&old)? {
        let id = format!("AB-7020:{}", original.id);
        let topic: i64 = lookup.query_row([id], |r| r.get(0))?;
        assert_eq!(topic, original.argomento_id, "ID {}", original.id);
    }
    let persisted = db.query_row(
        "SELECT s.repetitions, s.due_at, l.id FROM card_state s
         JOIN review_log l USING(card_id) WHERE s.card_id='AB-7020:21574'",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        },
    )?;
    assert_eq!(persisted, (1, review.due_at, review.log_id));
    Ok(())
}

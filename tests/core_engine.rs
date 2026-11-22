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

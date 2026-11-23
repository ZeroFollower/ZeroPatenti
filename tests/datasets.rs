use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use zeropatenti::{datasets, exam::store, storage::init_database};

#[test]
fn published_ab_banks_preserve_progress_and_draw_the_right_exam() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("DB");
    let mut profile = Connection::open_in_memory()?;
    init_database(&profile)?;
    let legacy = root.join("obsoleto/AB_2024.sqlite3");
    let current = datasets::local_years(&root)?[&2026].clone();
    assert_eq!(datasets::import(&mut profile, &legacy)?, datasets::LEGACY);
    assert_eq!(datasets::import(&mut profile, &current)?, datasets::AB);
    assert_eq!(datasets::import(&mut profile, &current)?, datasets::AB);
    for (edition, count, pictures) in [(datasets::AB, 7058, 400), (datasets::LEGACY, 7020, 859)] {
        assert_eq!(
            profile.query_row(
                "SELECT COUNT(*) FROM questions WHERE source_version=?",
                [edition],
                |r| r.get::<_, i64>(0)
            )?,
            count
        );
        assert_eq!(
            profile.query_row(
                "SELECT COUNT(*) FROM dataset_images_installed WHERE version=? AND length(bytes)>0",
                [edition],
                |r| r.get::<_, i64>(0)
            )?,
            pictures
        );
    }
    let session = store::create(
        &mut profile,
        "B",
        "conseguimento",
        &store::Config::Official,
        123,
        123,
    )?;
    let (edition,count):(String,i64)=profile.query_row(
        "SELECT s.bank_version, count(*) FROM exam_sessions s
         JOIN exam_session_questions q USING(session_id) WHERE session_id=? GROUP BY s.bank_version",
        [&session], |r| Ok((r.get(0)?, r.get(1)?))
    )?;
    assert_eq!((edition.as_str(), count), (datasets::AB, 30));
    let mut stmt = profile.prepare(
        "SELECT count(*) FROM exam_session_questions e
        JOIN questions q ON q.card_id=e.question_id WHERE e.session_id=?
        GROUP BY q.group_id / 1000 ORDER BY q.group_id / 1000",
    )?;
    let quotas = stmt
        .query_map([&session], |r| r.get::<_, u32>(0).map(|n| n as usize))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert_eq!(
        quotas,
        zeropatenti::exam::catalog::official_quotas(datasets::AB).unwrap()
    );
    assert_eq!(
        profile.query_row("SELECT count(*) FROM review_log", [], |r| r
            .get::<_, i64>(0))?,
        0
    );
    Ok(())
}

#[test]
fn every_new_year_replaces_the_default_and_retires_its_predecessors() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("DB");
    let temp = tempfile::tempdir()?;
    let mut profile = Connection::open_in_memory()?;
    init_database(&profile)?;
    for name in [
        "AB_20300.sqlite3",
        "AB2029.sqlite3",
        "AB_2099.sqlite3.bak",
        "ab_2050.sqlite3",
    ] {
        std::fs::write(temp.path().join(name), [])?;
    }
    let fixture = &datasets::local_years(&root)?[&2026];
    for year in [2026, 2027, 2028] {
        let file = temp.path().join(format!("AB_{year}.sqlite3"));
        std::fs::copy(fixture, &file)?;
        let expected = datasets::edition(year);
        assert_eq!(datasets::known_latest(&profile, temp.path())?, year);
        assert_eq!(datasets::import(&mut profile, &file)?, expected);
        assert_eq!(datasets::latest_installed(&profile)?, Some(year));
        let session = store::create(
            &mut profile,
            "B",
            "conseguimento",
            &store::Config::Official,
            123,
            123,
        )?;
        let actual: String = profile.query_row(
            "SELECT bank_version FROM exam_sessions WHERE session_id=?",
            [&session],
            |r| r.get(0),
        )?;
        assert_eq!(actual, expected);
        let years = datasets::available_years(&profile, temp.path())?;
        assert_eq!(years.last(), Some(&year));
        for &previous in years.range(..year) {
            assert_eq!(
                datasets::warning(previous, year),
                format!("Edizione A/B {previous} obsoleta. Per il conseguimento usa A/B {year}.")
            );
        }
    }
    datasets::remember_published(&mut profile, Some(&[2024, 2026, 2027, 2028, 2029]))?;
    datasets::remember_published(&mut profile, None)?;
    assert_eq!(datasets::known_latest(&profile, temp.path())?, 2029);
    assert_eq!(datasets::latest_installed(&profile)?, Some(2028));
    Ok(())
}

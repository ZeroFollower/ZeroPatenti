use super::*;

#[test]
fn continuous_study_serves_oldest_due_first_and_saves_the_fsrs_interval() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-oldest-due-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let mut c = Controller::open(&dir.join("study.sqlite3"))?;
    let today = now();
    let day = 86_400;
    let tx = c.db.transaction()?;
    for (i, (card, days_ago)) in [
        ("Nuova", None),
        ("Recente", Some(30)),
        ("Futura", Some(0)),
        ("Antica", Some(90)),
    ]
    .into_iter()
    .enumerate()
    {
        tx.execute(
            "INSERT INTO questions VALUES (?1,'AB-7020',?2,1,1,'Segnali',?1,1,NULL)",
            params![card, i as i64 + 1],
        )?;
        if let Some(days) = days_ago {
            review::apply_review(&tx, card, true, 3, today - days * day, 0.90)?;
        }
    }
    tx.commit()?;
    let (ui, state, timer) = bind(c)?;
    ui.invoke_act("study-continuous".into());
    assert_eq!(ui.get_prompt(), "Antica");
    ui.invoke_act("true".into());
    ui.invoke_jump(4); // Facile
    let (reviewed_at, interval, due_at): (i64, f32, i64) = state.borrow().db.query_row(
        "SELECT l.reviewed_at, l.predicted_interval_days, s.due_at FROM card_state s
         JOIN review_log l USING(card_id) WHERE s.card_id='Antica' AND l.rating=4",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let scheduler = fsrs::FSRS::default();
    let previous = scheduler
        .next_states_with_elapsed_days(None, 0.90, 0.)?
        .good
        .memory;
    let elapsed_days = (reviewed_at - (today - 90 * day)) as f32 / day as f32;
    let expected = scheduler
        .next_states_with_elapsed_days(Some(previous), 0.90, elapsed_days)?
        .easy;
    assert_eq!(interval, expected.interval);
    assert_eq!(
        due_at,
        reviewed_at + expected.interval.round().max(1.) as i64 * day
    );

    ui.invoke_act("next".into());
    assert_eq!(ui.get_prompt(), "Recente");
    ui.invoke_act("false".into());
    ui.invoke_act("next".into());
    assert_eq!(ui.get_prompt(), "Nuova");
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn continuous_dashboard_studies_beyond_the_goal_and_picks_newly_due_reviews_first() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-continuous-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let mut c = Controller::open(&dir.join("test.sqlite3"))?;
    for n in 1..=25 {
        c.db.execute(
            "INSERT INTO questions VALUES (?1,'AB-7020',?2,1,1,'Segnali',?1,1,NULL)",
            params![format!("AB-7020:{n}"), n],
        )?;
    }
    let tx = c.db.transaction()?;
    review::apply_review(&tx, "AB-7020:1", true, 3, now(), 0.90)?;
    tx.commit()?;
    // Today's new-card goal has already been met.
    let exam = (Local::now().date_naive() + chrono::Days::new(60))
        .format("%d/%m/%Y")
        .to_string();
    crate::gui::profile::save_exam_date(&c.db, "B", "conseguimento", &exam)?;
    let (ui, state, timer) = bind(c)?;
    assert_eq!(state.borrow().quick_study_count, 0);
    ui.invoke_act("study-continuous".into());
    assert_eq!(ui.get_page(), "study", "{}", ui.get_notice());
    let first = ui.get_prompt();
    assert_ne!(first, "AB-7020:1");
    ui.invoke_act("true".into());
    ui.invoke_jump(3);
    state.borrow().db.execute(
        "UPDATE card_state SET due_at=0 WHERE card_id='AB-7020:1'",
        [],
    )?;
    ui.invoke_act("next".into());
    assert_eq!(ui.get_prompt(), "AB-7020:1");
    ui.invoke_act("study-prev".into());
    assert_eq!(ui.get_prompt(), first);
    ui.invoke_jump(3);
    assert_eq!(ui.get_study_stage(), "history");
    ui.invoke_act("next".into());
    for n in 2..=25 {
        assert_eq!(ui.get_page(), "study", "{}", ui.get_notice());
        assert_eq!(ui.get_study_number(), n);
        ui.invoke_act("false".into());
        ui.invoke_act("next".into());
    }
    assert_eq!(ui.get_page(), "home");
    assert!(ui.get_notice().contains("ripasso"));
    let counts: (i64, i64) = state.borrow().db.query_row(
        "SELECT COUNT(*), COUNT(DISTINCT card_id) FROM review_log",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(counts, (26, 25));
    ui.invoke_act("study-continuous".into());
    assert_ne!(ui.get_page(), "study");
    assert!(ui.get_notice().contains("ripasso"));
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn continuous_review_respects_selected_topics_version_and_due_dates() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!(
        "zp-continuous-filter-{}-{}",
        std::process::id(),
        now()
    ));
    std::fs::create_dir(&dir)?;
    let mut c = Controller::open(&dir.join("test.sqlite3"))?;
    for (card, version, question, topic) in [
        ("A", "AB-7020", 1, 1),
        ("B", "AB-7020", 2, 1),
        ("C", "AB-7020", 3, 1),
        ("D", "AB-7020", 4, 2),
        ("E", "ARCHIVIO", 1, 1),
    ] {
        c.db.execute(
            "INSERT INTO questions VALUES (?1,?2,?3,1,?4,?4,?1,1,NULL)",
            params![card, version, question, topic],
        )?;
    }
    let tx = c.db.transaction()?;
    for id in ["A", "B", "D", "E"] {
        review::apply_review(&tx, id, true, 3, now(), 0.90)?;
    }
    tx.execute("UPDATE card_state SET due_at=0 WHERE card_id <> 'B'", [])?;
    tx.commit()?;
    let (ui, state, timer) = bind(c)?;
    ui.invoke_act("review".into());
    ui.invoke_topic_toggle(1);
    ui.set_unlimited_study(true);
    ui.invoke_act("start".into());
    assert_eq!(ui.get_prompt(), "A");
    assert!(ui.get_study_continuous());
    ui.invoke_act("true".into());
    ui.invoke_jump(4);
    ui.invoke_act("next".into());
    assert_eq!(ui.get_page(), "home");
    assert!(!ui.get_study_continuous());
    ui.invoke_act("review".into());
    ui.invoke_topic_toggle(1);
    ui.invoke_act("start".into());
    assert_eq!(ui.get_page(), "study-setup");
    assert!(ui.get_notice().contains("ripasso"));
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

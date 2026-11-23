use super::*;

fn onboarding_db(case: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "zp-onboarding-{case}-{}-{}",
        std::process::id(),
        now()
    ));
    std::fs::create_dir(&dir)?;
    Ok(dir.join("profile.sqlite3"))
}

#[test]
fn onboarding_undecided_persists_after_reopen() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let path = onboarding_db("undecided")?;
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert!(ui.get_show_exam_onboarding());
    ui.set_onboarding_date("12/04/2028".into());
    ui.invoke_act("onboarding-undecided".into());
    assert!(!ui.get_show_exam_onboarding());
    drop((timer, ui, state));
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert!(!ui.get_show_exam_onboarding());
    assert!(ui.get_exam_date().is_empty());
    ui.invoke_act("settings".into());
    ui.set_exam_date_input("12/04/2028".into());
    ui.invoke_act("save-exam-date".into());
    assert_eq!(ui.get_exam_date(), "12/04/2028");
    drop((timer, ui, state));
    std::fs::remove_dir_all(path.parent().unwrap())?;
    Ok(())
}

#[test]
fn onboarding_requires_a_valid_date_or_an_explicit_choice() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let path = onboarding_db("interrupted")?;
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    for value in ["", "31/02/2028", "1/4/2028"] {
        ui.set_onboarding_date(value.into());
        ui.invoke_act("onboarding-save".into());
        assert!(ui.get_show_exam_onboarding());
        assert!(!ui.get_onboarding_error().is_empty());
        assert!(ui.get_exam_date().is_empty());
    }
    drop((timer, ui, state));
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert!(ui.get_show_exam_onboarding());
    assert!(ui.get_onboarding_error().is_empty());
    drop((timer, ui, state));
    std::fs::remove_dir_all(path.parent().unwrap())?;
    Ok(())
}

#[test]
fn onboarding_saves_for_the_restored_profile_and_stays_closed_after_date_removal() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let path = onboarding_db("dated")?;
    // A legacy installation with a selected profile but no exam date.
    Connection::open(&path)?.execute_batch(
        "CREATE TABLE gui_active_profile (id INTEGER PRIMARY KEY, license TEXT, procedure TEXT);
         INSERT INTO gui_active_profile VALUES (1, 'A2', 'revisione');",
    )?;
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert!(ui.get_show_exam_onboarding());
    ui.set_onboarding_date("12/04/2028".into());
    ui.invoke_act("onboarding-save".into());
    assert!(!ui.get_show_exam_onboarding());
    assert_eq!(ui.get_exam_date(), "12/04/2028");
    drop((timer, ui, state));
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert!(!ui.get_show_exam_onboarding());
    assert_eq!(ui.get_exam_date(), "12/04/2028");
    ui.set_license("B".into());
    ui.invoke_act("bank".into());
    assert!(ui.get_exam_date().is_empty());
    ui.set_license("A2".into());
    ui.invoke_act("bank".into());
    ui.invoke_act("settings".into());
    assert_eq!(ui.get_exam_date_input(), "12/04/2028");
    ui.set_exam_date_input("".into());
    ui.invoke_act("save-exam-date".into());
    drop((timer, ui, state));
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert!(!ui.get_show_exam_onboarding());
    assert!(ui.get_exam_date().is_empty());
    drop((timer, ui, state));
    std::fs::remove_dir_all(path.parent().unwrap())?;
    Ok(())
}

#[test]
fn onboarding_recognizes_a_legacy_date_on_another_profile() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let path = onboarding_db("legacy")?;
    Connection::open(&path)?.execute_batch(
        "CREATE TABLE gui_exam_dates (license TEXT, procedure TEXT, exam_date TEXT, PRIMARY KEY (license,procedure));
         INSERT INTO gui_exam_dates VALUES ('A2', 'revisione', '2028-04-12');",
    )?;
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert!(!ui.get_show_exam_onboarding());
    assert!(ui.get_exam_date().is_empty());
    ui.set_license("A2".into());
    ui.set_procedure("revisione".into());
    ui.invoke_act("bank".into());
    assert_eq!(ui.get_exam_date(), "12/04/2028");
    ui.invoke_act("settings".into());
    ui.set_exam_date_input("".into());
    ui.invoke_act("save-exam-date".into());
    drop((timer, ui, state));
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert!(!ui.get_show_exam_onboarding());
    assert!(ui.get_exam_date().is_empty());
    drop((timer, ui, state));
    std::fs::remove_dir_all(path.parent().unwrap())?;
    Ok(())
}

#[test]
fn onboarding_date_is_rolled_back_if_recording_the_choice_fails() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let path = onboarding_db("retry")?;
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    state.borrow().db.execute_batch(
        "CREATE TRIGGER reject_choice BEFORE INSERT ON gui_exam_onboarding
         BEGIN SELECT RAISE(ABORT, 'write failed'); END;",
    )?;
    ui.set_onboarding_date("12/04/2028".into());
    ui.invoke_act("onboarding-save".into());
    assert!(ui.get_show_exam_onboarding());
    assert!(!ui.get_onboarding_error().is_empty());
    assert_eq!(
        crate::gui::profile::exam_date(&state.borrow().db, "B", "conseguimento")?,
        None
    );
    state
        .borrow()
        .db
        .execute_batch("DROP TRIGGER reject_choice")?;
    ui.invoke_act("onboarding-save".into());
    assert!(!ui.get_show_exam_onboarding());
    assert_eq!(ui.get_exam_date(), "12/04/2028");
    drop((timer, ui, state));
    std::fs::remove_dir_all(path.parent().unwrap())?;
    Ok(())
}

#[test]
fn completed_goal_reopens_when_a_review_becomes_due_on_the_same_day() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-next-review-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let mut c = Controller::open(&dir.join("profile.sqlite3"))?;
    let noon = Local
        .with_ymd_and_hms(2027, 3, 3, 12, 0, 0)
        .single()
        .unwrap();
    c.db.execute(
        "INSERT INTO questions VALUES ('AB-7020:1','AB-7020',1,1,1,'Segnali','Domanda',1,NULL)",
        [],
    )?;
    let tx = c.db.transaction()?;
    review::apply_review(&tx, "AB-7020:1", true, 3, noon.timestamp() - 86400, 0.90)?;
    tx.execute("UPDATE card_state SET due_at=?1", [noon.timestamp() + 60])?;
    tx.commit()?;
    crate::gui::profile::save_exam_date(&c.db, "B", "conseguimento", "05/03/2027")?;
    let (ui, state, timer) = bind(c)?;
    state.borrow_mut().refresh_dashboard(&ui, noon)?;
    assert_eq!(state.borrow().quick_study_count, 0);
    state
        .borrow_mut()
        .refresh_dashboard_if_needed(&ui, noon + chrono::Duration::seconds(59))?;
    assert_eq!(ui.get_due(), 0);
    state
        .borrow_mut()
        .refresh_dashboard_if_needed(&ui, noon + chrono::Duration::minutes(1))?;
    assert_eq!(ui.get_due(), 1);
    assert_eq!(state.borrow().quick_study_count, 1);
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn dated_quick_study_tracks_graded_new_cards_and_due_reviews() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-goal-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let path = dir.join("profile.sqlite3");
    let mut c = Controller::open(&path)?;
    for n in 1..=63 {
        c.db.execute(
            "INSERT INTO questions VALUES (?1,'AB-7020',?2,1,1,'Segnali',?1,1,NULL)",
            params![format!("AB-7020:{n}"), n],
        )?;
    }
    let tx = c.db.transaction()?;
    review::apply_review(&tx, "AB-7020:1", true, 3, now() - 10 * 86400, 0.90)?;
    tx.execute("UPDATE card_state SET due_at=0", [])?;
    tx.commit()?;
    let (ui, state, timer) = bind(c)?;
    let today = Local::now().date_naive();
    let exam = (today + chrono::Days::new(2))
        .format("%d/%m/%Y")
        .to_string();
    assert_eq!(ui.get_recommended(), 19);
    ui.invoke_act("settings".into());
    ui.set_exam_date_input(exam.as_str().into());
    ui.invoke_act("save-exam-date".into());
    assert_eq!(ui.get_exam_date(), exam.as_str());
    assert!(ui.get_exam_countdown().contains("2 giorni"));
    assert_eq!(ui.get_recommended(), 31);
    ui.invoke_act("study-start".into());
    assert_eq!(ui.get_study_total(), 32);
    assert_eq!(ui.get_prompt(), "AB-7020:1");
    ui.invoke_act("false".into());
    ui.invoke_act("next".into());
    ui.invoke_act("home".into());
    assert_eq!(ui.get_due(), 0);
    assert_eq!(ui.get_recommended(), 31);
    ui.invoke_act("study-start".into());
    ui.invoke_act("true".into());
    ui.invoke_jump(3);
    ui.invoke_act("home".into());
    assert_eq!(ui.get_recommended(), 30);
    assert!(ui.get_study_goal().contains("1 su 31"));
    drop((timer, ui, state));
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert_eq!(ui.get_exam_date(), exam.as_str());
    assert_eq!(ui.get_recommended(), 30);
    ui.set_license("A2".into());
    ui.invoke_act("bank".into());
    assert!(ui.get_exam_date().is_empty());
    ui.set_license("B".into());
    ui.invoke_act("bank".into());
    assert_eq!(ui.get_exam_date_input(), exam.as_str());
    ui.set_exam_date_input("31/02/2027".into());
    ui.invoke_act("save-exam-date".into());
    assert_eq!(ui.get_exam_date(), exam.as_str());
    assert!(!ui.get_notice().is_empty());
    ui.set_exam_date_input(
        (today + chrono::Days::new(70))
            .format("%d/%m/%Y")
            .to_string()
            .into(),
    );
    ui.invoke_act("save-exam-date".into());
    assert_eq!(ui.get_recommended(), 0);
    ui.invoke_act("study-start".into());
    assert_eq!(ui.get_page(), "home");
    assert!(ui.get_notice().contains("completato"));
    ui.set_exam_date_input(today.format("%d/%m/%Y").to_string().into());
    ui.invoke_act("save-exam-date".into());
    assert!(ui.get_exam_countdown().to_lowercase().contains("oggi"));
    assert_eq!(ui.get_recommended(), 61);
    ui.set_exam_date_input(
        (today - chrono::Days::new(1))
            .format("%d/%m/%Y")
            .to_string()
            .into(),
    );
    ui.invoke_act("save-exam-date".into());
    assert!(ui.get_exam_countdown().contains("trascorsa"));
    assert_eq!(ui.get_recommended(), 20);
    ui.set_exam_date_input("".into());
    ui.invoke_act("save-exam-date".into());
    assert!(ui.get_exam_countdown().is_empty());
    assert!(ui.get_study_goal().is_empty());
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn restores_selected_license_and_procedure_on_reopen() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-profile-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let path = dir.join("profile.sqlite3");
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    ui.set_license("A2".into());
    ui.set_procedure("revisione".into());
    ui.invoke_act("bank".into());
    assert!(ui.get_notice().is_empty(), "{}", ui.get_notice());
    drop((timer, ui, state));
    let (ui, state, timer) = bind(Controller::open(&path)?)?;
    assert_eq!(ui.get_license(), "A2");
    assert_eq!(ui.get_procedure(), "revisione");
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

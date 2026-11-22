use super::*;
use slint::platform::{
    Platform, WindowAdapter,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};

struct TestPlatform;
impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

#[test]
fn slint_callbacks_import_pause_reopen_submit_and_filter_results() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-ui-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let db = dir.join("test.sqlite3");
    let csv = dir.join("questions.csv");
    std::fs::write(&csv, include_bytes!("../../examples/domande_demo.csv"))?;
    let (ui, state, timer) = bind(Controller::open(&db)?)?;
    ui.invoke_act("import".into());
    ui.set_import_path(csv.to_string_lossy().as_ref().into());
    // An unrelated bank under B must leave the selection available for a clean retry.
    ui.invoke_act("import-file".into());
    assert_eq!(ui.get_page(), "import");
    assert!(ui.get_notice().contains("non coincide"));
    assert_eq!(storage::progress(&state.borrow().db, "AB-7020")?.total, 0);
    {
        let mut c = state.borrow_mut();
        storage::import_file(&mut c.db, "AB-7020", &csv)?;
        crate::exam::catalog::trust_fixture(&c.db, "B", "conseguimento")?;
        c.db.execute("DELETE FROM questions", [])?;
    }
    ui.invoke_act("import-file".into());
    assert_eq!(ui.get_page(), "home", "{}", ui.get_notice());
    assert!(ui.get_bank().contains("2 domande"));
    let original_root = ui.get_asset_root();
    let replacement = dir.join("replacement");
    std::fs::create_dir(&replacement)?;
    let conflicting = replacement.join("questions.csv");
    let sample = include_str!("../../examples/domande_demo.csv");
    for contents in ["{invalid".to_owned(), sample.replace("DEMO:", "Changed:")] {
        std::fs::write(&conflicting, contents)?;
        ui.invoke_act("import".into());
        ui.set_import_path(conflicting.to_string_lossy().as_ref().into());
        ui.set_asset_root(replacement.to_string_lossy().as_ref().into());
        ui.invoke_act("import-file".into());
        assert_eq!(ui.get_page(), "import");
        ui.invoke_act("home".into());
        assert_eq!(ui.get_asset_root(), original_root);
    }
    ui.set_count(2);
    ui.invoke_act("start".into());
    assert_eq!(ui.get_page(), "exam", "{}", ui.get_notice());
    ui.invoke_act("true".into());
    ui.invoke_act("false".into());
    ui.invoke_jump(1);
    let writer = Connection::open(&db)?;
    state.borrow().db.busy_timeout(Duration::ZERO)?;
    writer.execute_batch("BEGIN IMMEDIATE")?;
    ui.invoke_act("pause".into());
    let frozen = state
        .borrow()
        .pending_pause
        .context("Il tempo deve fermarsi anche se SQLite è occupato")?
        .0;
    assert!(state.borrow().clock.is_none());
    writer.execute_batch("ROLLBACK")?;
    drop(writer);
    state.borrow_mut().tick(&ui)?;
    assert_eq!(state.borrow().session.as_ref().unwrap().active_ms, frozen);
    assert_eq!(ui.get_page(), "paused", "{}", ui.get_notice());
    assert!(Controller::open(&db).is_err());
    drop(timer);
    drop(ui);
    drop(state);

    let (ui, state, timer) = bind(Controller::open(&db)?)?;
    let saved = ui
        .get_sessions()
        .row_data(0)
        .context("Sessione sospesa mancante")?;
    ui.invoke_open_entry("paused".into(), saved.key.clone());
    ui.invoke_act("resume".into());
    assert_eq!(ui.get_position(), 1);
    ui.invoke_jump(0);
    assert_eq!(ui.get_answer(), "Falso");
    let first = state.borrow().session.as_ref().unwrap().questions[0]
        .text
        .clone();
    let second = state.borrow().session.as_ref().unwrap().questions[1]
        .text
        .clone();
    let correct: bool = state.borrow().db.query_row(
        "SELECT correct_answer FROM exam_session_questions WHERE session_id=? AND position=0",
        [saved.key.as_str()],
        |r| r.get(0),
    )?;
    ui.invoke_act(if correct {
        "true".into()
    } else {
        "false".into()
    });
    ui.invoke_act("submit".into());
    assert_eq!(ui.get_page(), "result", "{}", ui.get_notice());
    assert!(!ui.get_details().is_empty());
    ui.set_errors_only(true);
    ui.invoke_act("result".into());
    assert!(!ui.get_details().contains(&first));
    assert!(ui.get_details().contains(&second));
    assert!(ui.get_subtitle().contains("Segnaletica"));
    ui.invoke_act("home".into());
    ui.invoke_act("stats".into());
    assert_eq!(ui.get_history().row_count(), 1, "{}", ui.get_notice());
    assert_eq!(ui.get_history().row_data(0).unwrap().key, saved.key);
    ui.invoke_open_entry("result".into(), saved.key);
    let logs: i64 = state
        .borrow()
        .db
        .query_row("SELECT count(*) FROM review_log", [], |r| r.get(0))?;
    assert_eq!(logs, 1);
    drop(timer);
    drop(ui);
    drop(state);
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn media_cleanup_preserves_shared_roots_and_external_folders() -> Result<()> {
    let base = std::env::temp_dir().join(format!("zp-media-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&base)?;
    let c = Controller::open(&base.join("db"))?;
    let shared = c.staged_media.join("listato-shared");
    let discarded = c.staged_media.join("listato-discarded");
    let unrelated = base.join("listato-external");
    let unknown = c.staged_media.join("unrelated");
    for dir in [&shared, &discarded, &unrelated, &unknown] {
        std::fs::create_dir_all(dir)?;
    }
    Controller::save_media(&c.db, "AB", &shared.to_string_lossy())?;
    Controller::save_media(&c.db, "AM", &shared.to_string_lossy())?;
    Controller::save_media(&c.db, "AB", &unrelated.to_string_lossy())?;
    c.discard_unused_media()?;
    assert!(shared.is_dir());
    assert!(!discarded.exists());
    Controller::save_media(&c.db, "AM", "")?;
    c.discard_unused_media()?;
    assert!(!shared.exists());
    assert!(unrelated.is_dir() && unknown.is_dir());
    drop(c);
    std::fs::remove_dir_all(base)?;
    Ok(())
}

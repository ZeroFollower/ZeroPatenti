use super::*;
mod banks;
mod continuous;
mod profile;
mod study_ui;
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
    assert_eq!((ui.get_official_count(), ui.get_official_minutes()), (0, 0));
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
    assert_eq!(
        ui.get_page(),
        "import",
        "L’importazione senza firma richiede conferma"
    );
    assert_eq!(storage::progress(&state.borrow().db, "AB-7020")?.total, 0);
    ui.invoke_act("confirm-unsigned".into());
    assert_eq!(ui.get_page(), "home", "{}", ui.get_notice());
    assert!(ui.get_bank().contains("2 domande"));
    ui.invoke_act("verify-bank".into());
    assert!(ui.get_notice().contains("AB-7020 verificato"));
    assert_eq!(
        (ui.get_official_count(), ui.get_official_minutes()),
        (30, 20)
    );
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
    assert!(saved.label.contains("Tempo residuo"));
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
    // In Italy this UTC timestamp belongs to 2 January locally.
    let submitted = 1_767_310_200;
    let local_date = Local.timestamp_opt(submitted, 0).single().unwrap();
    {
        let c = state.borrow();
        c.db.execute(
            "UPDATE exam_sessions SET updated_at=? WHERE session_id=?",
            params![submitted, saved.key.as_str()],
        )?;
        c.db.execute(
            "UPDATE exam_results SET submitted_at=? WHERE session_id=?",
            params![submitted, saved.key.as_str()],
        )?;
    }
    ui.set_since(local_date.format("%Y-%m-%d").to_string().into());
    ui.set_until(ui.get_since());
    ui.invoke_act("stats".into());
    assert_eq!(ui.get_history().row_count(), 1, "{}", ui.get_notice());
    assert_eq!(ui.get_history().row_data(0).unwrap().key, saved.key);
    assert!(ui.get_stats_overview().contains("Copertura"));
    assert!(!ui.get_stats_overview().contains("Andamento recente"));
    let trend_date = format!("Andamento recente\n{}", local_date.format("%d/%m/%Y"));
    assert!(
        ui.get_details().contains(&trend_date),
        "{}",
        ui.get_details()
    );
    // A replaced catalog edition still has a frozen correction and topic list.
    state.borrow().db.execute(
        "UPDATE exam_sessions SET bank_version='AB-ARCHIVIO' WHERE session_id=?",
        [saved.key.as_str()],
    )?;
    ui.invoke_act("stats".into());
    assert_eq!(ui.get_history().row_count(), 0);
    assert!(ui.get_stats_banks().iter().any(|v| v == "AB-ARCHIVIO"));
    ui.set_stats_bank("AB-ARCHIVIO".into());
    ui.invoke_act("stats".into());
    assert_eq!(ui.get_history().row_count(), 1);
    assert!(
        ui.get_stats_topics()
            .iter()
            .any(|v| v.contains("Segnaletica"))
    );
    let selected_topic = ui
        .get_stats_topics()
        .iter()
        .find(|v| v.contains("Segnaletica"))
        .unwrap();
    ui.set_stats_topic(selected_topic.clone());
    ui.invoke_act("stats".into());
    ui.invoke_open_entry("result".into(), saved.key);
    ui.invoke_act("stats".into());
    assert_eq!(ui.get_stats_bank(), "AB-ARCHIVIO");
    assert_eq!(ui.get_stats_topic(), selected_topic);
    assert_eq!(ui.get_history().row_count(), 1);
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
fn guided_study_keeps_graded_answers_when_revisiting() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-study-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let csv = dir.join("questions.csv");
    std::fs::write(&csv, include_bytes!("../../examples/domande_demo.csv"))?;
    let mut c = Controller::open(&dir.join("db"))?;
    storage::import_file(&mut c.db, "AB-7020", &csv)?;
    let (ui, state, timer) = bind(c)?;
    assert_eq!(
        (
            ui.get_total(),
            ui.get_studied(),
            ui.get_due(),
            ui.get_recommended(),
            ui.get_memory()
        ),
        (2, 0, 0, 2, -1)
    );
    ui.invoke_act("study-start".into());
    assert_eq!(ui.get_page(), "study", "{}", ui.get_notice());
    assert_eq!((ui.get_study_number(), ui.get_study_total()), (1, 2));
    assert!(ui.get_study_solution().is_empty());
    assert!(ui.get_study_explanation().is_empty());
    assert!(ui.get_study_detail().is_empty());
    let first = ui.get_prompt();
    ui.invoke_act("next".into());
    assert_eq!(ui.get_prompt(), first);
    let is_true = first.contains("rosso");
    ui.invoke_act(if is_true { "true" } else { "false" }.into());
    assert_eq!(ui.get_study_stage(), "rate");
    assert!(!ui.get_study_explanation().is_empty());
    assert!(ui.get_study_detail_source().contains("AB-7020"));
    ui.invoke_jump(3);
    ui.invoke_act("next".into());
    let second = ui.get_prompt();
    assert_ne!(first, second);
    ui.invoke_act("study-prev".into());
    assert_eq!(ui.get_prompt(), first);
    assert_eq!(ui.get_study_stage(), "history");
    assert_eq!(ui.get_answer(), truth(is_true));
    assert!(ui.get_study_correct());
    assert_eq!(ui.get_study_solution(), truth(is_true));
    assert_eq!(ui.get_study_rating(), 3);
    ui.invoke_jump(1);
    ui.invoke_act("next".into());
    assert_eq!(ui.get_prompt(), second);
    assert_eq!(ui.get_study_stage(), "answer");
    assert!(ui.get_study_detail().is_empty());
    assert_eq!(ui.get_study_number(), 2);
    ui.invoke_act(if is_true { "true" } else { "false" }.into());
    assert_eq!(ui.get_study_stage(), "next");
    assert!(!ui.get_study_correct());
    assert!(!ui.get_study_detail().is_empty());
    assert_eq!(ui.get_study_solution(), truth(!is_true));
    assert_eq!(ui.get_study_rating(), 1);
    ui.invoke_act("next".into());
    assert_eq!(ui.get_page(), "home");
    assert_eq!(storage::progress(&state.borrow().db, "AB-7020")?.reviews, 2);
    assert_eq!(
        (
            ui.get_studied(),
            ui.get_due(),
            ui.get_recommended(),
            ui.get_memory()
        ),
        (2, 0, 0, 100)
    );
    state.borrow().db.execute(
        "UPDATE card_state SET last_review_at=last_review_at-86400000, due_at=0",
        [],
    )?;
    ui.invoke_act("home".into());
    assert_eq!(ui.get_due(), 2);
    assert!((0..100).contains(&ui.get_memory()));
    ui.invoke_act("review".into());
    assert_eq!(ui.get_mode(), "Ripasso");
    assert_eq!(ui.get_page(), "study-setup");
    ui.invoke_act("settings".into());
    assert!(ui.get_bank_list().iter().any(|e| e.key == "AM-MIT-2025"));
    ui.invoke_open_entry("bank".into(), "AM-MIT-2025".into());
    assert_eq!(ui.get_page(), "settings");
    assert_eq!(ui.get_active_version(), "AM-MIT-2025");
    assert_eq!((ui.get_total(), ui.get_memory()), (0, -1));
    assert!(!ui.get_official_ready());
    ui.invoke_act("study-start".into());
    assert_eq!(ui.get_page(), "import");
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn exam_grid_tracks_answers_across_page_boundaries() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-grid-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let db = dir.join("test.sqlite3");
    let csv = dir.join("questions.csv");
    let mut questions =
        String::from("id,quesito_id,argomento_id,argomento_nome,testo,risposta,figura\n");
    for n in 1..=27 {
        questions.push_str(&format!(
            "{n},10,1,Segnaletica,Domanda di prova {n},VERO,\n"
        ));
    }
    std::fs::write(&csv, questions)?;
    let mut c = Controller::open(&db)?;
    storage::import_file(&mut c.db, "AB-7020", &csv)?;
    crate::exam::catalog::trust_fixture(&c.db, "B", "conseguimento")?;
    let (ui, state, timer) = bind(c)?;
    ui.set_count(27);
    ui.set_timed(false);
    ui.invoke_act("start".into());
    assert_eq!(ui.get_page(), "exam", "{}", ui.get_notice());
    assert_eq!(ui.get_grid().row_count(), 25);
    assert_eq!(ui.get_grid().row_data(24).unwrap(), "25");

    ui.invoke_jump(24);
    ui.invoke_act("true".into());
    assert_eq!(ui.get_grid().row_data(24).unwrap(), "25 ✓");
    ui.invoke_jump(25);
    assert_eq!(ui.get_grid().row_count(), 2);
    assert_eq!(ui.get_grid().row_data(0).unwrap(), "26");
    ui.invoke_act("false".into());
    assert_eq!(ui.get_grid().row_data(0).unwrap(), "26 ✓");
    ui.invoke_jump(24);
    assert_eq!(ui.get_grid().row_data(24).unwrap(), "25 ✓");
    ui.invoke_jump(25);
    ui.invoke_act("clear".into());
    assert_eq!(ui.get_grid().row_data(0).unwrap(), "26");
    let id = state.borrow().session.as_ref().unwrap().id.clone();
    ui.invoke_act("settings".into());
    assert_eq!(ui.get_page(), "settings", "{}", ui.get_notice());
    assert!(state.borrow().clock.is_none());
    assert_eq!(store::view(&state.borrow().db, &id)?.status, "paused");
    ui.invoke_act("simulation".into());
    assert_eq!(ui.get_mode(), "Ufficiale");
    assert!(
        ui.get_sessions()
            .iter()
            .any(|entry| entry.key == id.as_str())
    );
    ui.invoke_open_entry("cancel-saved".into(), id.as_str().into());
    assert_eq!(store::view(&state.borrow().db, &id)?.status, "paused");
    ui.invoke_open_entry("resume-saved".into(), id.as_str().into());
    assert_eq!(ui.get_page(), "exam");
    ui.invoke_act("pause".into());
    ui.invoke_act("simulation".into());
    ui.invoke_topic_toggle(0);
    let chosen = ui.get_selected_count();
    ui.set_cancel_target(id.as_str().into());
    ui.invoke_open_entry("cancel-saved".into(), id.as_str().into());
    assert_eq!(store::view(&state.borrow().db, &id)?.status, "cancelled");
    assert_eq!(ui.get_sessions().row_count(), 0);
    assert_eq!(ui.get_selected_count(), chosen);
    assert!(!ui.get_topics().row_data(0).unwrap().chosen);
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

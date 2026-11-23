use super::*;
use slint::{
    LogicalPosition, PhysicalSize, Rgb8Pixel, SharedPixelBuffer,
    platform::{PointerEventButton, WindowEvent},
};

fn click(ui: &AppWindow, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    ui.window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    ui.window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
    slint::platform::update_timers_and_animations();
}

// Count pixels of the test figure independently of fonts, labels and theme colours.
fn figure_area(ui: &AppWindow) -> Result<(usize, LogicalPosition)> {
    slint::platform::update_timers_and_animations();
    let shot = ui.window().take_snapshot()?;
    let mut count = 0;
    let mut center = (0usize, 0usize);
    for (i, pixel) in shot.as_slice().iter().enumerate() {
        if (pixel.r, pixel.g, pixel.b) == (231, 0, 153) {
            count += 1;
            center.0 += i % shot.width() as usize;
            center.1 += i / shot.width() as usize;
        }
    }
    ensure!(count > 0, "Figura assente");
    Ok((
        count,
        LogicalPosition::new((center.0 / count) as f32, (center.1 / count) as f32),
    ))
}

#[test]
fn searching_and_toggling_a_topic_preserves_the_session_configuration() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-topics-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let mut c = Controller::open(&dir.join("test.sqlite3"))?;
    let csv = dir.join("questions.csv");
    std::fs::write(
        &csv,
        "id,quesito_id,argomento_id,argomento_nome,testo,risposta\n1,1,1,Divieti,A,VERO\n2,2,2,Precedenze,B,FALSO\n",
    )?;
    storage::import_file(&mut c.db, crate::datasets::AB, &csv)?;
    // This pointer-layout test uses a local fixture, independently of GitHub availability.
    c.study_edition = Some((
        crate::datasets::LEGACY.to_owned(),
        crate::datasets::AB.to_owned(),
    ));
    let (ui, state, timer) = crate::gui::bind(c)?;
    ui.invoke_act("onboarding-undecided".into());
    ui.invoke_act("study-setup".into());
    ui.set_count(7);
    ui.window().set_size(PhysicalSize::new(390, 800));
    ui.show()?;
    slint::platform::update_timers_and_animations();
    click(&ui, 155., 312.);
    click(&ui, 100., 96.);
    for key in ["p", "r", "e"] {
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    }
    assert_eq!(ui.get_search(), "pre");
    click(&ui, 125., 230.);
    assert_eq!(ui.get_selected_count(), 1);
    assert!(ui.get_topics().row_data(0).unwrap().chosen);
    assert!(!ui.get_topics().row_data(1).unwrap().chosen);
    click(&ui, 122., 761.);
    assert_eq!(ui.get_count(), 7);
    assert_eq!(ui.get_page(), "study-setup");
    ui.invoke_act("start".into());
    assert_eq!(ui.get_study_total(), 1);
    assert_eq!(ui.get_subtitle(), "Divieti");
    ui.hide()?;
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn zoom_enlarges_the_figure_and_preserves_an_answered_exam_question() -> Result<()> {
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-zoom-{}-{}", std::process::id(), now()));
    std::fs::create_dir(&dir)?;
    let mut c = Controller::open(&dir.join("test.sqlite3"))?;
    let csv = dir.join("questions.csv");
    std::fs::write(&csv, include_bytes!("../../../examples/domande_demo.csv"))?;
    storage::import_file(&mut c.db, "AB-7020", &csv)?;
    crate::exam::catalog::trust_fixture(&c.db, "B", "conseguimento")?;
    let (ui, state, timer) = bind(c)?;
    ui.invoke_act("onboarding-undecided".into());
    ui.set_count(2);
    ui.invoke_act("start".into());
    ui.invoke_jump(1);
    ui.invoke_act("false".into());
    assert_eq!(ui.get_page(), "exam");
    let id = state.borrow().session.as_ref().unwrap().id.clone();
    let question = ui.get_prompt();
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(16, 16);
    buffer.make_mut_slice().fill(Rgb8Pixel::new(231, 0, 153));
    ui.set_figure(Image::from_rgb8(buffer));
    ui.set_has_figure(true);
    ui.show()?;
    for (width, height, plus_x, controls_y) in [(390, 800, 265., 767.), (1080, 680, 935., 623.)] {
        ui.window().set_size(PhysicalSize::new(width, height));
        let (thumbnail, center) = figure_area(&ui)?;
        click(&ui, center.x, center.y);
        let (fitted, _) = figure_area(&ui)?;
        assert!(
            fitted > thumbnail * 2,
            "Il visualizzatore deve ingrandire la figura"
        );
        click(&ui, plus_x, controls_y);
        assert!(
            figure_area(&ui)?.0 > fitted,
            "Lo zoom deve ingrandire i pixel della figura"
        );
        let close_x = if width == 390 { 342. } else { 1000. };
        click(&ui, close_x, controls_y);
        assert_eq!(figure_area(&ui)?.0, thumbnail);
        assert_eq!(ui.get_prompt(), question);
        assert_eq!(ui.get_position(), 1);
        assert_eq!(ui.get_answer(), "Falso");
        let saved = store::view(&state.borrow().db, &id)?;
        assert_eq!((saved.index, saved.questions[1].answer), (1, Some(false)));
        assert_eq!(saved.status, "active");
    }
    ui.hide()?;
    drop((timer, ui, state));
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

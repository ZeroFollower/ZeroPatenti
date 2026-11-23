use super::*;

#[test]
fn folder_picker_accepts_renamed_list_and_rejects_ambiguous_packages() -> Result<()> {
    use crate::gui::banks::questions_in;
    use std::fs;
    let dir = std::env::temp_dir().join(format!("zp-picker-{}-{}", std::process::id(), now()));
    fs::create_dir(&dir)?;
    let renamed = dir.join("AB2020.json");
    fs::write(&renamed, r#"{"domande":[]}"#)?;
    fs::write(dir.join("manifest.json"), "{}")?;
    assert_eq!(questions_in(&dir)?, renamed);
    fs::write(dir.join("questions.csv"), "")?;
    assert!(questions_in(&dir).is_err());
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn signed_bank_checks_manifest_figures_and_persisted_origin() -> Result<()> {
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use std::fs;
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-signature-{}-{}", std::process::id(), now()));
    fs::create_dir(&dir)?;
    let figures = dir.join("figures");
    fs::create_dir(&figures)?;
    let file = dir.join("questions.csv");
    let raw = "id,quesito_id,argomento_id,argomento_nome,testo,risposta,figura\n1,1,1,Segnali,Segnale di prova,VERO,figures/1.png\n";
    fs::write(&file, raw)?;
    let image = hex::decode(
        "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c63f8cfc0f01f00050001ff89993d1d0000000049454e44ae426082",
    )?;
    fs::write(figures.join("1.png"), &image)?;
    let key = SigningKey::from_bytes(&[99; 32]);
    let public = hex::encode(key.verifying_key().as_bytes());
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format": 1, "version": "AB-7020",
        "files": {"questions.csv": hex::encode(Sha256::digest(raw)), "figures/1.png": hex::encode(Sha256::digest(&image))}
    }))?;
    fs::write(dir.join("manifest.json"), &manifest)?;
    let db = dir.join("db");
    let mut c = Controller::open(&db)?;
    storage::import_file(&mut c.db, "AB-7020", &file)?;
    crate::exam::catalog::trust_fixture(&c.db, "B", "conseguimento")?;
    c.db.execute("DELETE FROM questions", [])?;
    let (ui, state, timer) = bind(c)?;
    ui.invoke_act("import".into());
    ui.set_import_path(file.to_string_lossy().as_ref().into());
    ui.invoke_act("import-file".into());
    assert!(ui.get_notice().contains("chiave pubblica"));
    ui.set_bank_public_key(public.into());
    ui.invoke_act("save-bank-key".into());
    ui.invoke_act("import-file".into());
    assert!(ui.get_notice().contains("manifest.sig"));
    let bad_sig = SigningKey::from_bytes(&[41; 32]).sign(&manifest);
    fs::write(dir.join("manifest.sig"), bad_sig.to_bytes())?;
    ui.invoke_act("confirm-unsigned".into());
    assert!(ui.get_notice().contains("non valida"));
    assert_eq!(storage::progress(&state.borrow().db, "AB-7020")?.total, 0);
    let mut incomplete: serde_json::Value = serde_json::from_slice(&manifest)?;
    incomplete["files"]
        .as_object_mut()
        .unwrap()
        .remove("figures/1.png");
    let incomplete = serde_json::to_vec(&incomplete)?;
    fs::write(dir.join("manifest.json"), &incomplete)?;
    fs::write(dir.join("manifest.sig"), key.sign(&incomplete).to_bytes())?;
    ui.invoke_act("import-file".into());
    assert!(ui.get_notice().contains("tutti e soli"));
    fs::write(dir.join("manifest.json"), &manifest)?;
    fs::write(dir.join("manifest.sig"), key.sign(&manifest).to_bytes())?;
    ui.invoke_act("import-file".into());
    assert_eq!(ui.get_page(), "home", "{}", ui.get_notice());
    ui.invoke_act("verify-bank".into());
    assert!(ui.get_notice().contains("figure verificati"));
    ui.invoke_act("study-start".into());
    assert!(ui.get_has_figure());
    ui.invoke_act("home".into());
    let mut altered = image.clone();
    altered.extend_from_slice(b"changed");
    fs::write(figures.join("1.png"), altered)?;
    ui.invoke_act("study-start".into());
    assert!(
        !ui.get_has_figure(),
        "Una figura firmata modificata dopo l’importazione va rifiutata anche al momento della visualizzazione"
    );
    ui.invoke_act("home".into());
    ui.invoke_act("verify-bank".into());
    assert!(ui.get_notice().contains("SHA-256"));
    fs::write(figures.join("1.png"), &image)?;
    fs::write(&file, raw.replace("Segnale di prova", "Quesito cambiato"))?;
    ui.invoke_act("verify-bank".into());
    assert!(ui.get_notice().contains("SHA-256"));
    fs::write(&file, raw)?;
    fs::remove_file(dir.join("manifest.json"))?;
    fs::remove_file(dir.join("manifest.sig"))?;
    ui.invoke_act("verify-bank".into());
    assert!(ui.get_notice().contains("manifest"));
    // A later CLI import may replace editorial notes under the same catalog digest.
    // It must invalidate the GUI provenance of the previous signed installation.
    storage::import_file(&mut state.borrow_mut().db, "AB-7020", &file)?;
    ui.invoke_act("verify-bank".into());
    assert!(ui.get_notice().contains("Nessuna firma"));
    fs::write(&file, raw.replace("figures/1.png", "../outside.png"))?;
    ui.invoke_act("import".into());
    ui.set_import_path(file.to_string_lossy().as_ref().into());
    ui.invoke_act("import-file".into());
    assert!(ui.get_notice().contains("Percorso figura non valido"));
    drop((timer, ui, state));
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn archive_preview_protects_progress_current_editions_and_external_files() -> Result<()> {
    use std::fs;
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let dir = std::env::temp_dir().join(format!("zp-archive-{}-{}", std::process::id(), now()));
    fs::create_dir(&dir)?;
    let csv = dir.join("questions.csv");
    fs::write(&csv, include_bytes!("../../../examples/domande_demo.csv"))?;
    let mut c = Controller::open(&dir.join("db"))?;
    for version in [
        "AB-7020",
        "AM-MIT-2025",
        "OLD-USED",
        "OLD-UNUSED",
        "OLD-NEW-PROGRESS",
    ] {
        storage::import_file(&mut c.db, version, &csv)?;
    }
    storage::mark_seen(&c.db, "OLD-USED:1", 5)?;
    Controller::save_media(&c.db, "OLD-UNUSED", &dir.to_string_lossy())?;
    let managed = c.staged_media.join("listato-retired");
    let unrelated = c.staged_media.join("listato-pending");
    fs::create_dir_all(&managed)?;
    fs::create_dir_all(&unrelated)?;
    storage::import_file(&mut c.db, "OLD-MANAGED", &csv)?;
    Controller::save_media(&c.db, "OLD-MANAGED", &managed.to_string_lossy())?;
    let (ui, state, timer) = bind(c)?;
    ui.invoke_act("import".into());
    ui.invoke_act("clean-archive".into());
    assert_eq!(
        storage::progress(&state.borrow().db, "OLD-UNUSED")?.total,
        2
    );
    ui.invoke_act("preview-archive".into());
    assert!(ui.get_cleanup_message().contains("OLD-UNUSED"));
    assert!(!ui.get_cleanup_message().contains("OLD-USED"));
    ui.invoke_act("cancel-cleanup".into());
    assert!(!ui.get_cleanup_pending());
    ui.invoke_act("preview-archive".into());
    storage::mark_seen(&state.borrow().db, "OLD-NEW-PROGRESS:1", 7)?;
    ui.invoke_act("clean-archive".into());
    assert!(ui.get_notice().contains("cambiate"));
    ui.invoke_act("preview-archive".into());
    ui.invoke_act("clean-archive".into());
    assert_eq!(
        storage::progress(&state.borrow().db, "OLD-UNUSED")?.total,
        0
    );
    for version in ["AB-7020", "AM-MIT-2025", "OLD-USED", "OLD-NEW-PROGRESS"] {
        assert_eq!(storage::progress(&state.borrow().db, version)?.total, 2);
    }
    assert!(csv.is_file());
    assert!(!managed.exists());
    assert!(unrelated.is_dir());
    assert!(!ui.get_can_cleanup());
    assert!(ui.get_bank_storage().contains("byte"));
    {
        let controller = state.borrow();
        let integrity: String = controller
            .db
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        assert_eq!(integrity, "ok");
        assert!(
            !controller
                .db
                .prepare("PRAGMA foreign_key_check")?
                .exists([])?
        );
    }
    drop((timer, ui, state));
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn ab_catalog_moves_the_default_and_warns_on_each_older_selection() -> Result<()> {
    use crate::datasets;
    slint::platform::set_platform(Box::new(TestPlatform))?;
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("DB");
    std::fs::create_dir(&dir)?;
    let controller = Controller::open(&tmp.path().join("progressi.sqlite3"))?;
    let (ui, state, _timer) = super::super::bind(controller)?;
    assert_eq!(ui.get_active_version().as_str(), datasets::AB);
    for year in [2027, 2028] {
        // Catalog discovery reads filenames without importing or downloading the edition.
        std::fs::write(dir.join(format!("AB_{year}.sqlite3")), [])?;
        state.borrow_mut().bank_management(&ui)?;
        assert_eq!(ui.get_active_version().as_str(), datasets::edition(year));
        assert!(ui.get_edition_warning().is_empty());
        let entries = ui.get_bank_list();
        for previous in [2024, 2026, 2027].into_iter().filter(|&old| old < year) {
            let version = datasets::edition(previous);
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.key == version && entry.label.contains("Obsoleto"))
            );
            ui.invoke_open_entry("bank".into(), version.into());
            assert_eq!(
                ui.get_edition_warning().as_str(),
                datasets::warning(previous, year)
            );
        }
        ui.invoke_open_entry("bank".into(), datasets::edition(year).into());
        assert!(ui.get_edition_warning().is_empty());
    }
    Ok(())
}

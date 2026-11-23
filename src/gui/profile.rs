use anyhow::{Context, Result, ensure};
use chrono::{Datelike, NaiveDate};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS gui_active_profile (
            id INTEGER PRIMARY KEY CHECK (id=1),
            license TEXT NOT NULL,
            procedure TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS gui_exam_dates (
            license TEXT NOT NULL,
            procedure TEXT NOT NULL,
            exam_date TEXT NOT NULL,
            PRIMARY KEY (license, procedure)
        );
        CREATE TABLE IF NOT EXISTS gui_exam_onboarding (
            id INTEGER PRIMARY KEY CHECK (id=1)
        );
        INSERT OR IGNORE INTO gui_exam_onboarding (id)
        SELECT 1 WHERE EXISTS (SELECT 1 FROM gui_exam_dates);",
    )?;
    Ok(())
}

pub(super) fn needs_exam_onboarding(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM gui_exam_onboarding)",
        [],
        |row| row.get(0),
    )?)
}

pub(super) fn finish_exam_onboarding(
    db: &mut Connection,
    license: &str,
    procedure: &str,
    date: Option<&str>,
) -> Result<()> {
    let tx = db.transaction()?;
    if let Some(value) = date {
        ensure!(
            !value.trim().is_empty(),
            "Inserisci una data nel formato GG/MM/AAAA."
        );
        save_exam_date(&tx, license, procedure, value)?;
    }
    tx.execute("INSERT INTO gui_exam_onboarding (id) VALUES (1)", [])?;
    tx.commit()?;
    Ok(())
}

pub(super) fn active(db: &Connection) -> Result<Option<(String, String)>> {
    Ok(db
        .query_row(
            "SELECT license, procedure FROM gui_active_profile WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

pub(super) fn select(db: &Connection, license: &str, procedure: &str) -> Result<()> {
    db.execute(
        "INSERT INTO gui_active_profile (id,license,procedure) VALUES (1,?1,?2)
         ON CONFLICT(id) DO UPDATE SET license=excluded.license, procedure=excluded.procedure",
        params![license, procedure],
    )?;
    Ok(())
}

pub(super) fn exam_date(
    db: &Connection,
    license: &str,
    procedure: &str,
) -> Result<Option<NaiveDate>> {
    let date: Option<String> = db
        .query_row(
            "SELECT exam_date FROM gui_exam_dates WHERE license=?1 AND procedure=?2",
            params![license, procedure],
            |r| r.get(0),
        )
        .optional()?;
    date.map(|value| value.parse().context("Data d’esame salvata non valida"))
        .transpose()
}

pub(super) fn save_exam_date(
    db: &Connection,
    license: &str,
    procedure: &str,
    value: &str,
) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        db.execute(
            "DELETE FROM gui_exam_dates WHERE license=?1 AND procedure=?2",
            params![license, procedure],
        )?;
    } else {
        let hint = "Inserisci una data valida nel formato GG/MM/AAAA.";
        let date =
            NaiveDate::parse_from_str(value, "%d/%m/%Y").map_err(|_| anyhow::anyhow!(hint))?;
        ensure!(
            value.len() == 10 && date.year() > 0 && date.format("%d/%m/%Y").to_string() == value,
            hint
        );
        db.execute(
            "INSERT INTO gui_exam_dates (license,procedure,exam_date) VALUES (?1,?2,?3)
             ON CONFLICT(license,procedure) DO UPDATE SET exam_date=excluded.exam_date",
            params![license, procedure, date.to_string()],
        )?;
    }
    Ok(())
}

pub(super) fn countdown(exam: NaiveDate, today: NaiveDate) -> String {
    let date = exam.format("%d/%m/%Y");
    match (exam - today).num_days() {
        d if d < 0 => format!("Data d’esame trascorsa · {date}"),
        0 => format!("Esame oggi · {date}"),
        1 => format!("Esame domani · {date}"),
        days => format!("Esame tra {days} giorni · {date}"),
    }
}

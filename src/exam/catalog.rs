//! The digest identifies the verified normalization, not just the filename or row count.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize)]
pub struct ExamRule {
    pub license: String,
    pub procedure: String,
    pub bank_version: String,
    pub version: String,
    pub source: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub question_count: usize,
    pub max_errors: usize,
    pub duration_seconds: u64,
}

pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS exam_rules (
        license TEXT NOT NULL, procedure TEXT NOT NULL, bank_version TEXT NOT NULL,
        version TEXT NOT NULL, source TEXT NOT NULL, valid_from TEXT NOT NULL, valid_until TEXT,
        question_count INTEGER NOT NULL, max_errors INTEGER NOT NULL, duration_seconds INTEGER NOT NULL,
        PRIMARY KEY(license, procedure, version));
        CREATE TABLE IF NOT EXISTS bank_attestations (
        bank_version TEXT PRIMARY KEY, digest TEXT NOT NULL, source TEXT NOT NULL, verified_on TEXT NOT NULL);")?;
    let catalog = crate::catalog::load()?;
    for bank in catalog.listati.into_iter().filter(|bank| bank.pronto) {
        let Some(edition) = bank.exam else { continue };
        conn.execute(
            "INSERT OR REPLACE INTO bank_attestations VALUES (?1, ?2, ?3, ?4)",
            params![
                bank.versione,
                edition.digest,
                bank.fonte,
                catalog.checked_at
            ],
        )?;
        for license in bank.patenti {
            conn.execute(
                "INSERT OR REPLACE INTO exam_rules VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    license,
                    bank.procedura,
                    bank.versione,
                    edition.version,
                    edition.source,
                    edition.valid_from,
                    edition.valid_until,
                    edition.question_count,
                    edition.max_errors,
                    edition.duration_seconds
                ],
            )?;
        }
    }
    Ok(())
}

fn bank_digest(conn: &Connection, version: &str) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT external_id,group_id,topic_id,topic,question_text,correct_answer,image_path,card_id
        FROM questions WHERE source_version=?1 ORDER BY external_id",
    )?;
    let mut hash = Sha256::new();
    let mut rows = stmt.query([version])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        ensure!(
            row.get::<_, String>(7)? == format!("{version}:{id}"),
            "Identità della domanda modificata"
        );
        let body = (
            id,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, bool>(5)?,
            row.get::<_, Option<String>>(6)?,
        );
        hash.update(serde_json::to_vec(&body)?);
        hash.update(b"\n");
        count += 1;
    }
    ensure!(count > 0, "Banca dati {version} non ancora importata");
    Ok(format!("{:x}", hash.finalize()))
}

/// Call inside the same transaction that reads and freezes the questions.
pub fn verified_rule(conn: &Connection, license: &str, procedure: &str) -> Result<ExamRule> {
    let bank = crate::catalog::version_for(license, procedure)?;
    let license = license.trim().to_ascii_uppercase();
    let procedure = procedure.trim().to_ascii_lowercase();
    let rule = conn
        .query_row(
            "SELECT version, source, valid_from, valid_until, question_count, max_errors, duration_seconds
             FROM exam_rules
             WHERE license=?1 AND procedure=?2 AND bank_version=?3 AND valid_from<=DATE('now')
               AND (valid_until IS NULL OR valid_until>=DATE('now'))
             ORDER BY valid_from DESC LIMIT 1",
            params![license, procedure, bank],
            |r| {
                Ok(ExamRule {
                    license: license.clone(),
                    procedure: procedure.clone(),
                    bank_version: bank.clone(),
                    version: r.get(0)?,
                    source: r.get(1)?,
                    valid_from: r.get(2)?,
                    valid_until: r.get(3)?,
                    question_count: r.get::<_, u32>(4)? as usize,
                    max_errors: r.get::<_, u32>(5)? as usize,
                    duration_seconds: u64::from(r.get::<_, u32>(6)?),
                })
            },
        )
        .optional()?
        .context("Nessuna regola verificata per questa patente e procedura")?;
    let expected: String = conn
        .query_row(
            "SELECT digest FROM bank_attestations WHERE bank_version=?1",
            [&bank],
            |r| r.get(0),
        )
        .optional()?
        .context("Listato senza attestazione ministeriale verificata")?;
    ensure!(
        bank_digest(conn, &bank)? == expected,
        "Il listato {bank} non coincide con l'edizione ministeriale verificata"
    );
    Ok(rule)
}

#[cfg(test)]
pub(crate) fn trust_fixture(conn: &Connection, license: &str, procedure: &str) -> Result<()> {
    let bank = crate::catalog::version_for(license, procedure)?;
    let digest = bank_digest(conn, &bank)?;
    conn.execute(
        "INSERT OR REPLACE INTO bank_attestations VALUES (?1,?2,'fixture','2026-11-22')",
        params![bank, digest],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verified_content_rejects_changes_and_wrong_procedure() -> Result<()> {
        let db = Connection::open_in_memory()?;
        crate::storage::init_database(&db)?;
        db.execute("INSERT INTO questions VALUES ('AB-7020:1','AB-7020',1,1,1,'Segnali','Prova',1,'figures/1.png')",[])?;
        db.execute("DELETE FROM bank_attestations", [])?;
        assert!(verified_rule(&db, "B", "conseguimento").is_err());
        trust_fixture(&db, "B", "conseguimento")?;
        let rule = verified_rule(&db, "B", "conseguimento")?;
        assert_eq!(
            (rule.question_count, rule.max_errors, rule.duration_seconds),
            (30, 3, 1200)
        );
        assert!(verified_rule(&db, "B", "revisione").is_err());
        db.execute("UPDATE questions SET correct_answer=0", [])?;
        assert!(verified_rule(&db, "B", "conseguimento").is_err());
        db.execute("UPDATE questions SET correct_answer=1,image_path=NULL", [])?;
        assert!(verified_rule(&db, "B", "conseguimento").is_err());
        Ok(())
    }
}

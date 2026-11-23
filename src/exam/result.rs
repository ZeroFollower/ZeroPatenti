use super::statistics::{Metrics, Observation, Pauses, Topic, observations, topics};
use super::store;
use anyhow::{Result, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Question {
    pub position: usize,
    pub id: String,
    pub topic_id: i64,
    pub topic: String,
    pub text: String,
    pub solution: bool,
    pub answer: Option<bool>,
    pub image_path: Option<String>,
    pub previously_seen: bool,
    pub observation: Observation,
}

impl Question {
    pub fn correct(&self) -> bool {
        self.answer == Some(self.solution)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Correction {
    pub kind: String,
    pub threshold: i64,
    pub correct: usize,
    pub wrong: usize,
    pub omitted: usize,
    pub passed: bool,
    pub active_ms: i64,
    pub remaining_ms: Option<i64>,
    pub questions: Vec<Question>,
    pub metrics: Metrics,
    pub topics: BTreeMap<i64, Topic>,
    pub pauses: Pauses,
}

impl Correction {
    pub fn margin(&self) -> i64 {
        self.threshold - (self.wrong + self.omitted) as i64
    }

    pub fn summary(&self) -> String {
        let mode = if self.kind == "official" {
            "ufficiale"
        } else {
            "personalizzata"
        };
        let esito = if self.passed {
            "superata"
        } else {
            "non superata"
        };
        let mut s = format!(
            "Prova {mode} {esito}\n{} corrette · {} errate · {} omesse ({:.0}% corrette)\nErrori ammessi {} · Margine {:+}\nTempo attivo {:.1} s",
            self.correct,
            self.wrong,
            self.omitted,
            self.metrics.accuracy() * 100.,
            self.threshold,
            self.margin(),
            self.active_ms as f64 / 1000.
        );
        if let Some(ms) = self.remaining_ms {
            let _ = write!(s, " · Residuo {:.1} s", ms as f64 / 1000.);
        }
        let _ = write!(
            s,
            "\n{} domande nuove · {} cambiate in corrette · {} cambiate in errate\n{} pause · {} s di pausa misurati",
            self.metrics.previously_unseen,
            self.metrics.beneficial_changes,
            self.metrics.harmful_changes,
            self.pauses.count,
            self.pauses.completed_seconds
        );
        self.metrics.describe_times(&mut s);
        for (outcome, t) in &self.metrics.time_by_outcome {
            if let (Some(mean), Some(median)) = (t.mean_ms, t.median_ms) {
                let _ = write!(
                    s,
                    "\nDomande {} · media {:.1} s · mediana {:.1} s",
                    outcome,
                    mean / 1000.,
                    median / 1000.
                );
            }
        }
        for t in self.topics.values() {
            let _ = write!(s, "\n{} · {}", t.name, t.metrics);
            t.metrics.describe_times(&mut s);
        }
        s
    }

    pub fn details(&self) -> String {
        let mut lines = Vec::new();
        for q in &self.questions {
            let state = if q.answer.is_none() {
                "Omessa"
            } else if q.correct() {
                "Corretta"
            } else {
                "Errata"
            };
            let answer = |v| match v {
                Some(true) => "Vero",
                Some(false) => "Falso",
                None => "omessa",
            };
            let mut line = format!(
                "{}. {} · {}\n{}\nRisposta {} · Soluzione {}",
                q.position + 1,
                q.topic,
                state,
                q.text,
                answer(q.answer),
                answer(Some(q.solution))
            );
            let ob = &q.observation;
            if let Some(ms) = ob.observed_ms {
                let _ = write!(
                    line,
                    "\nTempo osservato {:.1} s{}",
                    ms as f64 / 1000.,
                    if ob.incomplete { " (parziale)" } else { "" }
                );
            }
            if let Some(first) = ob.first_answer {
                let _ = write!(line, " · Prima risposta {}", answer(Some(first)));
            }
            if let Some(ms) = ob.first_answer_ms {
                let _ = write!(line, " · Prima scelta a {:.1} s", ms as f64 / 1000.);
            }
            if let Some(ms) = ob.final_answer_ms {
                let _ = write!(line, " · Scelta finale a {:.1} s", ms as f64 / 1000.);
            }
            if ob.skipped {
                line.push_str(" · Saltata durante la navigazione");
            }
            if ob.returns > 0 {
                let _ = write!(line, " · Ritorni {}", ob.returns);
            }
            lines.push(line);
        }
        lines.join("\n\n")
    }
}

/// Submit a paused session or read its frozen result. An active session must
/// use `store::command` with its foreground clock to close the final segment.
pub fn submit(conn: &mut Connection, id: &str) -> Result<Correction> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let active: bool = tx.query_row(
        "SELECT status='active' FROM exam_sessions WHERE session_id=?1",
        [id],
        |r| r.get(0),
    )?;
    ensure!(
        !active,
        "Per consegnare una prova attiva serve il suo cronometro"
    );
    finish(&tx, id, chrono::Utc::now().timestamp())?;
    let report = correction(&tx, id, false)?;
    tx.commit()?;
    Ok(report)
}

pub fn correction(conn: &Connection, id: &str, errors_only: bool) -> Result<Correction> {
    let json: String = conn.query_row(
        "SELECT result_json FROM exam_results WHERE session_id=?",
        [id],
        |r| r.get(0),
    )?;
    let mut c: Correction = serde_json::from_str(&json)?;
    if errors_only {
        c.questions.retain(|q| !q.correct());
    }
    Ok(c)
}

pub(crate) fn finish(tx: &Transaction<'_>, id: &str, now: i64) -> Result<()> {
    let (status, kind, threshold, active_ms, remaining_ms): (
        String,
        String,
        i64,
        i64,
        Option<i64>,
    ) = tx.query_row(
        "SELECT status,kind,threshold,active_ms,remaining_ms FROM exam_sessions WHERE session_id=?",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    if status == "submitted" {
        return Ok(());
    }
    ensure!(
        matches!(status.as_str(), "active" | "paused"),
        "La prova non è in corso"
    );
    let mut questions = tx
        .prepare(
            "SELECT q.position,q.question_id,q.topic_id,q.topic,q.question_text,
          q.correct_answer,a.answer,q.image_path,q.previously_seen
        FROM exam_session_questions q LEFT JOIN exam_answers a USING(session_id,question_id)
        WHERE q.session_id=? ORDER BY q.position",
        )?
        .query_map([id], |r| {
            Ok(Question {
                position: r.get::<_, u32>(0)? as usize,
                id: r.get(1)?,
                topic_id: r.get(2)?,
                topic: r.get(3)?,
                text: r.get(4)?,
                solution: r.get(5)?,
                answer: r.get(6)?,
                image_path: r.get(7)?,
                previously_seen: r.get(8)?,
                observation: Observation::default(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(!questions.is_empty(), "La prova non contiene domande");
    let pauses = observations(tx, id, &mut questions, now)?;
    let metrics = Metrics::from_questions(questions.iter());
    let topics = topics(questions.iter());
    let (correct, wrong, omitted) = (metrics.correct, metrics.wrong, metrics.omitted);
    let passed = (wrong + omitted) as i64 <= threshold;
    if kind == "custom" {
        for q in questions.iter().filter(|q| q.answer.is_some()) {
            let rating = if q.correct() { 3 } else { 1 };
            let applied = crate::review::apply_review(tx, &q.id, q.correct(), rating, now, 0.90)?;
            tx.execute(
                "INSERT INTO review_applications (session_id,question_id,review_log_id,rating)
                VALUES (?,?,?,?)",
                params![id, q.id, applied.log_id, rating],
            )?;
            store::event(
                tx,
                id,
                "ReviewApplied",
                now,
                serde_json::json!({"question_id":q.id,"rating":rating,
                "review_log_id":applied.log_id,"due_at":applied.due_at}),
            )?;
        }
    }
    let report = Correction {
        kind,
        threshold,
        correct,
        wrong,
        omitted,
        passed,
        active_ms,
        remaining_ms,
        questions,
        metrics,
        topics,
        pauses,
    };
    tx.execute(
        "INSERT INTO exam_results(session_id,correct,wrong,omitted,passed,result_json,submitted_at)
        VALUES (?,?,?,?,?,?,?)",
        params![
            id,
            correct as i64,
            wrong as i64,
            omitted as i64,
            passed,
            serde_json::to_string(&report)?,
            now
        ],
    )?;
    tx.execute(
        "UPDATE exam_sessions SET status='submitted',updated_at=?2 WHERE session_id=?1",
        params![id, now],
    )?;
    store::event(
        tx,
        id,
        "SessionSubmitted",
        now,
        serde_json::json!({"correct":correct,"wrong":wrong,"omitted":omitted,"passed":passed}),
    )?;
    Ok(())
}

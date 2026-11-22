use super::result::{Correction, Question};
use anyhow::{Result, ensure};
use chrono::{Local, TimeZone};
use fsrs::{FSRS, MemoryState};
use rusqlite::{Connection, named_params, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter, Write};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnswerEvent {
    pub active_ms: i64,
    pub answer: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Observation {
    pub first_answer: Option<bool>,
    pub first_answer_ms: Option<i64>,
    pub final_answer_ms: Option<i64>,
    pub observed_ms: Option<i64>,
    pub incomplete: bool,
    pub visits: usize,
    pub returns: usize,
    pub skipped: bool,
    pub beneficial_changes: usize,
    pub harmful_changes: usize,
    pub sequence: Vec<AnswerEvent>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Pauses {
    pub count: usize,
    pub completed_seconds: i64,
    pub incomplete: usize,
}

pub(crate) fn observations(
    db: &Connection,
    id: &str,
    questions: &mut [Question],
    end: i64,
) -> Result<Pauses> {
    let index: HashMap<String, usize> = questions
        .iter()
        .enumerate()
        .map(|(i, q)| (q.id.clone(), i))
        .collect();
    let mut stmt = db.prepare(
        "SELECT event_type,occurred_at,active_ms,payload_json
        FROM exam_events WHERE session_id=? AND event_version=1 ORDER BY sequence",
    )?;
    let mut rows = stmt.query([id])?;
    let mut pauses = Pauses::default();
    let mut paused_at = None;
    let mut previous: Option<usize> = None;
    let mut displaying: Option<(usize, i64)> = None;
    while let Some(r) = rows.next()? {
        let kind: String = r.get(0)?;
        let wall: i64 = r.get(1)?;
        let active: i64 = r.get(2)?;
        let payload: serde_json::Value = serde_json::from_str(&r.get::<_, String>(3)?)?;
        match kind.as_str() {
            "SessionPaused" => {
                if let Some((open, _)) = displaying.take() {
                    questions[open].observation.incomplete = true;
                }
                pauses.count += 1;
                paused_at = Some(wall);
                if payload["incomplete"] == true || payload["recovered"] == true {
                    pauses.incomplete += 1;
                }
                if payload["incomplete"] == true {
                    paused_at = None;
                }
                continue;
            }
            "SessionResumed" => {
                if let Some(start) = paused_at.take() {
                    pauses.completed_seconds += (wall - start).max(0);
                }
                continue;
            }
            _ => {}
        }
        let Some(i) = payload["question_id"]
            .as_str()
            .and_then(|q| index.get(q))
            .copied()
        else {
            continue;
        };
        match kind.as_str() {
            "QuestionDisplayed" => {
                if let Some((open, _)) = displaying {
                    questions[open].observation.incomplete = true;
                }
                if let Some(last) = previous.filter(|last| *last != i) {
                    if questions[last]
                        .observation
                        .sequence
                        .last()
                        .and_then(|a| a.answer)
                        .is_none()
                    {
                        questions[last].observation.skipped = true;
                    }
                    if questions[i].observation.visits > 0 {
                        questions[i].observation.returns += 1;
                    }
                }
                previous = Some(i);
                displaying = Some((i, active));
                questions[i].observation.visits += 1;
            }
            "QuestionHidden" => {
                match displaying.take() {
                    Some((qid, start)) if qid == i && start <= active => {
                        *questions[i].observation.observed_ms.get_or_insert(0) += active - start;
                    }
                    open => {
                        questions[i].observation.incomplete = true;
                        if let Some((qid, _)) = open {
                            questions[qid].observation.incomplete = true;
                        }
                    }
                }
                questions[i].observation.incomplete |= payload["incomplete"] == true;
            }
            "AnswerSelected" | "AnswerChanged" | "AnswerCleared" => {
                let q = &mut questions[i];
                let ob = &mut q.observation;
                let elapsed = displaying
                    .filter(|(qid, start)| *qid == i && *start <= active && !ob.incomplete)
                    .map(|(_, start)| ob.observed_ms.unwrap_or(0) + active - start);
                let answer = payload["answer"].as_bool();
                if let Some(value) = answer {
                    if ob.first_answer.is_none() {
                        ob.first_answer = Some(value);
                        ob.first_answer_ms = elapsed;
                    }
                    if let Some(old) = ob
                        .sequence
                        .last()
                        .and_then(|e| e.answer)
                        .filter(|v| *v != value)
                    {
                        if old == q.solution {
                            ob.harmful_changes += 1;
                        } else {
                            ob.beneficial_changes += 1;
                        }
                    }
                }
                ob.final_answer_ms = answer.and(elapsed);
                ob.sequence.push(AnswerEvent {
                    active_ms: active,
                    answer,
                });
            }
            _ => {}
        }
    }
    if let Some((open, _)) = displaying {
        questions[open].observation.incomplete = true;
    }
    if let Some(start) = paused_at {
        pauses.completed_seconds += (end - start).max(0);
    }
    Ok(pauses)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Timing {
    pub samples: usize,
    pub mean_ms: Option<f64>,
    pub median_ms: Option<f64>,
}

impl Timing {
    fn new(mut values: Vec<i64>) -> Self {
        let samples = values.len();
        if samples == 0 {
            return Self::default();
        }
        values.sort_unstable();
        let mean_ms = Some(values.iter().map(|&v| v as f64).sum::<f64>() / samples as f64);
        let median_ms = Some((values[(samples - 1) / 2] as f64 + values[samples / 2] as f64) / 2.);
        Self {
            samples,
            mean_ms,
            median_ms,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub correct: usize,
    pub wrong: usize,
    pub omitted: usize,
    pub previously_unseen: usize,
    pub skipped: usize,
    pub returns: usize,
    pub beneficial_changes: usize,
    pub harmful_changes: usize,
    pub incomplete: usize,
    pub timing: Timing,
    pub time_by_outcome: BTreeMap<String, Timing>,
}

impl Metrics {
    pub fn accuracy(&self) -> f64 {
        let n = self.correct + self.wrong + self.omitted;
        if n == 0 {
            0.
        } else {
            self.correct as f64 / n as f64
        }
    }

    pub(crate) fn from_questions<'a>(questions: impl Iterator<Item = &'a Question>) -> Self {
        let mut metrics = Self::default();
        let mut times = Vec::new();
        let mut groups: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        for q in questions {
            let outcome = match q.answer {
                None => {
                    metrics.omitted += 1;
                    "omesse"
                }
                Some(a) if a == q.solution => {
                    metrics.correct += 1;
                    "corrette"
                }
                Some(_) => {
                    metrics.wrong += 1;
                    "errate"
                }
            };
            let ob = &q.observation;
            metrics.previously_unseen += usize::from(!q.previously_seen);
            metrics.skipped += usize::from(ob.skipped);
            metrics.returns += ob.returns;
            metrics.beneficial_changes += ob.beneficial_changes;
            metrics.harmful_changes += ob.harmful_changes;
            metrics.incomplete += usize::from(ob.incomplete);
            if let Some(t) = ob.observed_ms.filter(|_| !ob.incomplete) {
                times.push(t);
                groups.entry(outcome.to_owned()).or_default().push(t);
            }
        }
        metrics.timing = Timing::new(times);
        metrics.time_by_outcome = groups
            .into_iter()
            .map(|(k, v)| (k, Timing::new(v)))
            .collect();
        metrics
    }

    pub(crate) fn describe_times(&self, out: &mut String) {
        if let (Some(mean), Some(median)) = (self.timing.mean_ms, self.timing.median_ms) {
            let _ = write!(
                out,
                "\nTempo per domanda · media {:.1} s · mediana {:.1} s ({} misurate)",
                mean / 1000.,
                median / 1000.,
                self.timing.samples
            );
        }
        if self.incomplete > 0 {
            let _ = write!(out, "\n{} domande con tempi parziali", self.incomplete);
        }
    }
}

impl Display for Metrics {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} corrette, {} errate, {} omesse · {:.0}%",
            self.correct,
            self.wrong,
            self.omitted,
            self.accuracy() * 100.
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Topic {
    pub name: String,
    pub metrics: Metrics,
}

pub(crate) fn topics<'a>(questions: impl Iterator<Item = &'a Question>) -> BTreeMap<i64, Topic> {
    let mut groups: BTreeMap<i64, Vec<&Question>> = BTreeMap::new();
    for q in questions {
        groups.entry(q.topic_id).or_default().push(q);
    }
    groups
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                Topic {
                    name: v[0].topic.clone(),
                    metrics: Metrics::from_questions(v.into_iter()),
                },
            )
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct Filters {
    pub license: Option<String>,
    pub procedure: Option<String>,
    pub bank_version: Option<String>,
    pub kind: Option<String>,
    pub strategy: Option<String>,
    pub topic_id: Option<i64>,
    /// UTC seconds, inclusive lower and exclusive upper bound for historical activity.
    pub from: Option<i64>,
    pub until: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub total: usize,
    pub seen: usize,
    pub studied: usize,
    pub due: usize,
    pub due_next_week: usize,
    pub due_later: usize,
    pub reviews: usize,
    pub repeat_reviews: usize,
    pub correct_repeats: usize,
    pub estimated_recall: Option<f64>,
}

/// Card state is shared by study, review and exams on the same bank. Periods
/// filter the review ledger. Coverage, due dates and estimates describe the
/// current card state, independent of the exam strategy and period.
fn memory(db: &Connection, f: &Filters, now: i64) -> Result<Memory> {
    let restrict = f.license.is_some() || f.procedure.is_some();
    let mut banks: Vec<String> = crate::catalog::load()?
        .listati
        .into_iter()
        .filter(|b| {
            f.license
                .as_ref()
                .is_none_or(|l| b.patenti.iter().any(|p| p.eq_ignore_ascii_case(l)))
                && f.procedure
                    .as_ref()
                    .is_none_or(|p| b.procedura.eq_ignore_ascii_case(p))
        })
        .map(|b| b.versione)
        .collect();
    if restrict {
        let mut previous=db.prepare("SELECT DISTINCT bank_version FROM exam_sessions WHERE (?1 IS NULL OR license=?1 COLLATE NOCASE) AND (?2 IS NULL OR procedure=?2 COLLATE NOCASE)")?;
        for bank in
            previous.query_map(params![f.license, f.procedure], |r| r.get::<_, String>(0))?
        {
            banks.push(bank?);
        }
    }
    let banks = serde_json::to_string(&banks)?;
    let scope = "(:version IS NULL OR q.source_version=:version)
        AND (:topic IS NULL OR q.topic_id=:topic)
        AND (NOT :restrict OR q.source_version IN (SELECT value FROM json_each(:banks)))";
    let mut stmt = db.prepare(&format!(
        "SELECT s.last_review_at,s.due_at,s.stability,s.stability_fast,s.difficulty,
        EXISTS(SELECT 1 FROM study_card_views v WHERE v.card_id=q.card_id) OR
        EXISTS(SELECT 1 FROM exam_events e WHERE e.event_type='QuestionDisplayed'
               AND json_extract(e.payload_json,'$.question_id')=q.card_id)
        FROM questions q LEFT JOIN card_state s ON s.card_id=q.card_id WHERE {scope}"
    ))?;
    let mut rows=stmt.query(named_params!{":version":f.bank_version,":topic":f.topic_id,":restrict":restrict,":banks":banks})?;
    let model = FSRS::default();
    let mut m = Memory::default();
    let mut recall = 0.;
    let mut estimates = 0;
    while let Some(r) = rows.next()? {
        m.total += 1;
        let last: Option<i64> = r.get(0)?;
        if r.get::<_, bool>(5)? || last.is_some() {
            m.seen += 1;
        }
        let Some(last) = last else {
            continue;
        };
        m.studied += 1;
        let due: i64 = r.get(1)?;
        if due <= now {
            m.due += 1;
        } else if due <= now.saturating_add(7 * 86400) {
            m.due_next_week += 1;
        } else {
            m.due_later += 1;
        }
        let state = MemoryState {
            stability: r.get(2)?,
            stability_fast: r.get(3)?,
            difficulty: r.get(4)?,
        };
        let value =
            model.current_retrievability(state, now.saturating_sub(last).max(0) as f32 / 86_400.);
        if value.is_finite() {
            recall += f64::from(value);
            estimates += 1;
        }
    }
    if estimates > 0 {
        m.estimated_recall = Some(recall / estimates as f64);
    }
    (m.reviews,m.repeat_reviews,m.correct_repeats)=db.query_row(&format!("WITH period AS (
        SELECT r.answer_correct, EXISTS(SELECT 1 FROM review_log earlier WHERE earlier.card_id=r.card_id
            AND (earlier.reviewed_at,earlier.id)<(r.reviewed_at,r.id)) AS repeated
        FROM review_log r JOIN questions q ON q.card_id=r.card_id
        LEFT JOIN review_applications a ON a.review_log_id=r.id
        LEFT JOIN exam_sessions s ON s.session_id=a.session_id
        LEFT JOIN exam_session_questions snapshot ON snapshot.session_id=a.session_id AND snapshot.question_id=a.question_id
        WHERE ((a.session_id IS NULL AND {scope}) OR
            (a.session_id IS NOT NULL AND (:license IS NULL OR s.license=:license COLLATE NOCASE)
            AND (:procedure IS NULL OR s.procedure=:procedure COLLATE NOCASE)
            AND (:version IS NULL OR s.bank_version=:version) AND (:topic IS NULL OR snapshot.topic_id=:topic)))
        AND (:from IS NULL OR r.reviewed_at>=:from) AND (:until IS NULL OR r.reviewed_at<:until))
        SELECT count(*),coalesce(sum(repeated),0),coalesce(sum(repeated*answer_correct),0) FROM period"),
        named_params!{":version":f.bank_version,":topic":f.topic_id,":restrict":restrict,":banks":banks,
            ":from":f.from,":until":f.until,":license":f.license,":procedure":f.procedure},
        |r|Ok((r.get::<_,i64>(0)? as usize,r.get::<_,i64>(1)? as usize,r.get::<_,i64>(2)? as usize)))?;
    Ok(m)
}

#[derive(Clone, Debug, Ord, PartialOrd, PartialEq, Eq)]
pub struct SeriesKey {
    pub license: String,
    pub procedure: String,
    pub bank_version: String,
    pub kind: String,
    pub strategy: String,
}

#[derive(Clone, Debug)]
pub struct Attempt {
    pub id: String,
    pub at: i64,
    pub passed: bool,
    pub session_active_ms: i64,
    pub pauses: Pauses,
    pub metrics: Metrics,
    pub topics: BTreeMap<i64, Topic>,
}

#[derive(Clone, Debug)]
pub struct Series {
    pub key: SeriesKey,
    pub history: Vec<Attempt>,
    pub cancelled: usize,
    pub topics: BTreeMap<i64, Topic>,
    pub metrics: Metrics,
}

#[derive(Clone, Debug)]
pub struct Dashboard {
    pub memory: Memory,
    pub series: Vec<Series>,
}

#[derive(Default)]
struct SessionGroup {
    cancelled: usize,
    reports: Vec<(String, i64, Correction)>,
}

pub fn dashboard(conn: &Connection, f: &Filters, now: i64) -> Result<Dashboard> {
    ensure!(
        !matches!((f.from,f.until),(Some(a),Some(b)) if a>=b),
        "Il periodo selezionato non è valido"
    );
    // All dashboard queries see the same committed version during concurrent submissions.
    let db = conn.unchecked_transaction()?;
    let mut groups: BTreeMap<SeriesKey, SessionGroup> = BTreeMap::new();
    {
        let mut stmt = db.prepare(
            "SELECT s.session_id,s.license,s.procedure,s.bank_version,s.kind,
            s.strategy,s.status,s.updated_at,r.result_json
            FROM exam_sessions s LEFT JOIN exam_results r USING(session_id)
            WHERE s.status IN ('cancelled','submitted')
            AND (:license IS NULL OR s.license=:license COLLATE NOCASE)
            AND (:procedure IS NULL OR s.procedure=:procedure COLLATE NOCASE)
            AND (:version IS NULL OR s.bank_version=:version)
            AND (:kind IS NULL OR s.kind=:kind) AND (:strategy IS NULL OR s.strategy=:strategy)
            AND (:from IS NULL OR s.updated_at>=:from) AND (:until IS NULL OR s.updated_at<:until)
            AND (:topic IS NULL OR EXISTS(SELECT 1 FROM exam_session_questions q
                WHERE q.session_id=s.session_id AND q.topic_id=:topic))
            ORDER BY s.updated_at,s.session_id",
        )?;
        let mut rows=stmt.query(named_params!{":license":f.license,":procedure":f.procedure,":version":f.bank_version,
            ":kind":f.kind,":strategy":f.strategy,":topic":f.topic_id,":from":f.from,":until":f.until})?;
        while let Some(r) = rows.next()? {
            let key = SeriesKey {
                license: r.get(1)?,
                procedure: r.get(2)?,
                bank_version: r.get(3)?,
                kind: r.get(4)?,
                strategy: r.get(5)?,
            };
            let group = groups.entry(key).or_default();
            if r.get::<_, String>(6)? == "cancelled" {
                group.cancelled += 1;
                continue;
            }
            let mut c: Correction = serde_json::from_str(&r.get::<_, String>(8)?)?;
            if let Some(topic) = f.topic_id {
                c.questions.retain(|q| q.topic_id == topic);
            }
            group.reports.push((r.get(0)?, r.get(7)?, c));
        }
    }
    let mut series = Vec::new();
    for (key, SessionGroup { cancelled, reports }) in groups {
        let questions = reports.iter().flat_map(|(_, _, c)| c.questions.iter());
        let metrics = Metrics::from_questions(questions.clone());
        let topics_by_id = topics(questions);
        let history = reports
            .into_iter()
            .map(|(id, at, c)| Attempt {
                id,
                at,
                passed: c.passed,
                session_active_ms: c.active_ms,
                pauses: c.pauses,
                metrics: Metrics::from_questions(c.questions.iter()),
                topics: topics(c.questions.iter()),
            })
            .collect();
        series.push(Series {
            key,
            history,
            cancelled,
            metrics,
            topics: topics_by_id,
        });
    }
    let memory = memory(&db, f, now)?;
    db.commit()?;
    Ok(Dashboard { series, memory })
}

impl Dashboard {
    pub fn summary(&self) -> String {
        let m = &self.memory;
        let mut out = format!(
            "Copertura attuale della banca · {} viste su {} · {} studiate\nRipassi in scadenza · {} già dovuti · {} nei prossimi 7 giorni · {} successivi\n{} risposte registrate nel periodo da studio, ripasso e prove personalizzate",
            m.seen, m.total, m.studied, m.due, m.due_next_week, m.due_later, m.reviews
        );
        if m.repeat_reviews > 0 {
            let _ = write!(
                out,
                " · {:.0}% corrette su {} ripetizioni",
                100. * m.correct_repeats as f64 / m.repeat_reviews as f64,
                m.repeat_reviews
            );
        }
        if let Some(r) = m.estimated_recall {
            let _ = write!(
                out,
                "\nRicordo stimato sulle {} domande studiate · {:.0}%",
                m.studied,
                r * 100.
            );
        }
        if self.series.is_empty() {
            out.push_str("\n\nNessuna prova conclusa o annullata nel periodo.");
        }
        for s in &self.series {
            let label = if s.key.kind == "official" {
                "Ufficiale"
            } else {
                match s.key.strategy.as_str() {
                    "balanced" => "Personalizzata equilibrata",
                    "random" => "Personalizzata casuale",
                    "intelligent" => "Personalizzata intelligente",
                    _ => "Personalizzata",
                }
            };
            let _ = write!(
                out,
                "\n\n{} · {} · {} · {}\n{} concluse · {} superate · {} annullate",
                label,
                s.key.license,
                s.key.procedure,
                s.key.bank_version,
                s.history.len(),
                s.history.iter().filter(|a| a.passed).count(),
                s.cancelled
            );
            if s.history.is_empty() {
                continue;
            }
            let _ = write!(out, "\n{}", s.metrics);
            s.metrics.describe_times(&mut out);
            let mut ranked: Vec<_> = s.topics.values().collect();
            ranked.sort_by(|a, b| {
                a.metrics
                    .accuracy()
                    .total_cmp(&b.metrics.accuracy())
                    .then(a.name.cmp(&b.name))
            });
            for t in ranked {
                let _ = write!(out, "\n{} · {}", t.name, t.metrics);
            }
            out.push_str("\nAndamento recente");
            for a in s.history.iter().rev().take(8) {
                let date = Local
                    .timestamp_opt(a.at, 0)
                    .single()
                    .map(|d| d.format("%d/%m/%Y").to_string())
                    .unwrap_or_default();
                let _ = write!(
                    out,
                    "\n{} · {} errate, {} omesse · prova {:.1} s",
                    date,
                    a.metrics.wrong,
                    a.metrics.omitted,
                    a.session_active_ms as f64 / 1000.
                );
            }
        }
        out
    }
}

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

mod banks;
mod profile;

use crate::{
    catalog,
    exam::{
        catalog::verified_rule,
        clock::ActiveClock,
        result,
        selection::{Strategy, scaled_threshold},
        statistics,
        store::{self, Action, Config, SessionView},
    },
    review::{self, StudyCard},
    storage,
};

slint::slint! {
    #[edition = "1.19"]
    export {AppWindow, TopicItem, Entry} from "../ui/app.slint";
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(items)).into()
}

fn now() -> i64 {
    Utc::now().timestamp()
}
fn truth(value: bool) -> &'static str {
    if value { "Vero" } else { "Falso" }
}

fn duration(ms: i64) -> String {
    let seconds = (ms.max(0) + 999) / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn date_bound(text: &str, end: bool) -> Result<Option<i64>> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let date = NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
        .context("Inserisci la data nel formato AAAA-MM-GG")?;
    let day = if end {
        date.succ_opt().context("Data finale non valida")?
    } else {
        date
    };
    Ok(Some(
        Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .context("La mezzanotte selezionata non esiste nel fuso locale")?
            .timestamp(),
    ))
}

const QUICK_STUDY_LIMIT: usize = 20;

struct StudyItem {
    card: StudyCard,
    answer: Option<bool>,
    rating: Option<u8>,
}

impl StudyItem {
    fn new(card: StudyCard) -> Self {
        Self {
            card,
            answer: None,
            rating: None,
        }
    }
}

struct ContinuousStudy {
    topics: Vec<i64>,
    reviews_only: bool,
}

fn no_scheduled_cards(reviews_only: bool) -> &'static str {
    if reviews_only {
        "Nessun ripasso in scadenza per gli argomenti selezionati."
    } else {
        "Hai studiato tutte le domande di questi argomenti. Potrai continuare al prossimo ripasso."
    }
}

struct Controller {
    db: Connection,
    pending_import: Option<banks::ImportSelection>,
    pending_archive: Option<Vec<banks::ArchivedEdition>>,
    _instance: std::fs::File,
    #[cfg(any(target_os = "android", test))]
    staged_media: PathBuf,
    topic_ids: Vec<i64>,
    version: String,
    dashboard_day: Option<NaiveDate>,
    next_due_at: Option<i64>,
    quick_study_count: usize,
    session: Option<SessionView>,
    clock: Option<ActiveClock>,
    pending_pause: Option<(i64, i64)>,
    study: Vec<StudyItem>,
    continuous: Option<ContinuousStudy>,
    study_index: usize,
    study_furthest: usize,
}

impl Controller {
    fn open(path: &Path) -> Result<Self> {
        let mut db = Connection::open(path)?;
        // Recovery may close interrupted segments only when no GUI owns the clock.
        let mut lock_path = path.canonicalize()?.into_os_string();
        lock_path.push(".gui-lock");
        let instance = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        instance
            .try_lock()
            .context("Questo database è già aperto in un'altra finestra ZeroPatenti")?;
        db.busy_timeout(Duration::from_secs(5))?;
        storage::init_database(&db)?;
        banks::migrate(&db)?;
        profile::initialize(&db)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS gui_media_roots(bank_version TEXT PRIMARY KEY, directory TEXT NOT NULL);")?;
        store::recover(&mut db, now())?;
        Ok(Self {
            db,
            pending_import: None,
            pending_archive: None,
            _instance: instance,
            #[cfg(any(target_os = "android", test))]
            staged_media: path.with_file_name("bank-media"),
            topic_ids: vec![],
            version: String::new(),
            dashboard_day: None,
            next_due_at: None,
            quick_study_count: QUICK_STUDY_LIMIT,
            session: None,
            clock: None,
            pending_pause: None,
            study: Vec::new(),
            continuous: None,
            study_index: 0,
            study_furthest: 0,
        })
    }

    fn refresh_dashboard_if_needed(
        &mut self,
        ui: &AppWindow,
        current: DateTime<Local>,
    ) -> Result<()> {
        if self.dashboard_day != Some(current.date_naive())
            || self
                .next_due_at
                .is_some_and(|due| due <= current.timestamp())
        {
            self.refresh_dashboard(ui, current)?;
        }
        Ok(())
    }

    fn refresh_dashboard(&mut self, ui: &AppWindow, current: DateTime<Local>) -> Result<()> {
        let today = current.date_naive();
        let exam = profile::exam_date(&self.db, &ui.get_license(), &ui.get_procedure())?;
        let mut filters = statistics::Filters {
            bank_version: Some(self.version.clone()),
            ..Default::default()
        };
        let scheduled = exam.filter(|date| *date >= today);
        if scheduled.is_some() {
            filters.from = date_bound(&today.to_string(), false)?;
            filters.until = Some(current.timestamp() + 1);
        }
        let progress = statistics::memory(&self.db, &filters, current.timestamp())?;
        let new = progress.total - progress.studied;
        let (recommended, goal) = if let Some(date) = scheduled {
            let days = (date - today).num_days().max(1) as usize;
            // Rebuild the morning's unseen pool so the goal stays stable as cards are graded.
            let completed = progress.reviews - progress.repeat_reviews;
            let target = (new + completed).div_ceil(days);
            let remaining = target.saturating_sub(completed).min(new);
            self.quick_study_count = progress.due + remaining;
            let goal = if progress.total == 0 {
                "Importa il listato per calcolare l’obiettivo giornaliero.".to_owned()
            } else if target == 0 {
                "Hai già studiato tutte le domande del listato.".to_owned()
            } else if completed > target {
                format!("Obiettivo raggiunto · {completed} nuove domande completate oggi.")
            } else {
                format!("Obiettivo di oggi · {completed} su {target} nuove domande.")
            };
            (remaining, goal)
        } else {
            self.quick_study_count = QUICK_STUDY_LIMIT;
            (
                new.min(QUICK_STUDY_LIMIT.saturating_sub(progress.due)),
                String::new(),
            )
        };
        let date_text = exam
            .map(|date| date.format("%d/%m/%Y").to_string())
            .unwrap_or_default();
        ui.set_exam_date_input(date_text.as_str().into());
        ui.set_exam_date(date_text.into());
        ui.set_exam_countdown(
            exam.map(|date| profile::countdown(date, today))
                .unwrap_or_default()
                .into(),
        );
        ui.set_study_goal(goal.into());
        ui.set_total(i32::try_from(progress.total)?);
        ui.set_studied(i32::try_from(progress.seen)?);
        ui.set_due(i32::try_from(progress.due)?);
        ui.set_recommended(i32::try_from(recommended)?);
        ui.set_memory(
            progress
                .estimated_recall
                .map_or(-1, |p| (100.0 * p).round() as i32),
        );
        self.dashboard_day = Some(today);
        self.next_due_at = progress.next_due_at;
        Ok(())
    }

    fn bank(&mut self, ui: &AppWindow) -> Result<()> {
        self.version = catalog::version_for(&ui.get_license(), &ui.get_procedure())?;
        let topics = storage::topic_counts(&self.db, &self.version)?;
        self.refresh_dashboard(ui, Local::now())?;
        let total = ui.get_total();
        ui.set_active_version(self.version.as_str().into());
        ui.set_bank(format!("{} · {total} domande", self.version).into());
        let mut entries = Vec::new();
        let mut counts = self
            .db
            .prepare("SELECT COUNT(*) FROM questions WHERE source_version=?")?;
        for edition in catalog::load()?.listati.into_iter().filter(|e| e.pronto) {
            let count: i64 = counts.query_row([&edition.versione], |r| r.get(0))?;
            let status = if count == 0 {
                "Da importare".to_owned()
            } else {
                format!("{count} domande locali")
            };
            entries.push(Entry {
                label: format!("{} · {status}", edition.nome).into(),
                key: edition.versione.into(),
            });
        }
        ui.set_bank_list(model(entries));
        self.topic_ids = topics.iter().map(|t| t.id).collect();
        ui.set_selected_count(topics.len() as i32);
        ui.set_topics(model(
            topics
                .iter()
                .map(|t| TopicItem {
                    label: t.name.as_str().into(),
                    summary: format!(
                        "{} domande · {} studiate · {} da ripassare",
                        t.total, t.studied, t.due
                    )
                    .into(),
                    chosen: true,
                })
                .collect(),
        ));
        let root: Option<String> = self
            .db
            .query_row(
                "SELECT directory FROM gui_media_roots WHERE bank_version=?1",
                [&self.version],
                |r| r.get(0),
            )
            .optional()?;
        ui.set_asset_root(root.unwrap_or_default().into());
        self.rule(ui);
        self.refresh_sessions(ui)
    }

    fn refresh_sessions(&self, ui: &AppWindow) -> Result<()> {
        let sessions = store::unfinished(&self.db)?
            .into_iter()
            .filter(|s| {
                s.license == ui.get_license().as_str() && s.procedure == ui.get_procedure().as_str()
            })
            .map(|s| Entry {
                label: format!(
                    "Patente {} · {} · {}/{} risposte · {}",
                    s.license,
                    if s.kind == "official" {
                        "Ufficiale"
                    } else {
                        "Personalizzata"
                    },
                    s.questions.iter().filter(|q| q.answer.is_some()).count(),
                    s.questions.len(),
                    clock_text(s.active_ms, s.remaining_ms)
                )
                .into(),
                key: s.id.into(),
            })
            .collect();
        ui.set_sessions(model(sessions));
        Ok(())
    }

    fn rule(&self, ui: &AppWindow) {
        ui.set_official_ready(false);
        ui.set_official_count(0);
        ui.set_official_minutes(0);
        let hint = (|| -> Result<String> {
            let rule = verified_rule(&self.db, &ui.get_license(), &ui.get_procedure())?;
            ui.set_official_count(i32::try_from(rule.question_count)?);
            ui.set_official_minutes(i32::try_from(rule.duration_seconds / 60)?);
            let official = crate::exam::catalog::official_quotas(&rule.bank_version).is_some();
            ui.set_official_ready(official);
            if ui.get_mode() == "Ufficiale" && !official {
                bail!(
                    "Distribuzione ufficiale dei quesiti non verificata per questa edizione. È disponibile la prova personalizzata."
                );
            }
            let count = if ui.get_mode() == "Ufficiale" {
                rule.question_count
            } else {
                ui.get_count() as usize
            };
            let threshold = scaled_threshold(count, rule.question_count, rule.max_errors)?;
            Ok(format!(
                "Massimo {threshold} errori su {count} domande · tempo ufficiale {} min",
                rule.duration_seconds / 60
            ))
        })();
        ui.set_rule_note(hint.unwrap_or_else(|e| e.to_string()).into());
    }

    fn selected(&self, ui: &AppWindow) -> Vec<i64> {
        self.topic_ids
            .iter()
            .copied()
            .zip(ui.get_topics().iter())
            .filter_map(|(id, t)| t.chosen.then_some(id))
            .collect()
    }

    fn start(&mut self, ui: &AppWindow) -> Result<()> {
        let topics = self.selected(ui);
        let count = ui.get_count() as usize;
        let mode = ui.get_mode();
        if mode == "Studio" || mode == "Ripasso" {
            let limit = (!ui.get_unlimited_study()).then_some(count);
            return self.start_study(ui, topics, limit, mode == "Ripasso");
        }
        let config = if mode == "Ufficiale" {
            Config::Official
        } else {
            let strategy = match ui.get_strategy().as_str() {
                "Casuale" => Strategy::Random,
                "Intelligente" => Strategy::Intelligent,
                _ => Strategy::Balanced,
            };
            Config::Custom {
                topics,
                count,
                strategy,
                timed: ui.get_timed(),
            }
        };
        let seed: i64 = self.db.query_row("SELECT random()", [], |row| row.get(0))?;
        let id = store::create(
            &mut self.db,
            &ui.get_license(),
            &ui.get_procedure(),
            &config,
            seed as u64,
            now(),
        )?;
        self.session = Some(store::view(&self.db, &id)?);
        self.exam(ui, Action::Start)
    }

    fn start_study(
        &mut self,
        ui: &AppWindow,
        topics: Vec<i64>,
        limit: Option<usize>,
        reviews_only: bool,
    ) -> Result<()> {
        ensure!(!topics.is_empty(), "Seleziona almeno un argomento");
        let cards = review::due_cards(
            &self.db,
            &self.version,
            limit.unwrap_or(1),
            &topics,
            reviews_only,
        )?;
        ensure!(!cards.is_empty(), "{}", no_scheduled_cards(reviews_only));
        self.continuous = limit.is_none().then_some(ContinuousStudy {
            topics,
            reviews_only,
        });
        ui.set_study_continuous(self.continuous.is_some());
        self.study = cards.into_iter().map(StudyItem::new).collect();
        self.study_index = 0;
        self.study_furthest = 0;
        ui.set_study_total(i32::try_from(self.study.len())?);
        ui.set_page("study".into());
        self.show_card(ui)
    }

    fn figure(&self, ui: &AppWindow, version: &str, relative: Option<&str>) {
        ui.set_has_figure(false);
        let Some(relative) = relative else { return };
        match self.load_figure(version, relative) {
            Ok(image) => {
                ui.set_figure(image);
                ui.set_has_figure(true);
            }
            Err(error) => ui.set_notice(format!("Figura non disponibile. {error}").into()),
        }
    }

    fn show_card(&self, ui: &AppWindow) -> Result<()> {
        let item = self
            .study
            .get(self.study_index)
            .context("Nessuna domanda aperta")?;
        storage::mark_seen(&self.db, &item.card.id, now())?;
        ui.set_study_number(i32::try_from(self.study_index + 1)?);
        ui.set_title_text(ui.get_mode());
        ui.set_subtitle(item.card.topic.as_str().into());
        ui.set_prompt(item.card.text.as_str().into());
        self.figure(ui, &self.version, item.card.figure.as_deref());
        let stage = if self.study_index < self.study_furthest {
            "history"
        } else if item.rating.is_some() {
            "next"
        } else if item.answer.is_some() {
            "rate"
        } else {
            "answer"
        };
        ui.set_study_stage(stage.into());
        ui.set_answer(item.answer.map(truth).unwrap_or("").into());
        ui.set_study_solution(
            item.answer
                .map(|_| truth(item.card.correct))
                .unwrap_or("")
                .into(),
        );
        ui.set_study_correct(item.answer == Some(item.card.correct));
        ui.set_study_rating(i32::from(item.rating.unwrap_or(0)));
        if item.answer.is_some() {
            let help = review::study_help(&self.db, &item.card)?;
            ui.set_study_explanation(help.brief.into());
            ui.set_study_explanation_source(help.brief_source.into());
            ui.set_study_detail(help.detail.into());
            ui.set_study_detail_source(help.detail_source.into());
        } else {
            ui.set_study_explanation("".into());
            ui.set_study_explanation_source("".into());
            ui.set_study_detail("".into());
            ui.set_study_detail_source("".into());
        }
        ui.set_details(match item.answer {
            None => "".into(),
            Some(value) if value == item.card.correct => "Risposta corretta.".into(),
            Some(_) => format!("Risposta errata. Soluzione: {}.", truth(item.card.correct)).into(),
        });
        Ok(())
    }

    fn rate(&mut self, ui: &AppWindow, rating: u8) -> Result<()> {
        let item = self
            .study
            .get_mut(self.study_index)
            .context("Nessuna domanda aperta")?;
        let correct = item.answer.context("Rispondi alla domanda")? == item.card.correct;
        ensure!(
            item.rating.is_none()
                && self.study_index == self.study_furthest
                && (correct || rating == 1),
            "Valutazione non disponibile"
        );
        let tx = self.db.transaction()?;
        review::apply_review(&tx, &item.card.id, correct, rating, now(), 0.90)?;
        tx.commit()?;
        item.rating = Some(rating);
        self.show_card(ui)
    }

    fn answer_study(&mut self, ui: &AppWindow, value: bool) -> Result<()> {
        let item = self
            .study
            .get_mut(self.study_index)
            .context("Nessuna domanda aperta")?;
        ensure!(item.answer.is_none(), "Risposta già registrata");
        if value != item.card.correct {
            let tx = self.db.transaction()?;
            review::apply_review(&tx, &item.card.id, false, 1, now(), 0.90)?;
            tx.commit()?;
            item.rating = Some(1);
        }
        item.answer = Some(value);
        self.show_card(ui)
    }

    fn advance_study(&mut self, ui: &AppWindow) -> Result<()> {
        ensure!(
            self.study
                .get(self.study_index)
                .is_some_and(|item| item.rating.is_some()),
            "Rispondi e valuta la domanda prima di proseguire"
        );
        if self.study_index + 1 == self.study.len() {
            let Some(session) = &self.continuous else {
                self.home(ui)?;
                ui.set_notice("Sessione completata.".into());
                return Ok(());
            };
            let next = review::due_cards(
                &self.db,
                &self.version,
                1,
                &session.topics,
                session.reviews_only,
            )?;
            let Some(card) = next.into_iter().next() else {
                let message = no_scheduled_cards(session.reviews_only);
                self.home(ui)?;
                ui.set_notice(message.into());
                return Ok(());
            };
            self.study.push(StudyItem::new(card));
        }
        self.study_index += 1;
        self.study_furthest = self.study_furthest.max(self.study_index);
        self.show_card(ui)
    }

    fn exam(&mut self, ui: &AppWindow, action: Action) -> Result<()> {
        let session = self.session.as_ref().context("Nessuna prova aperta")?;
        let timestamp = now();
        if matches!(action, Action::Pause)
            && let Some(clock) = self.clock.take()
        {
            self.pending_pause = Some((clock.active_ms(), timestamp));
        }
        let (action, elapsed, at) = match self.pending_pause {
            Some((elapsed, at)) => (Action::Pause, elapsed, at),
            None => (
                action,
                self.clock
                    .as_ref()
                    .map_or(session.active_ms, ActiveClock::active_ms),
                timestamp,
            ),
        };
        let view = store::command(&mut self.db, &session.id, action, elapsed, at)?;
        self.pending_pause = None;
        if view.status == "active" {
            if self.clock.is_none() {
                self.clock = Some(ActiveClock::new(view.active_ms));
            }
        } else {
            self.clock = None;
        }
        self.session = Some(view);
        self.show_exam(ui)
    }

    fn tick(&mut self, ui: &AppWindow) -> Result<()> {
        if self.pending_pause.is_some() {
            return self.exam(ui, Action::Pause);
        }
        let (Some(s), Some(clock)) = (self.session.as_mut(), self.clock.as_ref()) else {
            return Ok(());
        };
        let (status, elapsed, left) = store::tick(&mut self.db, &s.id, clock.active_ms(), now())?;
        if status == "active" {
            s.active_ms = elapsed;
            s.remaining_ms = left;
            ui.set_clock_text(clock_text(elapsed, left).into());
            return Ok(());
        }
        self.clock = None;
        self.session = Some(store::view(&self.db, &s.id)?);
        self.show_exam(ui)
    }

    fn show_exam(&mut self, ui: &AppWindow) -> Result<()> {
        let s = self.session.as_ref().context("Nessuna prova aperta")?;
        match s.status.as_str() {
            "submitted" => return self.show_result(ui, &s.id),
            "cancelled" => return self.home(ui),
            "active" => ui.set_page("exam".into()),
            _ => ui.set_page("paused".into()),
        }
        ui.set_position(s.index as i32);
        let question = s
            .questions
            .get(s.index)
            .context("Quesito non disponibile")?;
        ui.set_subtitle(
            format!(
                "{} · {} · {} di {} risposte",
                s.license,
                question.topic,
                s.questions.iter().filter(|q| q.answer.is_some()).count(),
                s.questions.len()
            )
            .into(),
        );
        ui.set_clock_text(clock_text(s.active_ms, s.remaining_ms).into());
        ui.set_title_text(format!("Domanda {} di {}", s.index + 1, s.questions.len()).into());
        ui.set_prompt(question.text.as_str().into());
        ui.set_answer(question.answer.map(truth).unwrap_or("").into());
        let page_size = ui.get_grid_page_size() as usize;
        let start = s.index / page_size * page_size;
        let end = (start + page_size).min(s.questions.len());
        ui.set_grid_start(start as i32);
        ui.set_question_count(s.questions.len() as i32);
        ui.set_grid(model(
            s.questions[start..end]
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    format!(
                        "{}{}",
                        start + i + 1,
                        if q.answer.is_some() { " ✓" } else { "" }
                    )
                    .into()
                })
                .collect(),
        ));
        self.figure(ui, &s.bank_version, question.image_path.as_deref());
        Ok(())
    }

    fn show_result(&self, ui: &AppWindow, id: &str) -> Result<()> {
        let correction = result::correction(&self.db, id, ui.get_errors_only())?;
        ui.set_page("result".into());
        ui.set_title_text("Risultato della prova".into());
        ui.set_subtitle(correction.summary().into());
        ui.set_details(correction.details().into());
        Ok(())
    }

    fn stats(&self, ui: &AppWindow) -> Result<()> {
        let license = ui.get_license();
        let procedure = ui.get_procedure();
        let mut editions = vec![SharedString::from(self.version.as_str())];
        let mut stmt=self.db.prepare("SELECT DISTINCT bank_version FROM exam_sessions WHERE license=? AND procedure=? AND bank_version<>? ORDER BY bank_version")?;
        for version in stmt.query_map(
            params![license.as_str(), procedure.as_str(), self.version],
            |row| row.get::<_, String>(0),
        )? {
            editions.push(version?.into());
        }
        let selected_version = ui.get_stats_bank();
        ensure!(
            editions.contains(&selected_version),
            "Edizione non disponibile per questa patente"
        );
        ui.set_stats_banks(model(editions));
        ui.set_stats_bank(selected_version.clone());
        let mut stmt=self.db.prepare("SELECT topic_id, MIN(topic) FROM (
            SELECT topic_id,topic FROM questions WHERE source_version=?1
            UNION ALL SELECT q.topic_id,q.topic FROM exam_session_questions q JOIN exam_sessions s USING(session_id)
            WHERE s.bank_version=?1 AND s.license=?2 AND s.procedure=?3
        ) GROUP BY topic_id ORDER BY topic_id")?;
        let mut choices = vec!["Tutti".into()];
        let mut topic_id = None;
        let selected = ui.get_stats_topic();
        for row in stmt.query_map(
            params![
                selected_version.as_str(),
                license.as_str(),
                procedure.as_str()
            ],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )? {
            let (id, name) = row?;
            let label = SharedString::from(format!("{id} · {name}"));
            if label == selected {
                topic_id = Some(id);
            }
            choices.push(label);
        }
        ui.set_stats_topics(model(choices));
        ui.set_stats_topic(if topic_id.is_some() {
            selected
        } else {
            "Tutti".into()
        });
        let series = ui.get_stats_series();
        let strategy = match series.as_str() {
            "Casuale" => Some("random"),
            "Equilibrata" => Some("balanced"),
            "Intelligente" => Some("intelligent"),
            _ => None,
        };
        let filters = statistics::Filters {
            license: Some(ui.get_license().to_string()),
            procedure: Some(ui.get_procedure().to_string()),
            bank_version: Some(selected_version.to_string()),
            kind: match series.as_str() {
                "Tutte" => None,
                "Ufficiale" => Some("official".into()),
                _ => Some("custom".into()),
            },
            strategy: strategy.map(String::from),
            topic_id,
            from: date_bound(&ui.get_since(), false)?,
            until: date_bound(&ui.get_until(), true)?,
        };
        ensure!(
            filters.from.zip(filters.until).is_none_or(|(a, b)| a < b),
            "L'intervallo di date non è valido"
        );
        let dash = statistics::dashboard(&self.db, &filters, now())?;
        ui.set_stats_overview(dash.overview().into());
        ui.set_details(dash.breakdown().into());
        let mut history = Vec::new();
        for series in &dash.series {
            for attempt in &series.history {
                let date = Local
                    .timestamp_opt(attempt.at, 0)
                    .single()
                    .map(|date| date.format("%d/%m/%Y %H:%M").to_string())
                    .unwrap_or_default();
                let mode = series.key.label();
                let scope = if topic_id.is_some() {
                    " (argomento selezionato)"
                } else {
                    ""
                };
                let metrics = &attempt.metrics;
                history.push((
                    attempt.at,
                    Entry {
                        key: attempt.id.as_str().into(),
                        label: format!(
                            "{date} · {mode}{scope} · {} corrette, {} errate, {} omesse",
                            metrics.correct, metrics.wrong, metrics.omitted
                        )
                        .into(),
                    },
                ));
            }
        }
        history.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.key.cmp(&a.1.key)));
        ui.set_history(model(history.into_iter().map(|(_, entry)| entry).collect()));
        ui.set_page("stats".into());
        Ok(())
    }

    fn save_media(db: &Connection, version: &str, path: &str) -> Result<()> {
        ensure!(
            path.is_empty() || Path::new(path).is_dir(),
            "Cartella immagini inesistente"
        );
        db.execute("INSERT INTO gui_media_roots VALUES (?1,?2) ON CONFLICT(bank_version) DO UPDATE SET directory=excluded.directory",params![version, path])?;
        Ok(())
    }

    #[cfg(any(target_os = "android", test))]
    fn discard_unused_media(&self) -> Result<()> {
        self.remove_unused_media(None)
    }

    #[cfg(any(target_os = "android", test))]
    fn remove_unused_media(
        &self,
        candidates: Option<&std::collections::BTreeSet<PathBuf>>,
    ) -> Result<()> {
        if candidates.is_some_and(std::collections::BTreeSet::is_empty)
            || !self.staged_media.is_dir()
        {
            return Ok(());
        }
        ensure!(
            !banks::is_link(&std::fs::symlink_metadata(&self.staged_media)?),
            "Cartella privata delle immagini non valida"
        );
        let parent = self.staged_media.canonicalize()?;
        let mut roots = Vec::new();
        let mut stmt = self
            .db
            .prepare("SELECT directory FROM gui_media_roots WHERE directory <> ''")?;
        for root in stmt.query_map([], |row| row.get::<_, String>(0))? {
            let path = PathBuf::from(root?);
            match path.canonicalize() {
                Ok(root) => roots.push(root),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            }
        }
        for item in std::fs::read_dir(&self.staged_media)? {
            let item = item?;
            if !item.file_type()?.is_dir()
                || !item.file_name().to_string_lossy().starts_with("listato-")
            {
                continue;
            }
            let dir = item.path().canonicalize()?;
            if dir.parent() != Some(parent.as_path())
                || banks::is_link(&std::fs::symlink_metadata(item.path())?)
            {
                continue;
            }
            if candidates.is_some_and(|paths| !paths.contains(&dir)) {
                continue;
            }
            if !roots
                .iter()
                .any(|root| root.starts_with(&dir) || dir.starts_with(root))
            {
                std::fs::remove_dir_all(dir)?;
            }
        }
        Ok(())
    }

    #[cfg(target_os = "android")]
    fn receive_bank(&self, ui: &AppWindow, file: &Path) -> Result<()> {
        // Accept only the direct temporary bank files created by BankActivity.
        let inbox = self
            .staged_media
            .parent()
            .and_then(Path::parent)
            .context("Cartella privata mancante")?
            .join("cache")
            .join("bank-picker")
            .canonicalize()?;
        let file = file.canonicalize()?;
        let incoming = file.parent().context("Cartella mancante")?;
        ensure!(
            incoming.parent() == Some(inbox.as_path())
                && incoming
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("listato-"))
                && matches!(
                    file.file_name().and_then(|n| n.to_str()),
                    Some("questions.csv" | "questions.json")
                ),
            "Percorso di importazione non valido"
        );
        let result = self.stage_bank(ui, &file, incoming);
        if incoming.exists() {
            std::fs::remove_dir_all(incoming)?;
        }
        result
    }

    #[cfg(target_os = "android")]
    fn stage_bank(&self, ui: &AppWindow, file: &Path, incoming: &Path) -> Result<()> {
        self.discard_unused_media()?;
        if ui.get_page() != "import" {
            return Ok(());
        }
        std::fs::create_dir_all(&self.staged_media)?;
        let dir = self.staged_media.join(
            incoming
                .file_name()
                .context("Nome della cartella mancante")?,
        );
        std::fs::rename(incoming, &dir)?;
        ui.set_import_path(
            dir.join(file.file_name().context("Nome del file mancante")?)
                .to_string_lossy()
                .as_ref()
                .into(),
        );
        ui.set_asset_root(dir.to_string_lossy().as_ref().into());
        ui.set_notice("Cartella pronta per l'importazione.".into());
        Ok(())
    }

    fn leave_activity(&mut self, ui: &AppWindow) -> Result<()> {
        if self.clock.is_some() || self.pending_pause.is_some() {
            self.exam(ui, Action::Pause)?;
        }
        self.session = None;
        self.study.clear();
        self.continuous = None;
        ui.set_study_continuous(false);
        ui.set_confirm_cancel(false);
        ui.set_cancel_target("".into());
        Ok(())
    }

    fn home(&mut self, ui: &AppWindow) -> Result<()> {
        self.leave_activity(ui)?;
        ui.set_page("home".into());
        #[cfg(target_os = "android")]
        {
            self.discard_unused_media()?;
            ui.set_import_path("".into());
        }
        self.bank(ui)
    }

    fn section(&mut self, ui: &AppWindow, action: &str) -> Result<()> {
        if action == "stats" && ui.get_page() == "stats" {
            return self.stats(ui);
        }
        let refresh = self.session.is_some() || !self.study.is_empty() || ui.get_page() == "import";
        self.leave_activity(ui)?;
        let page = match action {
            "study-setup" | "review" | "study-start" | "study-continuous" => {
                ui.set_mode(
                    if action == "review" {
                        "Ripasso"
                    } else {
                        "Studio"
                    }
                    .into(),
                );
                "study-setup"
            }
            "simulation" | "official" | "custom" => {
                let custom = action == "custom";
                ui.set_mode(
                    if custom {
                        "Personalizzata"
                    } else {
                        "Ufficiale"
                    }
                    .into(),
                );
                "simulation"
            }
            other => other,
        };
        if refresh {
            self.bank(ui)?;
        } else if action == "study-start" {
            self.refresh_dashboard(ui, Local::now())?;
        } else if matches!(page, "study-setup" | "simulation") {
            self.rule(ui);
        }
        ui.set_page(page.into());
        if page == "import" {
            return self.bank_management(ui);
        }
        if page == "settings" {
            ui.set_exam_date_input(ui.get_exam_date());
        }
        if matches!(action, "study-start" | "study-continuous") {
            if ui.get_total() == 0 {
                ui.set_page("import".into());
                self.bank_management(ui)?;
                ui.set_notice("Importa il listato per iniziare a studiare.".into());
                return Ok(());
            }
            let topics = self.topic_ids.clone();
            if action == "study-continuous" {
                return self.start_study(ui, topics, None, false);
            }
            if self.quick_study_count == 0 {
                ui.set_page("home".into());
                ui.set_notice("Obiettivo di oggi completato.".into());
                return Ok(());
            }
            return self.start_study(ui, topics, Some(self.quick_study_count), false);
        }
        if action == "stats" {
            return self.stats(ui);
        }
        Ok(())
    }

    fn switch_bank(&mut self, ui: &AppWindow, version: &str) -> Result<()> {
        let edition = catalog::load()?
            .listati
            .into_iter()
            .find(|e| e.pronto && e.versione == version)
            .context("Listato non disponibile")?;
        let license = if edition
            .patenti
            .iter()
            .any(|l| l == ui.get_license().as_str())
        {
            ui.get_license()
        } else if edition.patenti.iter().any(|l| l == "B") {
            "B".into()
        } else {
            edition
                .patenti
                .first()
                .context("Categoria mancante")?
                .as_str()
                .into()
        };
        ui.set_license(license);
        ui.set_procedure(edition.procedura.into());
        self.refresh_bank(ui)
    }

    fn refresh_bank(&mut self, ui: &AppWindow) -> Result<()> {
        self.leave_activity(ui)?;
        self.bank(ui)?;
        profile::select(&self.db, &ui.get_license(), &ui.get_procedure())?;
        ui.set_stats_bank(self.version.as_str().into());
        ui.set_stats_topic("Tutti".into());
        match ui.get_page().as_str() {
            "stats" => self.stats(ui),
            "import" => self.bank_management(ui),
            "study" | "paused" | "exam" | "result" => {
                ui.set_page("home".into());
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn act(&mut self, ui: &AppWindow, action: &str) -> Result<()> {
        match action {
            "select-all" | "select-none" => {
                let chosen = action == "select-all";
                let topics = ui.get_topics();
                for (i, mut t) in topics.iter().enumerate() {
                    t.chosen = chosen;
                    topics.set_row_data(i, t);
                }
                ui.set_selected_count(if chosen { topics.row_count() as i32 } else { 0 });
                Ok(())
            }
            #[cfg(target_os = "android")]
            "pick-bank" => crate::android::choose_bank(),
            #[cfg(not(target_os = "android"))]
            "pick-bank" => self.pick_bank(ui),
            "save-bank-key" => self.set_bank_key(ui),
            "preview-archive" => self.preview_archive(ui),
            "clean-archive" => self.clean_archive(ui),
            "cancel-cleanup" => self.bank_management(ui),
            "home" => self.home(ui),
            "bank" => self.refresh_bank(ui),
            "onboarding-save" | "onboarding-undecided" => {
                if ui.get_show_exam_onboarding() {
                    let input = ui.get_onboarding_date();
                    let date = (action == "onboarding-save").then_some(input.as_str());
                    if let Err(error) = profile::finish_exam_onboarding(
                        &mut self.db,
                        &ui.get_license(),
                        &ui.get_procedure(),
                        date,
                    ) {
                        ui.set_onboarding_error(error.to_string().into());
                    } else {
                        ui.set_onboarding_error("".into());
                        ui.set_show_exam_onboarding(false);
                        self.refresh_dashboard(ui, Local::now())?;
                    }
                }
                Ok(())
            }
            "save-exam-date" => {
                profile::save_exam_date(
                    &self.db,
                    &ui.get_license(),
                    &ui.get_procedure(),
                    &ui.get_exam_date_input(),
                )?;
                self.refresh_dashboard(ui, Local::now())?;
                ui.set_notice(
                    if ui.get_exam_date().is_empty() {
                        "Data d’esame rimossa."
                    } else {
                        "Data d’esame salvata."
                    }
                    .into(),
                );
                Ok(())
            }
            "verify-bank" => self.verify_bank(ui),
            "config" => {
                self.rule(ui);
                Ok(())
            }
            "study-setup" | "review" | "study-start" | "study-continuous" | "simulation"
            | "official" | "custom" | "import" | "settings" | "stats" => self.section(ui, action),
            "save-media" => {
                ensure!(
                    self.signed_file()?.is_none(),
                    "Reimporta il pacchetto firmato per cambiare la cartella delle immagini"
                );
                Self::save_media(&self.db, &self.version, &ui.get_asset_root())?;
                self.home(ui)
            }
            "import-file" | "confirm-unsigned" => {
                self.import_bank(ui, action == "confirm-unsigned")
            }
            "start" => self.start(ui),
            "true" | "false" if ui.get_page() == "study" => self.answer_study(ui, action == "true"),
            "study-prev" if ui.get_page() == "study" => {
                ensure!(self.study_index > 0, "Prima domanda della sessione");
                self.study_index -= 1;
                self.show_card(ui)
            }
            "next" if ui.get_page() == "study" => self.advance_study(ui),
            "true" | "false" => self.exam(ui, Action::Answer(Some(action == "true"))),
            "clear" => self.exam(ui, Action::Answer(None)),
            "submit" => {
                ui.set_errors_only(false);
                self.exam(ui, Action::Submit)
            }
            "pause" => self.exam(ui, Action::Pause),
            "resume" => {
                let created = self.session.as_ref().is_some_and(|s| s.status == "created");
                self.exam(
                    ui,
                    if created {
                        Action::Start
                    } else {
                        Action::Resume
                    },
                )
            }
            "cancel" => {
                ensure!(ui.get_confirm_cancel(), "Conferma l'annullamento");
                self.exam(ui, Action::Cancel)
            }
            "result" => self.show_result(
                ui,
                &self
                    .session
                    .as_ref()
                    .context("Risultato non disponibile")?
                    .id,
            ),
            _ => bail!("Azione sconosciuta"),
        }
    }
}

fn feedback(ui: &AppWindow, res: Result<()>) {
    if let Err(error) = res {
        eprintln!("ZeroPatenti: {error:#}");
        let message = error.to_string();
        let technical = ["fsrs", "retention", "stability", "retrievability"];
        let public = if technical
            .iter()
            .any(|term| message.to_ascii_lowercase().contains(term))
        {
            "Non è stato possibile aggiornare il ripasso della domanda. Riprova.".to_string()
        } else {
            message
        };
        ui.set_notice(public.into());
    }
}

#[cfg(test)]
mod tests;

fn clock_text(elapsed: i64, left: Option<i64>) -> String {
    match left {
        Some(ms) => format!("Tempo residuo {}", duration(ms)),
        None => format!("Tempo attivo {} · senza limite", duration(elapsed)),
    }
}

fn bind(mut controller: Controller) -> Result<(AppWindow, Rc<RefCell<Controller>>, Timer)> {
    let ui = AppWindow::new()?;
    if let Some((license, procedure)) = profile::active(&controller.db)? {
        ui.set_license(license.into());
        ui.set_procedure(procedure.into());
    }
    controller.home(&ui)?;
    ui.set_show_exam_onboarding(profile::needs_exam_onboarding(&controller.db)?);
    ui.set_stats_bank(controller.version.as_str().into());
    ui.set_stats_topic("Tutti".into());
    #[cfg(target_os = "android")]
    if let Some(dir) = crate::android::import_dir() {
        ui.set_import_path(dir.join("questions.json").to_string_lossy().as_ref().into());
    }
    let state = Rc::new(RefCell::new(controller));
    let (weak, data) = (ui.as_weak(), state.clone());
    ui.on_act(move |name| {
        let ui = weak.unwrap();
        ui.set_notice("".into());
        feedback(&ui, data.borrow_mut().act(&ui, &name));
    });
    let (weak, data) = (ui.as_weak(), state.clone());
    ui.on_jump(move |index| {
        let ui = weak.unwrap();
        let mut c = data.borrow_mut();
        let res = if ui.get_page() == "study" {
            c.rate(&ui, index as u8)
        } else {
            c.exam(&ui, Action::Go(index as usize))
        };
        feedback(&ui, res);
    });
    let weak = ui.as_weak();
    ui.on_matching_topics(|topics, query, selected_only, _selection_count| {
        let query = query.trim().to_lowercase();
        model(
            topics
                .iter()
                .enumerate()
                .filter(|(_, topic)| {
                    (!selected_only || topic.chosen) && topic.label.to_lowercase().contains(&query)
                })
                .map(|(i, _)| i as i32)
                .collect(),
        )
    });
    ui.on_topic_toggle(move |i| {
        let ui = weak.unwrap();
        let topics = ui.get_topics();
        if let Some(mut t) = topics.row_data(i as usize) {
            t.chosen = !t.chosen;
            topics.set_row_data(i as usize, t);
            ui.set_selected_count(topics.iter().filter(|t| t.chosen).count() as i32);
        }
    });
    let (weak, data) = (ui.as_weak(), state.clone());
    ui.on_open_entry(move |kind, id| {
        let ui = weak.unwrap();
        let mut c = data.borrow_mut();
        ui.set_notice("".into());
        ui.set_confirm_cancel(false);
        ui.set_errors_only(false);
        let res = (|| {
            if kind == "bank" {
                return c.switch_bank(&ui, &id);
            }
            if kind == "cancel-saved" || kind == "resume-saved" {
                ensure!(
                    matches!(ui.get_page().as_str(), "home" | "simulation"),
                    "Torna all’elenco delle prove sospese"
                );
                let saved = store::view(&c.db, &id)?;
                ensure!(
                    saved.license == ui.get_license().as_str()
                        && saved.procedure == ui.get_procedure().as_str()
                        && matches!(saved.status.as_str(), "paused" | "created"),
                    "Prova sospesa non disponibile"
                );
                if kind == "cancel-saved" {
                    ensure!(
                        ui.get_cancel_target() == id,
                        "Conferma l’annullamento della prova selezionata"
                    );
                    store::command(&mut c.db, &id, Action::Cancel, saved.active_ms, now())?;
                    ui.set_cancel_target("".into());
                    c.refresh_sessions(&ui)?;
                    ui.set_notice("Prova annullata.".into());
                    return Ok(());
                }
                c.leave_activity(&ui)?;
                c.session = Some(saved);
                return c.act(&ui, "resume");
            }
            c.leave_activity(&ui)?;
            c.session = Some(store::view(&c.db, &id)?);
            if kind == "result" {
                c.show_result(&ui, &id)
            } else {
                c.show_exam(&ui)
            }
        })();
        feedback(&ui, res);
    });
    let (weak, data) = (ui.as_weak(), state.clone());
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut c = data.borrow_mut();
        #[cfg(target_os = "android")]
        for file in crate::android::take_banks() {
            feedback(&ui, c.receive_bank(&ui, &file));
        }
        if c.clock.is_some() || c.pending_pause.is_some() {
            feedback(&ui, c.tick(&ui));
        } else if ui.get_page() == "home" {
            feedback(&ui, c.refresh_dashboard_if_needed(&ui, Local::now()));
        }
    });
    #[cfg(target_os = "android")]
    crate::android::on_background({
        let (weak, data) = (ui.as_weak(), state.clone());
        move || {
            if let Some(ui) = weak.upgrade() {
                let mut c = data.borrow_mut();
                if c.clock.is_some() {
                    feedback(&ui, c.exam(&ui, Action::Pause));
                }
            }
        }
    });
    Ok((ui, state, timer))
}

pub fn run(path: &Path) -> Result<()> {
    let (ui, state, _timer) = bind(Controller::open(path)?)?;
    #[cfg(not(target_os = "android"))]
    {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
        ui.window().set_size(slint::LogicalSize::new(1080., 650.));
        let (weak, data) = (ui.as_weak(), state.clone());
        ui.window().on_winit_window_event(move |_, event| {
            if matches!(event, WindowEvent::Focused(false))
                && let Some(ui) = weak.upgrade()
            {
                let mut c = data.borrow_mut();
                if c.clock.is_some() {
                    feedback(&ui, c.exam(&ui, Action::Pause));
                }
            }
            EventResult::Propagate
        });
    }
    ui.run()?;
    if state.borrow().clock.is_some() || state.borrow().pending_pause.is_some() {
        state.borrow_mut().exam(&ui, Action::Pause)?;
    }
    #[cfg(target_os = "android")]
    state.borrow().discard_unused_media()?;
    Ok(())
}

use std::{
    cell::RefCell,
    collections::VecDeque,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{Local, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

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

struct Controller {
    db: Connection,
    _instance: std::fs::File,
    #[cfg(any(target_os = "android", test))]
    staged_media: PathBuf,
    topic_ids: Vec<i64>,
    version: String,
    session: Option<SessionView>,
    clock: Option<ActiveClock>,
    pending_pause: Option<(i64, i64)>,
    study: VecDeque<StudyCard>,
    study_answer: Option<bool>,
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
        db.execute_batch("CREATE TABLE IF NOT EXISTS gui_media_roots(bank_version TEXT PRIMARY KEY, directory TEXT NOT NULL);")?;
        store::recover(&mut db, now())?;
        Ok(Self {
            db,
            _instance: instance,
            #[cfg(any(target_os = "android", test))]
            staged_media: path.with_file_name("bank-media"),
            topic_ids: vec![],
            version: String::new(),
            session: None,
            clock: None,
            pending_pause: None,
            study: VecDeque::new(),
            study_answer: None,
        })
    }

    fn bank(&mut self, ui: &AppWindow) -> Result<()> {
        self.version = catalog::version_for(&ui.get_license(), &ui.get_procedure())?;
        let topics = storage::topic_counts(&self.db, &self.version)?;
        let total: i64 = topics.iter().map(|t| t.total).sum();
        ui.set_bank(format!("{} · {total} domande", self.version).into());
        self.topic_ids = topics.iter().map(|t| t.id).collect();
        ui.set_selected_count(topics.len() as i32);
        let mut stats_topics = vec!["Tutti".into()];
        stats_topics.extend(topics.iter().map(|t| SharedString::from(t.name.as_str())));
        ui.set_stats_topics(model(stats_topics));
        ui.set_stats_topic("Tutti".into());
        ui.set_topics(model(
            topics
                .iter()
                .map(|t| TopicItem {
                    label: format!(
                        "{} · {} domande · {} studiate · {} da ripassare",
                        t.name, t.total, t.studied, t.due
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
        let sessions = store::unfinished(&self.db)?
            .into_iter()
            .filter(|s| {
                s.license == ui.get_license().as_str() && s.procedure == ui.get_procedure().as_str()
            })
            .map(|s| Entry {
                label: format!(
                    "Riprendi · {} · {}/{} risposte",
                    s.license,
                    s.questions.iter().filter(|q| q.answer.is_some()).count(),
                    s.questions.len()
                )
                .into(),
                key: s.id.into(),
            })
            .collect();
        ui.set_sessions(model(sessions));
        Ok(())
    }

    fn rule(&self, ui: &AppWindow) {
        let hint = (|| -> Result<String> {
            let rule = verified_rule(&self.db, &ui.get_license(), &ui.get_procedure())?;
            if ui.get_mode() == "Ufficiale"
                && crate::exam::catalog::official_quotas(&rule.bank_version).is_none()
            {
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
            ensure!(!topics.is_empty(), "Seleziona almeno un argomento");
            self.study =
                review::due_cards(&self.db, &self.version, count, &topics, mode == "Ripasso")?
                    .into();
            ensure!(
                !self.study.is_empty(),
                "Nessuna domanda disponibile per questi argomenti"
            );
            ui.set_page("study".into());
            return self.show_card(ui);
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

    fn figure(&self, ui: &AppWindow, relative: Option<&str>) {
        ui.set_has_figure(false);
        let Some(relative) = relative else { return };
        let root = PathBuf::from(ui.get_asset_root().as_str());
        let image = (|| {
            let root = root.canonicalize().ok()?;
            let path = root.join(relative).canonicalize().ok()?;
            if !path.starts_with(root) {
                return None;
            }
            Image::load_from_path(&path).ok()
        })();
        if let Some(image) = image {
            ui.set_figure(image);
            ui.set_has_figure(true);
        } else {
            ui.set_notice("Figura non disponibile. Controlla la cartella delle immagini.".into());
        }
    }

    fn show_card(&mut self, ui: &AppWindow) -> Result<()> {
        self.study_answer = None;
        if let Some(card) = self.study.front() {
            storage::mark_seen(&self.db, &card.id, now())?;
            ui.set_title_text(ui.get_mode());
            ui.set_subtitle(card.topic.as_str().into());
            ui.set_prompt(card.text.as_str().into());
            ui.set_study_stage("answer".into());
            ui.set_answer("".into());
            self.figure(ui, card.figure.as_deref());
        } else {
            self.home(ui)?;
            ui.set_notice("Sessione completata.".into());
        }
        Ok(())
    }

    fn rate(&mut self, ui: &AppWindow, rating: u8) -> Result<()> {
        let card = self.study.front().context("Nessuna domanda aperta")?;
        let answer = self.study_answer.context("Rispondi alla domanda")?;
        let correct = answer == card.correct;
        ensure!(
            ui.get_study_stage() != "next" && (correct || rating == 1),
            "Valutazione non disponibile"
        );
        let tx = self.db.transaction()?;
        review::apply_review(&tx, &card.id, correct, rating, now(), 0.90)?;
        tx.commit()?;
        ui.set_study_stage("next".into());
        Ok(())
    }

    fn answer_study(&mut self, ui: &AppWindow, value: bool) -> Result<()> {
        ensure!(ui.get_study_stage() == "answer", "Risposta già registrata");
        let correct = self
            .study
            .front()
            .context("Nessuna domanda aperta")?
            .correct;
        self.study_answer = Some(value);
        if correct == value {
            ui.set_details("Corretto. Quanto è stato difficile ricordarla?".into());
            ui.set_study_stage("rate".into());
        } else {
            self.rate(ui, 1)?;
            ui.set_details(format!("Risposta errata. Soluzione: {}.", truth(correct)).into());
        }
        Ok(())
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
        ui.set_grid(model(
            s.questions
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    format!("{}{}", i + 1, if q.answer.is_some() { " ✓" } else { "" }).into()
                })
                .collect(),
        ));
        self.figure(ui, question.image_path.as_deref());
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
        let series = ui.get_stats_series();
        let strategy = match series.as_str() {
            "Casuale" => Some("random"),
            "Equilibrata" => Some("balanced"),
            "Intelligente" => Some("intelligent"),
            _ => None,
        };
        let topic_id = ui
            .get_stats_topics()
            .iter()
            .position(|v| v == ui.get_stats_topic())
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.topic_ids.get(i).copied());
        let filters = statistics::Filters {
            license: Some(ui.get_license().to_string()),
            procedure: Some(ui.get_procedure().to_string()),
            bank_version: Some(self.version.clone()),
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
        ui.set_details(dash.summary().into());
        let mut stmt=self.db.prepare("SELECT s.session_id, r.submitted_at,s.kind,r.correct,r.wrong+r.omitted FROM exam_sessions s JOIN exam_results r USING(session_id)
            WHERE s.license=?1 AND s.procedure=?2 AND s.bank_version=?3 AND (?4 IS NULL OR s.kind=?4) AND (?5 IS NULL OR s.strategy=?5)
            AND (?6 IS NULL OR r.submitted_at>=?6) AND (?7 IS NULL OR r.submitted_at<?7)
            AND (?8 IS NULL OR EXISTS(SELECT 1 FROM exam_session_questions q WHERE q.session_id=s.session_id AND q.topic_id=?8))
            ORDER BY r.submitted_at DESC,s.session_id")?;
        let entries = stmt
            .query_map(
                params![
                    filters.license,
                    filters.procedure,
                    filters.bank_version,
                    filters.kind,
                    filters.strategy,
                    filters.from,
                    filters.until,
                    filters.topic_id
                ],
                |r| {
                    let kind: String = r.get(2)?;
                    Ok(Entry {
                        key: r.get::<_, String>(0)?.into(),
                        label: format!(
                            "{} · {} · {} corrette, {} errori",
                            Local
                                .timestamp_opt(r.get(1)?, 0)
                                .single()
                                .map(|d| d.format("%Y-%m-%d").to_string())
                                .unwrap_or_default(),
                            if kind == "custom" {
                                "Personalizzata"
                            } else {
                                "Ufficiale"
                            },
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?
                        )
                        .into(),
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ui.set_history(model(entries));
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
        if !self.staged_media.is_dir() {
            return Ok(());
        }
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

    fn home(&mut self, ui: &AppWindow) -> Result<()> {
        self.session = None;
        self.clock = None;
        ui.set_confirm_cancel(false);
        ui.set_page("home".into());
        #[cfg(target_os = "android")]
        {
            self.discard_unused_media()?;
            ui.set_import_path("".into());
        }
        self.bank(ui)
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
            "home" => self.home(ui),
            "bank" => self.bank(ui),
            "config" => {
                self.rule(ui);
                Ok(())
            }
            "import" => {
                ui.set_page("import".into());
                Ok(())
            }
            "save-media" => {
                Self::save_media(&self.db, &self.version, &ui.get_asset_root())?;
                self.home(ui)
            }
            "import-file" => {
                let file = PathBuf::from(ui.get_import_path().as_str());
                let root = if ui.get_asset_root().is_empty() {
                    file.canonicalize()?
                        .parent()
                        .context("Cartella del listato mancante")?
                        .to_string_lossy()
                        .into_owned()
                } else {
                    ui.get_asset_root().to_string()
                };
                let questions = storage::load_normalized_questions(&file)?;
                let version = catalog::version_for(&ui.get_license(), &ui.get_procedure())?;
                let tx = self.db.transaction()?;
                let count = storage::import_questions_in(&tx, &version, questions)?;
                verified_rule(&tx, &ui.get_license(), &ui.get_procedure())?;
                let stored: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM questions WHERE source_version=?1",
                    [&version],
                    |row| row.get(0),
                )?;
                ensure!(
                    i64::try_from(count)? == stored,
                    "Listato incompleto per {version}"
                );
                Self::save_media(&tx, &version, &root)?;
                tx.commit()?;
                self.home(ui)?;
                ui.set_notice(format!("Importate {count} domande.").into());
                Ok(())
            }
            "start" => self.start(ui),
            "true" | "false" if ui.get_page() == "study" => self.answer_study(ui, action == "true"),
            "next" => {
                self.study.pop_front();
                self.show_card(ui)
            }
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
            "stats" => self.stats(ui),
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
        ui.set_notice(format!("{error:#}").into());
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
    controller.home(&ui)?;
    #[cfg(target_os = "android")]
    ui.set_folder_picker(true);
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
    ui.on_matches_topic(|label, query| label.to_lowercase().contains(&query.trim().to_lowercase()));
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

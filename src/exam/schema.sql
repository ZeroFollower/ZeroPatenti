CREATE TABLE IF NOT EXISTS exam_sessions (
    session_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('official', 'custom')),
    license TEXT NOT NULL,
    procedure TEXT NOT NULL,
    bank_version TEXT NOT NULL,
    rule_version TEXT NOT NULL,
    rule_source TEXT NOT NULL,
    config_json TEXT NOT NULL,
    strategy TEXT NOT NULL,
    seed TEXT NOT NULL,
    algorithm_version INTEGER NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('created','active','paused','cancelled','submitted')),
    current_index INTEGER NOT NULL DEFAULT 0 CHECK(current_index >= 0),
    threshold INTEGER NOT NULL CHECK(threshold >= 0),
    duration_ms INTEGER CHECK(duration_ms > 0),
    active_ms INTEGER NOT NULL DEFAULT 0 CHECK(active_ms >= 0),
    remaining_ms INTEGER CHECK(remaining_ms >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    CHECK ((duration_ms IS NULL AND remaining_ms IS NULL) OR
           (duration_ms IS NOT NULL AND remaining_ms IS NOT NULL AND
            duration_ms = active_ms + remaining_ms))
);
CREATE INDEX IF NOT EXISTS exam_session_status ON exam_sessions(status, updated_at);

CREATE TABLE IF NOT EXISTS exam_session_questions (
    session_id TEXT NOT NULL REFERENCES exam_sessions(session_id),
    position INTEGER NOT NULL CHECK(position >= 0),
    question_id TEXT NOT NULL,
    topic_id INTEGER NOT NULL,
    topic TEXT NOT NULL,
    question_text TEXT NOT NULL,
    correct_answer INTEGER NOT NULL CHECK(correct_answer IN (0,1)),
    image_path TEXT,
    previously_seen INTEGER NOT NULL CHECK(previously_seen IN (0,1)),
    PRIMARY KEY(session_id,position),
    UNIQUE(session_id,question_id)
);
CREATE TABLE IF NOT EXISTS exam_answers (
    session_id TEXT NOT NULL,
    question_id TEXT NOT NULL,
    answer INTEGER CHECK(answer IN (0,1)),
    updated_at INTEGER NOT NULL,
    changes INTEGER NOT NULL DEFAULT 0 CHECK(changes >= 0),
    PRIMARY KEY(session_id,question_id),
    FOREIGN KEY(session_id,question_id) REFERENCES exam_session_questions(session_id,question_id)
);
CREATE TABLE IF NOT EXISTS exam_events (
    event_id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES exam_sessions(session_id),
    sequence INTEGER NOT NULL CHECK(sequence > 0),
    event_type TEXT NOT NULL,
    event_version INTEGER NOT NULL DEFAULT 1,
    occurred_at INTEGER NOT NULL,
    active_ms INTEGER NOT NULL CHECK(active_ms >= 0),
    payload_json TEXT NOT NULL,
    UNIQUE(session_id,sequence)
);
CREATE INDEX IF NOT EXISTS exam_event_history ON exam_events(session_id,event_type,occurred_at);
CREATE INDEX IF NOT EXISTS exam_previous_display
    ON exam_events(json_extract(payload_json,'$.question_id')) WHERE event_type='QuestionDisplayed';
CREATE TRIGGER IF NOT EXISTS exam_events_no_update
    BEFORE UPDATE ON exam_events BEGIN SELECT RAISE(ABORT,'exam_events append-only'); END;
CREATE TRIGGER IF NOT EXISTS exam_events_no_delete
    BEFORE DELETE ON exam_events BEGIN SELECT RAISE(ABORT,'exam_events append-only'); END;
CREATE TABLE IF NOT EXISTS exam_results (
    session_id TEXT PRIMARY KEY NOT NULL REFERENCES exam_sessions(session_id),
    correct INTEGER NOT NULL CHECK(correct >= 0),
    wrong INTEGER NOT NULL CHECK(wrong >= 0),
    omitted INTEGER NOT NULL CHECK(omitted >= 0),
    passed INTEGER NOT NULL CHECK(passed IN (0,1)),
    result_json TEXT NOT NULL,
    submitted_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS review_applications (
    session_id TEXT NOT NULL,
    question_id TEXT NOT NULL,
    review_log_id INTEGER NOT NULL UNIQUE REFERENCES review_log(id),
    rating INTEGER NOT NULL CHECK(rating IN (1,3)),
    PRIMARY KEY(session_id,question_id),
    FOREIGN KEY(session_id,question_id) REFERENCES exam_session_questions(session_id,question_id)
);

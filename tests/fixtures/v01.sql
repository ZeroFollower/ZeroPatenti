BEGIN TRANSACTION;
CREATE TABLE questions (
            card_id TEXT PRIMARY KEY,
            source_version TEXT NOT NULL,
            external_id INTEGER NOT NULL,
            group_id INTEGER NOT NULL,
            topic_id INTEGER NOT NULL,
            topic TEXT NOT NULL,
            question_text TEXT NOT NULL,
            correct_answer INTEGER NOT NULL CHECK(correct_answer IN (0,1)),
            image_path TEXT,
            UNIQUE(source_version, external_id)
        );
INSERT INTO "questions" VALUES('DEMO:1','DEMO',1,10,1,'Segnaletica','DEMO: Il segnale rosso richiede di fermarsi.',1,NULL);
INSERT INTO "questions" VALUES('DEMO:2','DEMO',2,10,1,'Segnaletica','DEMO: In presenza del segnale di STOP non occorre mai fermarsi.',0,NULL);
CREATE TABLE card_state (
            card_id TEXT PRIMARY KEY REFERENCES questions(card_id) ON DELETE CASCADE,
            stability REAL NOT NULL,
            stability_fast REAL NOT NULL,
            difficulty REAL NOT NULL,
            last_review_at INTEGER NOT NULL,
            due_at INTEGER NOT NULL,
            repetitions INTEGER NOT NULL DEFAULT 0
        );
INSERT INTO "card_state" VALUES('DEMO:2',0.1103999987244606,0.08832000195980072,6.1686000823974609,1795372355,1795458755,1);
CREATE TABLE review_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            card_id TEXT NOT NULL REFERENCES questions(card_id),
            reviewed_at INTEGER NOT NULL,
            answer_correct INTEGER NOT NULL CHECK(answer_correct IN (0,1)),
            rating INTEGER NOT NULL CHECK(rating BETWEEN 1 AND 4),
            elapsed_days REAL NOT NULL,
            predicted_interval_days REAL NOT NULL,
            due_at INTEGER NOT NULL,
            fsrs_version TEXT NOT NULL,
            desired_retention REAL NOT NULL
        );
INSERT INTO "review_log" VALUES(1,'DEMO:2',1795372355,0,1,0.0,3.8275626138783991e-05,1795458755,'7.0.0',0.89999997615814209);
CREATE INDEX idx_questions_version
          ON questions(source_version);
CREATE INDEX idx_state_due ON card_state(due_at);
CREATE INDEX idx_review_card ON review_log(card_id, reviewed_at);
DELETE FROM "sqlite_sequence";
INSERT INTO "sqlite_sequence" VALUES('review_log',1);
COMMIT;

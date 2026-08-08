ALTER TABLE identification_review_cases
ADD COLUMN version INTEGER NOT NULL DEFAULT 1 CHECK(version>=1);

CREATE TABLE identification_task_decisions (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    case_id BLOB NOT NULL CHECK(length(case_id)=16),
    case_version INTEGER NOT NULL CHECK(case_version>=2),
    kind TEXT NOT NULL CHECK(kind IN (
        'select-provider-candidate','rematch-with-hints','select-generic-video'
    )),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json) AND length(payload_json)<=16384),
    request_digest BLOB NOT NULL CHECK(length(request_digest)=32),
    idempotency_key_sha256 BLOB NOT NULL CHECK(length(idempotency_key_sha256)=32),
    created_at_us INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(case_id) REFERENCES identification_review_cases(id)
) STRICT;

CREATE UNIQUE INDEX identification_task_decision_idempotency
ON identification_task_decisions(account_id,case_id,idempotency_key_sha256);

CREATE INDEX identification_task_decision_case_history
ON identification_task_decisions(case_id,created_at_us DESC,id DESC);

CREATE TABLE identification_decision_dispatches (
    decision_id BLOB PRIMARY KEY NOT NULL CHECK(length(decision_id)=16),
    state TEXT NOT NULL CHECK(state IN ('accepted','applied','failed')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count>=0),
    next_attempt_at_us INTEGER,
    last_error_code TEXT CHECK(last_error_code IS NULL OR length(last_error_code) BETWEEN 1 AND 128),
    updated_at_us INTEGER NOT NULL,
    CHECK((state='accepted') OR next_attempt_at_us IS NULL),
    FOREIGN KEY(decision_id) REFERENCES identification_task_decisions(id)
) STRICT;

CREATE INDEX identification_decision_dispatch_pending
ON identification_decision_dispatches(state,next_attempt_at_us,decision_id);

CREATE TABLE identification_feedback (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    source_decision_id BLOB NOT NULL UNIQUE CHECK(length(source_decision_id)=16),
    selector_version INTEGER NOT NULL CHECK(selector_version=1),
    media_type TEXT NOT NULL CHECK(media_type IN ('movie','tv')),
    normalized_title TEXT NOT NULL CHECK(length(normalized_title) BETWEEN 1 AND 200),
    year INTEGER CHECK(year IS NULL OR year BETWEEN 1870 AND 2200),
    season INTEGER CHECK(season IS NULL OR season BETWEEN 0 AND 999),
    episodes_json TEXT NOT NULL CHECK(json_valid(episodes_json) AND length(episodes_json)<=256),
    provider TEXT NOT NULL CHECK(provider='tmdb'),
    provider_id TEXT NOT NULL CHECK(length(provider_id) BETWEEN 1 AND 64),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
    created_at_us INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(source_decision_id) REFERENCES identification_task_decisions(id)
) STRICT;

CREATE INDEX identification_feedback_exact_selector
ON identification_feedback(
    account_id,enabled,selector_version,media_type,normalized_title,year,season,episodes_json
);

ALTER TABLE tasks_processing_tasks
ADD COLUMN current_task_decision_id BLOB REFERENCES identification_task_decisions(id);

ALTER TABLE tasks_processing_tasks
ADD COLUMN decision_checkpoint TEXT CHECK(decision_checkpoint IS NULL OR decision_checkpoint IN (
    'manual-decision-pending','rematch-pending','generic-video-selected','planning-requested'
));

CREATE TRIGGER identification_task_decisions_immutable
BEFORE UPDATE ON identification_task_decisions
BEGIN SELECT RAISE(ABORT, 'task decision is immutable'); END;

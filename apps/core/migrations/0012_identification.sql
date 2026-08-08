CREATE TABLE identification_attempts (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    processing_attempt_id BLOB NOT NULL UNIQUE CHECK(length(processing_attempt_id)=16),
    file_revision_id BLOB NOT NULL CHECK(length(file_revision_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal>=1),
    parser_version TEXT NOT NULL CHECK(length(parser_version) BETWEEN 1 AND 64),
    provider_version TEXT NOT NULL CHECK(length(provider_version) BETWEEN 1 AND 64),
    rule_version INTEGER NOT NULL CHECK(rule_version BETWEEN 1 AND 65535),
    status TEXT NOT NULL CHECK(status IN ('running','decided','revision-changed','failed')),
    failure_code TEXT CHECK(failure_code IS NULL OR length(failure_code) BETWEEN 1 AND 128),
    started_at_us INTEGER NOT NULL,
    finished_at_us INTEGER,
    UNIQUE(task_id,ordinal),
    CHECK((status='running') = (finished_at_us IS NULL)),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY(processing_attempt_id) REFERENCES tasks_processing_attempts(id) ON DELETE CASCADE,
    FOREIGN KEY(file_revision_id) REFERENCES discovery_file_revisions(id)
) STRICT;

CREATE UNIQUE INDEX identification_one_running_attempt
ON identification_attempts(task_id) WHERE status='running';

CREATE TABLE identification_evidence (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    attempt_id BLOB NOT NULL CHECK(length(attempt_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 256),
    source TEXT NOT NULL CHECK(source IN ('filename','nfo','tmdb','cache','system')),
    source_version TEXT NOT NULL CHECK(length(source_version) BETWEEN 1 AND 64),
    kind TEXT NOT NULL CHECK(kind IN ('external-id','title','alias','year','episode','media-type','conflict','availability')),
    normalized_value TEXT NOT NULL CHECK(length(normalized_value) BETWEEN 1 AND 1024),
    strength TEXT NOT NULL CHECK(strength IN ('strong','supporting','conflicting')),
    reason TEXT NOT NULL CHECK(reason IN (
        'filename.title','filename.year','filename.episode','filename.media-type',
        'nfo.external-id','nfo.title','nfo.year','nfo.episode',
        'tmdb.external-id-verified','tmdb.title-match','tmdb.alias-match',
        'tmdb.year-match','tmdb.episode-exists','system.conflict',
        'system.provider-unavailable'
    )),
    source_hash BLOB NOT NULL CHECK(length(source_hash)=32),
    created_at_us INTEGER NOT NULL,
    UNIQUE(attempt_id,ordinal),
    FOREIGN KEY(attempt_id) REFERENCES identification_attempts(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE identification_candidates (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    attempt_id BLOB NOT NULL CHECK(length(attempt_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 20),
    provider TEXT NOT NULL CHECK(provider='tmdb'),
    media_type TEXT NOT NULL CHECK(media_type IN ('movie','tv')),
    provider_id INTEGER NOT NULL CHECK(provider_id>0),
    year INTEGER CHECK(year IS NULL OR year BETWEEN 1800 AND 2200),
    locale TEXT NOT NULL CHECK(length(locale)=5),
    original_title TEXT CHECK(original_title IS NULL OR length(original_title) BETWEEN 1 AND 512),
    ranking_score INTEGER NOT NULL CHECK(ranking_score BETWEEN 0 AND 100),
    provider_version INTEGER NOT NULL CHECK(provider_version BETWEEN 1 AND 65535),
    source_hash BLOB NOT NULL CHECK(length(source_hash)=32),
    created_at_us INTEGER NOT NULL,
    UNIQUE(attempt_id,ordinal),
    UNIQUE(attempt_id,provider,media_type,provider_id),
    FOREIGN KEY(attempt_id) REFERENCES identification_attempts(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE identification_candidate_titles (
    candidate_id BLOB NOT NULL CHECK(length(candidate_id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('title','alias')),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 64),
    value TEXT NOT NULL CHECK(length(value) BETWEEN 1 AND 512),
    PRIMARY KEY(candidate_id,kind,ordinal),
    FOREIGN KEY(candidate_id) REFERENCES identification_candidates(id) ON DELETE CASCADE
) WITHOUT ROWID, STRICT;

CREATE TABLE identification_candidate_release_dates (
    candidate_id BLOB NOT NULL CHECK(length(candidate_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 64),
    release_date TEXT NOT NULL CHECK(length(release_date)=10),
    PRIMARY KEY(candidate_id,ordinal),
    FOREIGN KEY(candidate_id) REFERENCES identification_candidates(id) ON DELETE CASCADE
) WITHOUT ROWID, STRICT;

CREATE TABLE identification_candidate_episodes (
    candidate_id BLOB NOT NULL CHECK(length(candidate_id)=16),
    season_number INTEGER NOT NULL CHECK(season_number BETWEEN 0 AND 65535),
    episode_number INTEGER NOT NULL CHECK(episode_number BETWEEN 1 AND 65535),
    PRIMARY KEY(candidate_id,season_number,episode_number),
    FOREIGN KEY(candidate_id) REFERENCES identification_candidates(id) ON DELETE CASCADE
) WITHOUT ROWID, STRICT;

CREATE TABLE identification_candidate_external_ids (
    candidate_id BLOB NOT NULL CHECK(length(candidate_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 16),
    provider TEXT NOT NULL CHECK(length(provider) BETWEEN 1 AND 32),
    value TEXT NOT NULL CHECK(length(value) BETWEEN 1 AND 64),
    is_default INTEGER NOT NULL CHECK(is_default IN (0,1)),
    PRIMARY KEY(candidate_id,ordinal),
    FOREIGN KEY(candidate_id) REFERENCES identification_candidates(id) ON DELETE CASCADE
) WITHOUT ROWID, STRICT;

CREATE TABLE identification_decisions (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    attempt_id BLOB NOT NULL UNIQUE CHECK(length(attempt_id)=16),
    level TEXT NOT NULL CHECK(level IN ('confirmed','probable','ambiguous','unidentified','blocked')),
    reason TEXT NOT NULL CHECK(reason IN (
        'identification.ambiguous','identification.confirmed-external-id',
        'identification.confirmed-title-year','identification.multiple-strong-candidates',
        'identification.no-candidate','identification.probable-title',
        'identification.provider-unavailable','identification.provider-unauthorized'
    )),
    selected_candidate_id BLOB CHECK(selected_candidate_id IS NULL OR length(selected_candidate_id)=16),
    retry_at_us INTEGER,
    rule_version INTEGER NOT NULL CHECK(rule_version BETWEEN 1 AND 65535),
    decided_at_us INTEGER NOT NULL,
    CHECK(level!='confirmed' OR selected_candidate_id IS NOT NULL),
    CHECK(retry_at_us IS NULL OR level='blocked'),
    FOREIGN KEY(attempt_id) REFERENCES identification_attempts(id) ON DELETE CASCADE,
    FOREIGN KEY(selected_candidate_id) REFERENCES identification_candidates(id)
) STRICT;

CREATE TABLE identification_decision_reasons (
    decision_id BLOB NOT NULL CHECK(length(decision_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 32),
    reason TEXT NOT NULL CHECK(length(reason) BETWEEN 1 AND 128),
    PRIMARY KEY(decision_id,ordinal),
    FOREIGN KEY(decision_id) REFERENCES identification_decisions(id) ON DELETE CASCADE
) WITHOUT ROWID, STRICT;

CREATE TABLE identification_review_cases (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    attempt_id BLOB NOT NULL CHECK(length(attempt_id)=16),
    decision_id BLOB NOT NULL UNIQUE CHECK(length(decision_id)=16),
    file_revision_id BLOB NOT NULL CHECK(length(file_revision_id)=16),
    inbox_directory_id BLOB NOT NULL CHECK(length(inbox_directory_id)=16),
    level TEXT NOT NULL CHECK(level IN ('probable','ambiguous','unidentified')),
    reason TEXT NOT NULL CHECK(length(reason) BETWEEN 1 AND 128),
    title_hint TEXT CHECK(title_hint IS NULL OR length(title_hint) BETWEEN 1 AND 512),
    status TEXT NOT NULL CHECK(status IN ('active','closed')),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    closed_at_us INTEGER,
    CHECK((status='closed') = (closed_at_us IS NOT NULL)),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY(attempt_id) REFERENCES identification_attempts(id),
    FOREIGN KEY(decision_id) REFERENCES identification_decisions(id),
    FOREIGN KEY(file_revision_id) REFERENCES discovery_file_revisions(id),
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id)
) STRICT;

CREATE UNIQUE INDEX identification_one_active_review_case
ON identification_review_cases(task_id) WHERE status='active';

CREATE TABLE identification_review_case_order_history (
    revision INTEGER PRIMARY KEY AUTOINCREMENT,
    case_id BLOB NOT NULL CHECK(length(case_id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    status TEXT NOT NULL CHECK(status IN ('active','closed')),
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(case_id) REFERENCES identification_review_cases(id) ON DELETE CASCADE,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id)
) STRICT;

CREATE INDEX identification_review_snapshot_idx
ON identification_review_case_order_history(account_id,revision,case_id,updated_at_us,status);

CREATE INDEX identification_review_case_revision_idx
ON identification_review_case_order_history(case_id,revision DESC);

CREATE TRIGGER identification_review_order_after_insert
AFTER INSERT ON identification_review_cases
BEGIN
    INSERT INTO identification_review_case_order_history(case_id,account_id,status,updated_at_us)
    VALUES (NEW.id,NEW.account_id,NEW.status,NEW.updated_at_us);
END;

CREATE TRIGGER identification_review_order_after_update
AFTER UPDATE OF status,updated_at_us ON identification_review_cases
BEGIN
    INSERT INTO identification_review_case_order_history(case_id,account_id,status,updated_at_us)
    VALUES (NEW.id,NEW.account_id,NEW.status,NEW.updated_at_us);
END;

CREATE TRIGGER identification_evidence_immutable
BEFORE UPDATE ON identification_evidence
BEGIN SELECT RAISE(ABORT, 'identification evidence is immutable'); END;

CREATE TRIGGER identification_candidates_immutable
BEFORE UPDATE ON identification_candidates
BEGIN SELECT RAISE(ABORT, 'identification candidate is immutable'); END;

CREATE TRIGGER identification_decisions_immutable
BEFORE UPDATE ON identification_decisions
BEGIN SELECT RAISE(ABORT, 'identification decision is immutable'); END;

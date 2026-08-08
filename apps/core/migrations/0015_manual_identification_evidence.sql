DROP TRIGGER identification_evidence_immutable;

CREATE TABLE identification_evidence_next (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    attempt_id BLOB NOT NULL CHECK(length(attempt_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 1 AND 256),
    source TEXT NOT NULL CHECK(source IN (
        'filename','nfo','tmdb','cache','system','manual-decision','manual-feedback'
    )),
    source_version TEXT NOT NULL CHECK(length(source_version) BETWEEN 1 AND 64),
    kind TEXT NOT NULL CHECK(kind IN (
        'external-id','title','alias','year','episode','media-type','conflict','availability'
    )),
    normalized_value TEXT NOT NULL CHECK(length(normalized_value) BETWEEN 1 AND 1024),
    strength TEXT NOT NULL CHECK(strength IN ('strong','supporting','conflicting')),
    reason TEXT NOT NULL CHECK(reason IN (
        'filename.title','filename.year','filename.episode','filename.media-type',
        'nfo.external-id','nfo.title','nfo.year','nfo.episode',
        'tmdb.external-id-verified','tmdb.title-match','tmdb.alias-match',
        'tmdb.year-match','tmdb.episode-exists','system.conflict',
        'system.provider-unavailable','manual.provider-selected','manual.rematch-hint',
        'manual.feedback-exact'
    )),
    source_hash BLOB NOT NULL CHECK(length(source_hash)=32),
    created_at_us INTEGER NOT NULL,
    UNIQUE(attempt_id,ordinal),
    FOREIGN KEY(attempt_id) REFERENCES identification_attempts(id) ON DELETE CASCADE
) STRICT;

INSERT INTO identification_evidence_next
(id,attempt_id,ordinal,source,source_version,kind,normalized_value,strength,reason,source_hash,created_at_us)
SELECT id,attempt_id,ordinal,source,source_version,kind,normalized_value,strength,reason,source_hash,created_at_us
FROM identification_evidence;

DROP TABLE identification_evidence;
ALTER TABLE identification_evidence_next RENAME TO identification_evidence;

CREATE TRIGGER identification_evidence_immutable
BEFORE UPDATE ON identification_evidence
BEGIN SELECT RAISE(ABORT, 'identification evidence is immutable'); END;

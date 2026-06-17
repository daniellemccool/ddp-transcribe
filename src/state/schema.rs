pub const SCHEMA_VERSION: &str = "2";

pub const SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS videos (
    -- TEXT PRIMARY KEY does NOT imply NOT NULL in SQLite (only INTEGER PRIMARY
    -- KEY does, as a rowid alias). Declare NOT NULL explicitly. Guarded by
    -- state::tests::null_video_id_rejected_by_videos_schema.
    video_id            TEXT PRIMARY KEY NOT NULL,
    source_url          TEXT NOT NULL,
    canonical           INTEGER NOT NULL,
    status              TEXT NOT NULL CHECK (status IN
                          ('pending','in_progress','succeeded','failed_terminal','failed_retryable')),
    claimed_by          TEXT,
    claimed_at          INTEGER,
    attempt_count       INTEGER NOT NULL DEFAULT 0,
    succeeded_at        INTEGER,
    duration_s          REAL,
    language_detected   TEXT,
    fetcher             TEXT,
    transcript_source   TEXT,
    -- Plan B Epic 2: failure classification columns (0022, 0023).
    -- String-typed today per 0023; Epic 3's typed enums serialize into
    -- the same columns via tag()/message() projections.
    last_retryable_kind     TEXT,
    last_retryable_message  TEXT,
    terminal_reason         TEXT,
    terminal_message        TEXT,
    first_seen_at       INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_videos_pending
    ON videos (status, first_seen_at, video_id)
    WHERE status = 'pending';

CREATE TABLE IF NOT EXISTS watch_history (
    respondent_id  TEXT NOT NULL,
    video_id       TEXT NOT NULL,
    watched_at     INTEGER NOT NULL,
    in_window      INTEGER NOT NULL,
    PRIMARY KEY (respondent_id, video_id, watched_at),
    FOREIGN KEY (video_id) REFERENCES videos(video_id)
);
CREATE INDEX IF NOT EXISTS idx_watch_history_video ON watch_history (video_id);

CREATE TABLE IF NOT EXISTS video_events (
    id           INTEGER PRIMARY KEY,
    video_id     TEXT NOT NULL,
    at           INTEGER NOT NULL,
    event_type   TEXT NOT NULL,
    worker_id    TEXT,
    detail_json  TEXT,
    FOREIGN KEY (video_id) REFERENCES videos(video_id)
);
CREATE INDEX IF NOT EXISTS idx_video_events_video ON video_events (video_id, at);

CREATE TABLE IF NOT EXISTS meta (
    -- See videos.video_id comment for the NOT NULL rationale.
    -- Guarded by state::tests::null_meta_key_rejected_by_meta_schema.
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
";

pub const SCHEMA_VERSION: &str = "5";

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
    -- Plan B Epic 4c (schema v5): typed metadata columns populated by the
    -- post-run `load-metadata` subcommand from video_metadata_raw blobs.
    -- All nullable; NULL = never loaded. metadata_fetched_at records the
    -- capture moment (engagement counts are point-in-time snapshots).
    video_description   TEXT,
    uploader            TEXT,
    uploader_id         TEXT,
    video_created_at    INTEGER,
    view_count          INTEGER,
    like_count          INTEGER,
    comment_count       INTEGER,
    captions_json       TEXT,
    metadata_fetched_at INTEGER,
    first_seen_at       INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_videos_pending_v3
    ON videos (status, attempt_count, first_seen_at, video_id)
    WHERE status = 'pending';

CREATE TABLE IF NOT EXISTS watch_history (
    respondent_id  TEXT NOT NULL,
    video_id       TEXT NOT NULL,
    watched_at     INTEGER NOT NULL,
    in_window      INTEGER NOT NULL,
    -- Plan B Epic 4b (schema v4): the verbatim DDP `Date` string, so a
    -- future timezone reinterpretation never requires re-ingest (see the
    -- Epic 4b timezone ADR). NULL = row ingested pre-v4; re-ingesting the
    -- same DDP file backfills it.
    watched_at_raw TEXT,
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

CREATE TABLE IF NOT EXISTS batch_runs (
    run_id       INTEGER PRIMARY KEY,
    started_at   INTEGER NOT NULL,
    -- NULL means the run crashed or was interrupted before close — an
    -- honest record the operator can see.
    finished_at  INTEGER,
    params_json  TEXT NOT NULL,
    policy_toml  TEXT NOT NULL,
    census_json  TEXT
);

CREATE TABLE IF NOT EXISTS video_metadata_raw (
    -- Raw fetch-time metadata envelope (Epic 4c): versioned JSON wrapping
    -- yt-dlp's --print output UNPARSED plus any embedded caption tracks.
    -- One row per unique video, last-write-wins across retries. Parsed
    -- only by `load-metadata` — replayable without re-fetch.
    video_id   TEXT PRIMARY KEY NOT NULL,
    fetched_at INTEGER NOT NULL,
    raw_json   TEXT NOT NULL,
    FOREIGN KEY (video_id) REFERENCES videos(video_id)
);
";

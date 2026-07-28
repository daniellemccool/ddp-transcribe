use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "ddp-transcribe",
    version,
    about = "TikTok donation pipeline (Plan A walking skeleton)"
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug, Clone)]
pub struct GlobalArgs {
    #[arg(long, value_enum, default_value_t = Profile::Dev, env = "DDP_TRANSCRIBE_PROFILE")]
    pub profile: Profile,

    #[arg(
        long,
        default_value = "./state.sqlite",
        env = "DDP_TRANSCRIBE_STATE_DB"
    )]
    pub state_db: PathBuf,

    #[arg(long, default_value = "./inbox", env = "DDP_TRANSCRIBE_INBOX")]
    pub inbox: PathBuf,

    #[arg(
        long,
        default_value = "./transcripts",
        env = "DDP_TRANSCRIBE_TRANSCRIPTS"
    )]
    pub transcripts: PathBuf,

    #[arg(long, value_enum, default_value_t = LogFormat::Human, env = "DDP_TRANSCRIBE_LOG_FORMAT")]
    pub log_format: LogFormat,

    /// Path to the whisper.cpp model file. Overrides the profile default.
    #[arg(long, env = "DDP_TRANSCRIBE_WHISPER_MODEL")]
    pub whisper_model: Option<PathBuf>,

    /// Path to a classification-policy TOML (Epic 4a). Absent → the
    /// evidence-derived compiled default.
    #[arg(long, env = "DDP_TRANSCRIBE_CLASSIFICATION")]
    pub classification: Option<PathBuf>,

    /// Compute per-language probability distribution per video.
    /// Costs one extra encoder pass per video; default false.
    #[arg(long, env = "DDP_TRANSCRIBE_COMPUTE_LANG_PROBS", global = true)]
    pub compute_lang_probs: bool,

    /// Threshold for sweeping stale (process-crashed) claims back to pending.
    /// Accepts humantime strings: "30m" (default), "1h", "45s".
    /// 0024: 30-min default is well above bake worst-case (~25s).
    #[arg(
        long,
        env = "DDP_TRANSCRIBE_STALE_CLAIM_THRESHOLD",
        value_parser = humantime::parse_duration
    )]
    pub stale_claim_threshold: Option<std::time::Duration>,

    /// Number of parallel fetch workers in the pipelined orchestrator.
    /// 0027: default 3 (curve-flattening point on the bake throughput
    /// math; ~3.5× serial wallclock on news_orgs fixture). Must be ≥ 1.
    #[arg(
        long,
        env = "DDP_TRANSCRIBE_DOWNLOAD_WORKERS",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
    )]
    pub download_workers: Option<usize>,

    /// Bounded mpsc capacity between fetch workers and the transcribe
    /// worker. 0027: default 2 (small backpressure smoothing for
    /// transcribe's ~1s variance; peak channel memory ~6 × 3 MB = 18 MB
    /// at default N=3 + capacity 2). Must be ≥ 1.
    #[arg(
        long,
        env = "DDP_TRANSCRIBE_CHANNEL_CAPACITY",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
    )]
    pub channel_capacity: Option<usize>,
}

pub(crate) fn parse_window_date(s: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| format!("invalid date {s:?} (expected YYYY-MM-DD): {e}"))
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create state.sqlite and apply schema. Idempotent.
    Init,
    /// Walk --inbox, parse DDP JSONs, upsert into videos and watch_history.
    Ingest {
        #[arg(long)]
        dry_run: bool,
        /// Inclusive analysis-window start (YYYY-MM-DD, UTC). Rows outside
        /// the window ingest with in_window = 0. Absent = unbounded.
        #[arg(long, value_parser = parse_window_date)]
        window_start: Option<chrono::NaiveDate>,
        /// Inclusive analysis-window end (YYYY-MM-DD, UTC; covers that
        /// whole day). Absent = unbounded.
        #[arg(long, value_parser = parse_window_date)]
        window_end: Option<chrono::NaiveDate>,
    },
    /// Run a batch: claim pending videos, fetch + transcribe, write artifacts.
    Process {
        #[arg(long)]
        max_videos: Option<usize>,
        /// Netscape-format cookie file passed to yt-dlp ONLY on retries of
        /// sensitive/login-gated videos (ADR 0035). Never sent on first attempts.
        #[arg(long, env = "DDP_TRANSCRIBE_COOKIES_FILE")]
        cookies_file: Option<PathBuf>,
        /// Automatic in-batch retry budget per video (lifetime attempts =
        /// retries + 1). Default 1.
        #[arg(long, default_value_t = 1)]
        retries: i64,
    },
    /// Upgrade a pre-Epic-2 (v1) state.sqlite to the current schema version.
    /// Idempotent: no-op if already at the current version.
    Migrate,
    /// Report pipeline state: counts by status, failure breakdowns,
    /// current claims, and batch-run history. Read-only.
    Status {
        /// Full event history for one video.
        #[arg(long, conflicts_with_all = ["respondent_id", "errors", "retryable"])]
        video_id: Option<String>,
        /// Per-respondent summary counts.
        #[arg(long, conflicts_with_all = ["errors", "retryable"])]
        respondent_id: Option<String>,
        /// List failed_terminal videos with terminal_reason / terminal_message.
        #[arg(long)]
        errors: bool,
        /// List failed_retryable videos with last_retryable_kind / _message.
        #[arg(long)]
        retryable: bool,
        /// Run the done-contract checks (artifact existence at the sharded
        /// paths + raw_signals.schema_version parse + pause-safe verdict).
        /// Reads the --transcripts tree; exits 1 when not pause-safe.
        #[arg(long, conflicts_with_all = ["video_id", "respondent_id", "errors", "retryable"])]
        verify: bool,
        /// Emit machine-readable JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
    /// Recompute watch_history.in_window from explicit window flags.
    /// One-shot; does not re-read DDP files. Refuses to run bare —
    /// silently wiping the study's window filter must be impossible.
    #[command(group(clap::ArgGroup::new("window").required(true).multiple(true)))]
    RecomputeWindow {
        /// Inclusive analysis-window start (YYYY-MM-DD, UTC).
        #[arg(long, value_parser = parse_window_date, group = "window")]
        window_start: Option<chrono::NaiveDate>,
        /// Inclusive analysis-window end (YYYY-MM-DD, UTC; covers that whole day).
        #[arg(long, value_parser = parse_window_date, group = "window")]
        window_end: Option<chrono::NaiveDate>,
        /// Explicitly opt into "no filter": set in_window = 1 for ALL rows.
        #[arg(long, group = "window", conflicts_with_all = ["window_start", "window_end"])]
        clear: bool,
        /// Report how many rows would change, without writing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Dev,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum LogFormat {
    Human,
    Json,
}

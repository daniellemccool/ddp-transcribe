use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "ddp-transcribe",
    version,
    about = "Video-transcription pipeline for data-donation studies (TikTok DDP)"
)]
pub struct Cli {
    #[command(flatten)]
    pub(crate) global: GlobalArgs,

    #[command(subcommand)]
    pub(crate) command: Command,
}

impl Cli {
    /// The one field `main` needs before dispatch (0045): tracing init has to
    /// run ahead of any library work, and this narrow accessor buys that
    /// access without making `Cli`'s fields — or `GlobalArgs` — public API.
    pub fn log_format(&self) -> LogFormat {
        self.global.log_format
    }
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct GlobalArgs {
    #[arg(long, value_enum, default_value_t = Profile::Dev, env = "DDP_TRANSCRIBE_PROFILE", global = true)]
    pub profile: Profile,

    #[arg(
        long,
        default_value = "./state.sqlite",
        env = "DDP_TRANSCRIBE_STATE_DB",
        global = true
    )]
    pub state_db: PathBuf,

    #[arg(
        long,
        default_value = "./inbox",
        env = "DDP_TRANSCRIBE_INBOX",
        global = true
    )]
    pub inbox: PathBuf,

    #[arg(
        long,
        default_value = "./transcripts",
        env = "DDP_TRANSCRIBE_TRANSCRIPTS",
        global = true
    )]
    pub transcripts: PathBuf,

    #[arg(long, value_enum, default_value_t = LogFormat::Human, env = "DDP_TRANSCRIBE_LOG_FORMAT", global = true)]
    pub log_format: LogFormat,

    /// Path to the whisper.cpp model file. Overrides the profile default.
    #[arg(long, env = "DDP_TRANSCRIBE_WHISPER_MODEL", global = true)]
    pub whisper_model: Option<PathBuf>,

    /// Path to a classification-policy TOML (Epic 4a). Absent → the
    /// evidence-derived compiled default.
    #[arg(long, env = "DDP_TRANSCRIBE_CLASSIFICATION", global = true)]
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
        value_parser = humantime::parse_duration,
        global = true
    )]
    pub stale_claim_threshold: Option<std::time::Duration>,

    /// Number of parallel fetch workers in the pipelined orchestrator.
    /// 0027: default 3 (curve-flattening point on the bake throughput
    /// math; ~3.5× serial wallclock on news_orgs fixture). Must be ≥ 1.
    #[arg(
        long,
        env = "DDP_TRANSCRIBE_DOWNLOAD_WORKERS",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        global = true
    )]
    pub download_workers: Option<usize>,

    /// Bounded mpsc capacity between fetch workers and the transcribe
    /// worker. 0027: default 2 (small backpressure smoothing for
    /// transcribe's ~1s variance; peak channel memory ~6 × 3 MB = 18 MB
    /// at default N=3 + capacity 2). Must be ≥ 1.
    #[arg(
        long,
        env = "DDP_TRANSCRIBE_CHANNEL_CAPACITY",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..),
        global = true
    )]
    pub channel_capacity: Option<usize>,
}

pub(crate) fn parse_window_date(s: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| format!("invalid date {s:?} (expected YYYY-MM-DD): {e}"))
}

/// Reject a reversed window range (operator typo) before it reaches the
/// store: a `start > end` window makes the in_window CASE predicate
/// unsatisfiable, silently zeroing every row instead of failing loudly.
/// Equal dates are a valid single-day window. `command` names the
/// subcommand for the error message (e.g. "ingest", "recompute-window").
pub(crate) fn validate_window_order(
    command: &str,
    window_start: Option<chrono::NaiveDate>,
    window_end: Option<chrono::NaiveDate>,
) -> anyhow::Result<()> {
    if let (Some(start), Some(end)) = (window_start, window_end) {
        if start > end {
            anyhow::bail!("{command}: --window-start {start} is after --window-end {end}");
        }
    }
    Ok(())
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
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
        /// retries + 1). Default 1. Range-bounded at parse time: negative
        /// values would silently zero the budget and i64::MAX would
        /// overflow at `retries + 1` (epic-4 followup).
        #[arg(
            long,
            default_value_t = 1,
            value_parser = clap::builder::RangedI64ValueParser::<i64>::new().range(0..=1_000_000)
        )]
        retries: i64,
        /// Operator checkpoint hook: run this command every
        /// --checkpoint-every while the batch is running (e.g.
        /// ~/sync-to-storage.sh). Failures warn and count; they never
        /// stop the run. Absent = no in-run checkpointing.
        #[arg(long)]
        checkpoint_cmd: Option<PathBuf>,
        /// Interval between checkpoint hook runs, and the hook's own
        /// timeout. Accepts humantime strings: "15m" (default), "1h".
        /// Requires --checkpoint-cmd.
        #[arg(
            long,
            default_value = "15m",
            value_parser = humantime::parse_duration,
            requires = "checkpoint_cmd"
        )]
        checkpoint_every: std::time::Duration,
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
    /// Parse captured raw metadata (video_metadata_raw) into the typed
    /// videos columns. Post-run; idempotent and replayable — re-running
    /// overwrites from the current blobs, so a parser fix needs no re-fetch.
    LoadMetadata {
        /// Examine and parse everything, write nothing, report counts.
        #[arg(long)]
        dry_run: bool,
    },
    /// Operator override of retry eligibility (0046): restore failed rows to
    /// `pending` after an external condition materially changed. Forensic and
    /// DEFAULT-DENY — at least one qualifying selector (--error-kind /
    /// --max-attempts / --older-than) or an explicit --all is required; the
    /// modifiers --max and --dry-run never grant eligibility. Never resets
    /// `attempt_count`; every moved row leaves an `operator_requeued` event.
    ///
    /// Post-override arithmetic: a row at `attempt_count = A` gets exactly one
    /// forced attempt unless the next `process` run uses `--retries > A`.
    // Grammar lives in clap, not in post-parse checks: `eligibility` is the
    // required group (the default-deny gate), `qualifying` is the subset that
    // --all conflicts with and that --include-terminal requires.
    #[command(group(clap::ArgGroup::new("qualifying").multiple(true)))]
    #[command(group(clap::ArgGroup::new("eligibility").required(true).multiple(true)))]
    RequeueFailures {
        /// Failure kind to match, compared by exact byte equality (no case
        /// folding, no comma splitting — classification labels may legally
        /// contain commas). Repeatable; repeats OR together. Matches
        /// last_retryable_kind on retryable rows, terminal_reason on terminal
        /// ones — never a terminal row's retained retryable kind.
        #[arg(long = "error-kind", value_name = "KIND", groups = ["qualifying", "eligibility"])]
        error_kind: Vec<String>,
        /// Skip rows with attempt_count >= N.
        #[arg(
            long,
            value_name = "N",
            value_parser = clap::builder::RangedU64ValueParser::<u32>::new().range(1..),
            groups = ["qualifying", "eligibility"],
        )]
        max_attempts: Option<u32>,
        /// Match rows whose last FAILURE event (allowlist: failed_retryable,
        /// failed_terminal, retry_requeued, cookie_parked) is strictly older
        /// than this. Administrative events never reset that clock, and a row
        /// with no allowlist event never matches. Accepts humantime strings:
        /// "30d", "12h".
        #[arg(
            long,
            value_name = "DUR",
            value_parser = humantime::parse_duration,
            groups = ["qualifying", "eligibility"],
        )]
        older_than: Option<std::time::Duration>,
        /// Also consider failed_terminal rows. Opt-in twice over: it requires a
        /// qualifying selector alongside it, so --include-terminal with --all
        /// (or with --max alone) is a usage error.
        #[arg(long, requires = "qualifying")]
        include_terminal: bool,
        /// Every failed_retryable row — never terminals. Conflicts with every
        /// qualifying selector: `--all --older-than 30d` is a usage error, not
        /// a silent intersection.
        #[arg(long, group = "eligibility", conflicts_with = "qualifying")]
        all: bool,
        /// Cap the number of rows moved, taken in the deterministic order
        /// attempt_count ASC, video_id ASC. A modifier: it never grants
        /// eligibility on its own.
        #[arg(
            long,
            value_name = "N",
            value_parser = clap::builder::RangedU64ValueParser::<u32>::new().range(1..),
        )]
        max: Option<u32>,
        /// Report per-kind counts for what would move, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Backfill raw metadata (video_metadata_raw) for succeeded videos
    /// that predate fetch-time capture. Metadata-only yt-dlp per video —
    /// no media download, never touches video status. Best-effort and
    /// re-runnable; run `load-metadata` afterwards to fill the typed
    /// columns.
    BackfillMetadata {
        /// Cap the number of videos attempted (smoke runs).
        #[arg(long)]
        limit: Option<u64>,
        /// Print the cohort size and exit without invoking yt-dlp.
        /// Conflicts with --limit: dry-run invokes nothing, so a cap on
        /// attempts has nothing to cap. Previously the pair parsed and
        /// --limit was silently ignored (documented only in the runbook);
        /// clap now says so itself.
        #[arg(long, conflicts_with = "limit")]
        dry_run: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
    Dev,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum LogFormat {
    Human,
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clap_definition_is_internally_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    /// v0.3.1 CLI hardening bundle: `tests/cli.rs`'s both-position test
    /// proves only that clap ACCEPTS a `GlobalArgs` flag after the
    /// subcommand. These pin the two behaviors that acceptance leaves
    /// unproven, and they live here rather than in `tests/cli.rs` because
    /// `Cli::global` is `pub(crate)` (0045) — reading the parsed field is
    /// only possible in-crate. `try_parse_from` also means no process spawn.
    ///
    /// 1. VALUE PROPAGATION: the value given after the subcommand actually
    ///    reaches `GlobalArgs`, rather than the field keeping its default.
    #[test]
    fn global_flag_value_propagates_from_the_post_subcommand_position() {
        let cli = Cli::try_parse_from([
            "ddp-transcribe",
            "status",
            "--state-db",
            "/after/state.sqlite",
            "--transcripts",
            "/after/transcripts",
            "--inbox",
            "/after/inbox",
            "--download-workers",
            "7",
            "--stale-claim-threshold",
            "45s",
        ])
        .expect("post-subcommand globals must parse");
        assert_eq!(cli.global.state_db, PathBuf::from("/after/state.sqlite"));
        assert_eq!(
            cli.global.transcripts,
            PathBuf::from("/after/transcripts"),
            "the post-subcommand value must not be shadowed by the default"
        );
        assert_eq!(cli.global.inbox, PathBuf::from("/after/inbox"));
        assert_eq!(cli.global.download_workers, Some(7));
        assert_eq!(
            cli.global.stale_claim_threshold,
            Some(std::time::Duration::from_secs(45))
        );
        // Untouched globals still hold their declared defaults — the
        // assertions above are about propagation, not about the whole
        // struct being rebuilt from the subcommand's matches.
        assert!(matches!(cli.global.profile, Profile::Dev));
        assert!(matches!(cli.global.log_format, LogFormat::Human));
    }

    /// 2. DUPLICATE PRECEDENCE: the same global flag given both before AND
    ///    after the subcommand. Clap resolves this last-occurrence-wins;
    ///    this repo has never pinned that inherited behavior, so an operator
    ///    who writes `--state-db A ... process --state-db B` gets B.
    #[test]
    fn duplicate_global_flag_takes_the_last_occurrence() {
        let cli = Cli::try_parse_from([
            "ddp-transcribe",
            "--state-db",
            "/before/state.sqlite",
            "--download-workers",
            "1",
            "status",
            "--state-db",
            "/after/state.sqlite",
            "--download-workers",
            "9",
        ])
        .expect("a duplicated global is not a usage error");
        assert_eq!(
            cli.global.state_db,
            PathBuf::from("/after/state.sqlite"),
            "the post-subcommand occurrence wins"
        );
        assert_eq!(cli.global.download_workers, Some(9));

        // Pre-subcommand-only still reaches the field (the baseline the
        // duplicate case is measured against).
        let cli = Cli::try_parse_from([
            "ddp-transcribe",
            "--state-db",
            "/before/state.sqlite",
            "status",
        ])
        .expect("pre-subcommand globals must parse");
        assert_eq!(cli.global.state_db, PathBuf::from("/before/state.sqlite"));
    }
}

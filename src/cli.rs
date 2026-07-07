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

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create state.sqlite and apply schema. Idempotent.
    Init,
    /// Walk --inbox, parse DDP JSONs, upsert into videos and watch_history.
    Ingest {
        #[arg(long)]
        dry_run: bool,
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
    /// Adjudicate failed_retryable rows: write-off classes → failed_terminal;
    /// probe the rest via TikTok oEmbed (dead → terminal, alive → pending under
    /// the attempt cap). Requires `curl` on PATH. Run `process` afterwards.
    Triage {
        /// Probe and report the census without mutating any rows.
        #[arg(long)]
        dry_run: bool,
        /// oEmbed probes per second. Must be > 0 — a non-positive rate
        /// previously clamped silently to a 1000s/probe crawl (final
        /// review, Epic 3 close); now rejected outright.
        #[arg(long, default_value_t = 1.0, value_parser = parse_positive_rate)]
        rate: f64,
        /// Rows at or above this attempt_count are not requeued.
        #[arg(long, default_value_t = 3)]
        max_attempts: i64,
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

/// `--rate`'s `value_parser` (final review, Epic 3 close): rejects
/// non-positive rates at parse time rather than letting `triage.rs`'s
/// `.max(0.001)` clamp silently turn e.g. `--rate 0` into a 1000s/probe
/// crawl. No `RangedU64ValueParser`-equivalent exists for `f64` in clap, so
/// this is a small hand-rolled parser matching the `humantime::parse_duration`
/// pattern already used for `stale_claim_threshold` above.
fn parse_positive_rate(s: &str) -> Result<f64, String> {
    let v: f64 = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    if v > 0.0 {
        Ok(v)
    } else {
        Err(format!("--rate must be greater than 0, got {v}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        let mut full = vec!["ddp-transcribe"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full)
    }

    #[test]
    fn triage_rejects_zero_rate() {
        let err = parse(&["triage", "--rate", "0"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn triage_rejects_negative_rate() {
        let err = parse(&["triage", "--rate=-1"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn triage_accepts_positive_rate() {
        let cli = parse(&["triage", "--rate", "0.5"]).unwrap();
        match cli.command {
            Command::Triage { rate, .. } => assert_eq!(rate, 0.5),
            other => panic!("expected Triage, got {other:?}"),
        }
    }
}

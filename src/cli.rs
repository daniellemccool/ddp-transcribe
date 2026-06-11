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
    },
    /// Upgrade a pre-Epic-2 (v1) state.sqlite to the current schema version.
    /// Idempotent: no-op if already at the current version.
    Migrate,
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

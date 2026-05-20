use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "uu-tiktok",
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
    #[arg(long, value_enum, default_value_t = Profile::Dev, env = "UU_TIKTOK_PROFILE")]
    pub profile: Profile,

    #[arg(long, default_value = "./state.sqlite", env = "UU_TIKTOK_STATE_DB")]
    pub state_db: PathBuf,

    #[arg(long, default_value = "./inbox", env = "UU_TIKTOK_INBOX")]
    pub inbox: PathBuf,

    #[arg(long, default_value = "./transcripts", env = "UU_TIKTOK_TRANSCRIPTS")]
    pub transcripts: PathBuf,

    #[arg(long, value_enum, default_value_t = LogFormat::Human, env = "UU_TIKTOK_LOG_FORMAT")]
    pub log_format: LogFormat,

    /// Path to the whisper.cpp model file. Overrides the profile default.
    #[arg(long, env = "UU_TIKTOK_WHISPER_MODEL")]
    pub whisper_model: Option<PathBuf>,

    /// Compute per-language probability distribution per video.
    /// Costs one extra encoder pass per video; default false.
    #[arg(long, env = "UU_TIKTOK_COMPUTE_LANG_PROBS", global = true)]
    pub compute_lang_probs: bool,
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

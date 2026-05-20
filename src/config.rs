use std::path::PathBuf;
use std::time::Duration;

use crate::cli::{GlobalArgs, Profile};

#[derive(Debug, Clone)]
pub struct Config {
    pub profile: Profile,
    pub state_db: PathBuf,
    pub inbox: PathBuf,
    pub transcripts: PathBuf,

    /// Path to the whisper.cpp model file. Plan A defaults to a tiny.en model
    /// that the operator places at this path before running `process`.
    pub whisper_model_path: PathBuf,

    // Plan A leftovers — Plan B's `WhisperEngine` does not consume these
    // (whisper-rs picks `n_threads = min(4, hw_concurrency)` itself, and the
    // GPU choice is an `i32` device index passed via `EngineConfig`). They
    // still have CLI/env plumbing and per-field tests in `config.rs::tests`;
    // deletion is a separate cleanup sweep (likely Epic 2 alongside the
    // state-machine work). Suppressing dead_code while they're plumbed but
    // unread keeps the diff minimal.
    #[allow(dead_code)]
    pub whisper_use_gpu: bool,
    #[allow(dead_code)]
    pub whisper_threads: usize,

    pub ytdlp_timeout: Duration,
    pub transcribe_timeout: Duration,
    pub compute_lang_probs: bool,
    /// Stale-claim sweep threshold (0024). Default 30 min if CLI/env
    /// did not supply a value.
    pub stale_claim_threshold: Duration,
    /// 0027: default 3 (curve-flattening point).
    pub download_workers: usize,
    /// 0027: default 2 (small backpressure smoothing).
    pub channel_capacity: usize,
}

impl Config {
    pub fn from_args(args: &GlobalArgs) -> Self {
        match args.profile {
            Profile::Dev => Self {
                profile: Profile::Dev,
                state_db: args.state_db.clone(),
                inbox: args.inbox.clone(),
                transcripts: args.transcripts.clone(),
                whisper_model_path: args
                    .whisper_model
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("./models/ggml-tiny.en.bin")),
                whisper_use_gpu: false,
                whisper_threads: num_cpus_safe(),
                ytdlp_timeout: Duration::from_secs(300),
                transcribe_timeout: Duration::from_secs(600),
                compute_lang_probs: args.compute_lang_probs,
                stale_claim_threshold: args
                    .stale_claim_threshold
                    .unwrap_or_else(|| Duration::from_secs(30 * 60)),
                download_workers: args.download_workers.unwrap_or(3),
                channel_capacity: args.channel_capacity.unwrap_or(2),
            },
        }
    }
}

fn num_cpus_safe() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_args() -> GlobalArgs {
        GlobalArgs {
            profile: Profile::Dev,
            state_db: PathBuf::from("/tmp/test.sqlite"),
            inbox: PathBuf::from("/tmp/in"),
            transcripts: PathBuf::from("/tmp/out"),
            log_format: crate::cli::LogFormat::Human,
            whisper_model: None,
            compute_lang_probs: false,
            stale_claim_threshold: None,
            download_workers: None,
            channel_capacity: None,
        }
    }

    #[test]
    fn dev_profile_uses_tiny_en_cpu() {
        let cfg = Config::from_args(&dev_args());
        assert!(cfg.whisper_model_path.to_string_lossy().contains("tiny.en"));
        assert!(!cfg.whisper_use_gpu);
        assert!(cfg.whisper_threads >= 1);
        assert_eq!(cfg.ytdlp_timeout, Duration::from_secs(300));
    }

    #[test]
    fn paths_pass_through_from_args() {
        let cfg = Config::from_args(&dev_args());
        assert_eq!(cfg.inbox, PathBuf::from("/tmp/in"));
        assert_eq!(cfg.transcripts, PathBuf::from("/tmp/out"));
        assert_eq!(cfg.state_db, PathBuf::from("/tmp/test.sqlite"));
    }

    #[test]
    fn whisper_model_override_takes_precedence_over_profile_default() {
        let mut args = dev_args();
        args.whisper_model = Some(PathBuf::from("/custom/ggml-small.bin"));
        let cfg = Config::from_args(&args);
        assert_eq!(
            cfg.whisper_model_path,
            PathBuf::from("/custom/ggml-small.bin")
        );
    }

    #[test]
    fn default_stale_claim_threshold_is_30_min() {
        let cfg = Config::from_args(&dev_args());
        assert_eq!(cfg.stale_claim_threshold, Duration::from_secs(30 * 60));
    }

    #[test]
    fn stale_claim_threshold_parsed_from_args() {
        use std::str::FromStr;
        let mut args = dev_args();
        args.stale_claim_threshold = Some(humantime::Duration::from_str("5m").unwrap().into());
        let cfg = Config::from_args(&args);
        assert_eq!(cfg.stale_claim_threshold, Duration::from_secs(5 * 60));
    }

    #[test]
    fn default_download_workers_is_3_per_ad0027() {
        let cfg = Config::from_args(&dev_args());
        assert_eq!(cfg.download_workers, 3);
    }

    #[test]
    fn default_channel_capacity_is_2_per_ad0027() {
        let cfg = Config::from_args(&dev_args());
        assert_eq!(cfg.channel_capacity, 2);
    }

    #[test]
    fn download_workers_override_flows_through() {
        let mut args = dev_args();
        args.download_workers = Some(5);
        let cfg = Config::from_args(&args);
        assert_eq!(cfg.download_workers, 5);
    }

    #[test]
    fn channel_capacity_override_flows_through() {
        let mut args = dev_args();
        args.channel_capacity = Some(8);
        let cfg = Config::from_args(&args);
        assert_eq!(cfg.channel_capacity, 8);
    }
}

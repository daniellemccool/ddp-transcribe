#![allow(clippy::unwrap_used, clippy::expect_used)]

//! End-to-end real-tools test for Plan B Epic 1. Exercises the embedded
//! whisper-rs pipeline against a real TikTok URL via yt-dlp.
//!
//! Requires (per 0009, 0011):
//! - yt-dlp on PATH
//! - ffmpeg on PATH (yt-dlp's postprocessor — 16 kHz mono WAV per 0014)
//! - ./models/ggml-tiny.en.bin on disk (or DDP_TRANSCRIBE_WHISPER_MODEL=PATH)
//! - clang/libclang installed (whisper-rs's whisper-rs-sys binds via bindgen)
//! - cmake on PATH (whisper-rs's build script invokes it)
//! - a working C/C++ toolchain (gcc or clang, plus libstdc++)
//! - CUDA toolkit for `--features cuda` builds (A10-bake runbook covers details)
//! - network egress to the TikTok CDN
//!
//! Run manually during the SRC A10 bake (Task 13):
//!   cargo test --release --features test-helpers,cuda \
//!     --test e2e_real_tools -- --ignored --nocapture
//!
//! Without --features cuda, falls back to whisper-rs's CPU build
//! (functional but slow; not suitable for the bake's throughput numbers).

use std::path::PathBuf;
use std::process::Command;

use ddp_transcribe::output::artifacts::EXPECTED_RAW_SIGNALS_SCHEMA_VERSION;
use tempfile::TempDir;

/// The URL both ignored tests fetch. Override with `DDP_TRANSCRIBE_E2E_URL`;
/// the compiled-in default is a placeholder ID, so a real manual run must
/// set the variable to a known-good public TikTok video.
fn e2e_url() -> String {
    std::env::var("DDP_TRANSCRIBE_E2E_URL")
        .unwrap_or_else(|_| "https://www.tiktokv.com/share/video/7234567890123456789/".into())
}

#[test]
#[ignore]
fn end_to_end_one_known_url() {
    let tmp = TempDir::new().unwrap();
    let inbox = tmp.path().join("inbox");
    let transcripts = tmp.path().join("transcripts");
    std::fs::create_dir_all(&inbox).unwrap();

    // Construct a single-row donation-extractor JSON file. Replace the URL below
    // with a known long-lived public TikTok video; document the choice in README.
    let respondent_file =
        inbox.join("assignment=1_task=1_participant=test_source=tiktok_key=1-tiktok.json");
    let url = e2e_url();
    let payload = format!(
        r#"[
            {{"tiktok_watch_history": [
                {{"Date": "2024-01-01 00:00:00", "Link": "{url}"}}
            ], "deleted row count": "0"}}
        ]"#
    );
    std::fs::write(&respondent_file, payload).unwrap();

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_ddp-transcribe"));
    let db = tmp.path().join("state.sqlite");

    let run = |args: &[&str]| {
        let status = Command::new(&bin)
            .args(["--state-db", db.to_str().unwrap()])
            .args(["--inbox", inbox.to_str().unwrap()])
            .args(["--transcripts", transcripts.to_str().unwrap()])
            .args(args)
            .status()
            .expect("ddp-transcribe runs");
        assert!(status.success(), "ddp-transcribe {args:?} failed");
    };

    run(&["init"]);
    run(&["ingest"]);
    run(&["process", "--max-videos", "1"]);

    // .txt sanity check (kept from Plan A): some transcript text exists.
    let mut found_txt = false;
    for entry in walkdir(&transcripts) {
        if entry.extension().and_then(|s| s.to_str()) == Some("txt") {
            let body = std::fs::read_to_string(&entry).unwrap();
            assert!(
                !body.trim().is_empty(),
                "transcript at {} is empty",
                entry.display()
            );
            found_txt = true;
        }
    }
    assert!(found_txt, "no .txt transcript produced");

    // Plan B Epic 1 (T12): .json artifact shape per 0010.
    //
    // Real audio content is non-deterministic and TikTok content drifts. Assert
    // structural shape per 0010 (schema_version, ranges, types) rather than
    // specific token text / probabilities / segment counts.
    let json_paths: Vec<_> = walkdir(&transcripts)
        .into_iter()
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    assert_eq!(
        json_paths.len(),
        1,
        "expected exactly one .json transcript artifact for a single-video run, got {}: {:?}",
        json_paths.len(),
        json_paths
    );

    let json_text = std::fs::read_to_string(&json_paths[0]).expect("read json artifact");
    let parsed: serde_json::Value = serde_json::from_str(&json_text).expect("parse json");

    // Provenance: trait name() values per T11 (no more hardcoded strings).
    assert_eq!(parsed["fetcher"], "ytdlp");
    assert_eq!(parsed["transcript_source"], "whisper-rs");
    assert!(
        !parsed["model"].as_str().unwrap_or("").is_empty(),
        "model field should be a non-empty string (engine returns the model file basename)"
    );

    // raw_signals: 0010 contract.
    let rs = &parsed["raw_signals"];
    assert!(!rs.is_null(), "raw_signals must be present");
    // Two assertions on schema_version:
    //   1. Match the implementation constant (drift detector — if the const
    //      changes, the test catches the desync between impl and bake).
    //   2. Match the literal "1" (wire-contract lock — schema v1 must always
    //      serialize as the string "1" regardless of what the const says).
    assert_eq!(rs["schema_version"], EXPECTED_RAW_SIGNALS_SCHEMA_VERSION);
    assert_eq!(rs["schema_version"], "1");
    assert!(
        !rs["language"].as_str().unwrap_or("").is_empty(),
        "language must be a non-empty ISO code"
    );

    let segments = rs["segments"].as_array().expect("segments array");
    assert!(
        !segments.is_empty(),
        "real audio should produce at least one segment"
    );
    let mut total_tokens = 0_usize;
    for (i, seg) in segments.iter().enumerate() {
        let no_speech = seg["no_speech_prob"]
            .as_f64()
            .expect("no_speech_prob is number");
        assert!(
            (0.0..=1.0).contains(&no_speech),
            "segment {i} no_speech_prob out of range: {no_speech}"
        );
        let tokens = seg["tokens"].as_array().expect("tokens array");
        total_tokens += tokens.len();
        for (j, tok) in tokens.iter().enumerate() {
            let id = tok["id"].as_i64().expect("token id is integer");
            assert!(
                id >= 0,
                "segment {i} token {j} id should be non-negative, got {id}"
            );
            assert!(
                tok["text"].is_string(),
                "segment {i} token {j} text must be a string"
            );
            let p = tok["p"].as_f64().expect("token p is number");
            assert!(
                (0.0..=1.0).contains(&p),
                "segment {i} token {j} p out of range: {p}"
            );
            let plog = tok["plog"].as_f64().expect("token plog is number");
            assert!(
                plog <= 0.0001,
                "segment {i} token {j} plog should be non-positive log-prob, got {plog}"
            );
        }
    }
    assert!(
        total_tokens > 0,
        "real audio should produce at least one token across all segments — got {} segments with 0 tokens total",
        segments.len()
    );
}

/// Epic 4c: a real fetch captures a parseable metadata envelope. Ignored —
/// needs network + yt-dlp on PATH (no whisper model, no CUDA: this drives
/// `YtDlpFetcher::acquire` directly, not the full pipeline). Run manually:
///
/// ```text
/// DDP_TRANSCRIBE_E2E_URL=<real url> cargo test --test e2e_real_tools \
///     real_fetch_captures_metadata_envelope -- --ignored --test-threads=1
/// ```
///
/// This is also the designated re-verification hook after a yt-dlp upgrade:
/// the `--print` template's field names are yt-dlp's info-dict keys, and a
/// rename upstream would silently empty the envelope's typed columns.
#[tokio::test]
#[ignore]
async fn real_fetch_captures_metadata_envelope() {
    let tmp = TempDir::new().unwrap();
    let url = e2e_url();
    // Whatever ID the URL carries; the fetcher only uses it for the work dir
    // and the output template, and the assertions read `id` from yt-dlp's
    // own printed line rather than from this string.
    let video_id = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .expect("url has a last path segment")
        .to_string();

    let fetcher = ddp_transcribe::fetcher::ytdlp::YtDlpFetcher::new(
        tmp.path(),
        std::time::Duration::from_secs(300),
    );
    let opts = ddp_transcribe::fetcher::FetchOpts::default();

    let (capture, outcome) = {
        use ddp_transcribe::fetcher::VideoFetcher;
        fetcher.acquire(&video_id, &url, &opts).await
    };

    // The envelope rides BOTH the success and the classified-failure path, so
    // the metadata assertions do not depend on the download having succeeded.
    // The outcome is reported only for diagnosis.
    let capture = capture.unwrap_or_else(|| {
        panic!("no metadata envelope captured (fetch outcome: {outcome:?})");
    });

    let envelope: serde_json::Value =
        serde_json::from_str(&capture.envelope_json).expect("envelope is JSON");
    assert_eq!(
        envelope["schema"], 1,
        "envelope schema version must be 1: {}",
        capture.envelope_json
    );

    let printed_line = envelope["printed"]
        .as_str()
        .expect("envelope.printed is a string");
    let printed: serde_json::Value =
        serde_json::from_str(printed_line).expect("printed line is JSON");

    assert!(
        !printed["id"].as_str().unwrap_or("").is_empty(),
        "printed id must be a non-empty string: {printed_line}"
    );
    assert!(
        !printed["description"].is_null(),
        "printed description must not be null — it is the creator caption text \
         that populates videos.video_description: {printed_line}"
    );
}

fn walkdir(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

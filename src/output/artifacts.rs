use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ============================================================================
// Plan B Epic 1 (T10): TranscriptMetadata + raw_signals projection
// ============================================================================
//
// Per AD0010 (raw_signals schema, schema_version): the per-video JSON artifact
// at `{transcripts_root}/{shard}/{video_id}.json` carries Plan A's existing
// provenance fields (video_id, source_url, fetcher, transcript_source, model,
// transcribed_at, language_detected, duration_s) PLUS an optional
// `raw_signals` sub-object pass-through (schema_version, language,
// lang_probs, segments[]).
//
// Module dependency direction (AD0016 worker-thread invariants):
// `src/transcribe.rs` MUST NOT import from this module. The transcribe layer
// is the source-of-truth domain type; the artifacts layer is the consumer
// that knows how to project domain types into JSON. The conversion lives on
// THIS side as `RawSignals::from_transcribe_output(&TranscribeOutput)`.
//
// T11 will wire the actual construction site at `src/pipeline.rs` once the
// Plan A whisper-cli call path is replaced by the Plan B whisper-rs engine.
// T10 just freezes the artifact schema and makes the struct compile +
// serialize correctly.

/// On-wire raw_signals schema version. AD0010 + comment-2: this is a JSON
/// string ("1"), not an integer — string versioning admits additive minor
/// revisions ("1.1") without forcing a re-parse of existing artifacts.
pub const EXPECTED_RAW_SIGNALS_SCHEMA_VERSION: &str = "1";

/// Per-video JSON artifact metadata. Lifted from `src/pipeline.rs` (Plan A's
/// private borrowed-string struct) to owned `String` fields here so the type
/// derives `Deserialize` + `PartialEq` for tests and is reusable from
/// non-pipeline code paths.
///
/// The `model` field name replaces Plan A's `transcript_model` (Plan B
/// design). `raw_signals` is `Some(...)` post-T11: pipeline.rs constructs it
/// via `RawSignals::from_transcribe_output` on every successful video.
/// `skip_serializing_if` is retained for forward compatibility — a future
/// fetcher tier that yields a transcript without raw signals (e.g., a cached
/// ReadyTranscript variant) would set this to None.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptMetadata {
    pub video_id: String,
    pub source_url: String,
    pub duration_s: Option<f64>,
    pub language_detected: Option<String>,
    pub transcribed_at: String,
    pub fetcher: String,
    pub transcript_source: String,
    pub model: String,

    /// Plan B Epic 1 addition (T10). `None` during the T10→T11 interim
    /// while pipeline.rs still uses the Plan A adapter; `Some(...)` once
    /// T11 rewrites the call site to use the embedded whisper-rs engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_signals: Option<RawSignals>,
}

/// Pass-through raw confidence signals from whisper.cpp's C API.
/// See AD0010 for the schema contract; T9's `TranscribeOutput` is the
/// source-of-truth domain type that this projection consumes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawSignals {
    pub schema_version: String,
    pub language: String,
    /// AD0010: serialize as `null` when absent (NOT omitted) — opt-in
    /// `--compute-lang-probs` consumers depend on the field always being
    /// present. No `skip_serializing_if` here.
    pub lang_probs: Option<Vec<(String, f32)>>,
    pub segments: Vec<RawSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawSegment {
    pub no_speech_prob: f32,
    pub tokens: Vec<RawToken>,
}

/// Per-token raw confidence signals. Shape matches T9's `TokenRaw` 1:1 so
/// the projection round-trips `id` + `text` losslessly — downstream
/// consumers need both to filter special tokens (`[BEG]`, `[END]`, `<|en|>`,
/// etc.) per AD0010's pass-through rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawToken {
    pub id: i32,
    pub text: String,
    pub p: f32,
    pub plog: f32,
}

impl RawSignals {
    /// Project T9's `TranscribeOutput` domain type into the artifact-side
    /// schema. AD0016: the conversion lives on the artifact side so the
    /// transcribe module stays independent of the artifact module.
    pub fn from_transcribe_output(output: &crate::transcribe::TranscribeOutput) -> Self {
        RawSignals {
            schema_version: EXPECTED_RAW_SIGNALS_SCHEMA_VERSION.to_string(),
            language: output.language.clone(),
            lang_probs: output.lang_probs.clone(),
            segments: output
                .segments
                .iter()
                .map(|s| RawSegment {
                    no_speech_prob: s.no_speech_prob,
                    tokens: s
                        .tokens
                        .iter()
                        .map(|t| RawToken {
                            id: t.id,
                            text: t.text.clone(),
                            p: t.p,
                            plog: t.plog,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// Atomic write for one file: write to `{path}.tmp`, fsync, rename to `{path}`,
/// fsync the parent directory. Caller is responsible for parent existence.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("path {} has no parent", path.display()))?;

    let mut tmp_path = path.to_path_buf();
    let tmp_name = format!(
        "{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .with_context(|| format!("path {} has no filename", path.display()))?
    );
    tmp_path.set_file_name(tmp_name);

    {
        let mut f = File::create(&tmp_path)
            .with_context(|| format!("creating tmp file {}", tmp_path.display()))?;
        f.write_all(contents)
            .with_context(|| format!("writing tmp file {}", tmp_path.display()))?;
        f.sync_all()
            .with_context(|| format!("fsyncing tmp file {}", tmp_path.display()))?;
    }

    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;

    let dir = File::open(parent)
        .with_context(|| format!("opening parent dir {} for fsync", parent.display()))?;
    dir.sync_all()
        .with_context(|| format!("fsyncing parent dir {}", parent.display()))?;

    Ok(())
}

/// Sweep all `*.tmp` files under the transcripts root. Called at process
/// startup so leftover tmp files from crashed runs don't accumulate.
pub fn cleanup_tmp_files(transcripts_root: &Path) -> Result<usize> {
    if !transcripts_root.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(transcripts_root)
        .with_context(|| format!("reading transcripts root {}", transcripts_root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            for shard_entry in std::fs::read_dir(&path)? {
                let shard_entry = shard_entry?;
                let p = shard_entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("tmp") {
                    let _ = std::fs::remove_file(&p);
                    removed += 1;
                }
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_creates_file_and_no_tmp_remains() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("hello.txt");
        atomic_write(&target, b"world").expect("write succeeds");

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "world");
        let tmp_path = tmp.path().join("hello.txt.tmp");
        assert!(!tmp_path.exists(), "tmp file should be renamed away");
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("hello.txt");
        atomic_write(&target, b"first").unwrap();
        atomic_write(&target, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
    }

    // ------------------------------------------------------------------
    // Plan B Epic 1 (T10) — TranscriptMetadata + raw_signals projection
    // ------------------------------------------------------------------

    use serde_json::Value;

    fn sample_metadata_with_raw_signals() -> TranscriptMetadata {
        TranscriptMetadata {
            video_id: "7234567890123456789".to_string(),
            source_url: "https://www.tiktokv.com/share/video/7234567890123456789/".to_string(),
            duration_s: Some(23.4),
            language_detected: Some("en".to_string()),
            transcribed_at: "2026-05-12T13:45:22Z".to_string(),
            fetcher: "ytdlp".to_string(),
            transcript_source: "whisper-rs".to_string(),
            model: "ggml-tiny.en.bin".to_string(),
            raw_signals: Some(RawSignals {
                schema_version: "1".to_string(),
                language: "en".to_string(),
                lang_probs: None,
                segments: vec![RawSegment {
                    no_speech_prob: 0.02,
                    tokens: vec![RawToken {
                        id: 50257,
                        text: "\u{2581}hello".to_string(),
                        p: 0.94,
                        plog: -0.06,
                    }],
                }],
            }),
        }
    }

    #[test]
    fn metadata_serializes_with_raw_signals_object_and_null_lang_probs() {
        let meta = sample_metadata_with_raw_signals();
        let json: Value = serde_json::to_value(&meta).expect("serialize");
        let rs = &json["raw_signals"];

        // schema_version is the on-wire string "1"; assert the literal here
        // (using the constant would tautologize the wire-contract test).
        assert_eq!(rs["schema_version"], "1");
        assert_eq!(rs["language"], "en");

        // AD0010: lang_probs MUST be present as `null` when not opted in
        // (NOT omitted). serde_json::Value::Null serializes/deserializes
        // identically; we assert the key exists AND its value is JSON null
        // by checking `is_null()` on the looked-up value.
        assert!(
            rs.get("lang_probs").is_some(),
            "lang_probs key must be present"
        );
        assert!(
            rs["lang_probs"].is_null(),
            "lang_probs must serialize as null when None"
        );

        let segments = rs["segments"].as_array().expect("segments array");
        assert_eq!(segments.len(), 1);
        assert!((segments[0]["no_speech_prob"].as_f64().unwrap() - 0.02).abs() < 1e-6);
    }

    #[test]
    fn metadata_without_raw_signals_omits_field_on_wire() {
        let mut meta = sample_metadata_with_raw_signals();
        meta.raw_signals = None;
        let json: Value = serde_json::to_value(&meta).expect("serialize");
        // Outer `raw_signals: Option<RawSignals>` uses
        // `skip_serializing_if = "Option::is_none"`, so the field is absent
        // (not null) on the wire when None — keeps the JSON clean during
        // the T10→T11 bridge window before T11 wires the engine output.
        let obj = json.as_object().expect("top-level is a JSON object");
        assert!(
            !obj.contains_key("raw_signals"),
            "raw_signals key must be absent when None (T10→T11 bridge window)"
        );
    }

    #[test]
    fn raw_signals_from_transcribe_output_preserves_token_identity() {
        use crate::transcribe::{SegmentRaw, TokenRaw, TranscribeOutput};

        let output = TranscribeOutput {
            text: "hello".to_string(),
            language: "en".to_string(),
            lang_probs: None,
            segments: vec![SegmentRaw {
                no_speech_prob: 0.02,
                tokens: vec![TokenRaw {
                    id: 50257,
                    text: "\u{2581}hello".to_string(),
                    p: 0.94,
                    plog: -0.06,
                }],
            }],
            model_id: "ggml-tiny.en.bin".to_string(),
        };

        let rs = RawSignals::from_transcribe_output(&output);

        // schema_version is sourced from the module-level constant —
        // assert via the constant here so a future bump to "1.1" updates
        // the constant in one place.
        assert_eq!(rs.schema_version, EXPECTED_RAW_SIGNALS_SCHEMA_VERSION);
        assert_eq!(rs.language, output.language);
        assert_eq!(rs.lang_probs, output.lang_probs);
        assert_eq!(rs.segments.len(), 1);
        assert!((rs.segments[0].no_speech_prob - output.segments[0].no_speech_prob).abs() < 1e-6);

        assert_eq!(rs.segments[0].tokens.len(), 1);
        let projected = &rs.segments[0].tokens[0];
        let original = &output.segments[0].tokens[0];
        assert_eq!(projected.id, original.id);
        assert_eq!(projected.text, original.text);
        assert!((projected.p - original.p).abs() < 1e-6);
        assert!((projected.plog - original.plog).abs() < 1e-6);
    }

    #[test]
    fn cleanup_tmp_removes_tmp_files_in_shard_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Set up shard 89 with one tmp file and one real file.
        let shard_dir = root.join("89");
        std::fs::create_dir_all(&shard_dir).unwrap();
        std::fs::write(shard_dir.join("video.txt.tmp"), b"junk").unwrap();
        std::fs::write(shard_dir.join("video.txt"), b"real").unwrap();

        let removed = cleanup_tmp_files(root).unwrap();
        assert_eq!(removed, 1);
        assert!(!shard_dir.join("video.txt.tmp").exists());
        assert!(shard_dir.join("video.txt").exists());
    }

    // ------------------------------------------------------------------
    // T4 perf-tweaks — compact JSON encoder structural assertions
    // ------------------------------------------------------------------

    #[test]
    fn compact_json_round_trips_metadata() {
        // T4 perf-tweaks: pipeline.rs uses `serde_json::to_vec` (compact)
        // instead of `to_vec_pretty`. Assert the compact bytes round-trip
        // back into a structurally-equal TranscriptMetadata.
        let metadata = sample_metadata_with_raw_signals();
        let bytes = serde_json::to_vec(&metadata).expect("compact serialize");
        let parsed: TranscriptMetadata =
            serde_json::from_slice(&bytes).expect("parse compact bytes");

        assert_eq!(parsed.video_id, metadata.video_id);
        assert_eq!(parsed.duration_s, metadata.duration_s);
        assert_eq!(
            parsed.raw_signals.as_ref().map(|r| &r.schema_version),
            metadata.raw_signals.as_ref().map(|r| &r.schema_version),
        );
    }

    #[test]
    fn compact_json_has_no_indent_whitespace() {
        // Structural test that compact differs from pretty form: no
        // newline-followed-by-spaces patterns indicating pretty-print indent.
        // Size reduction is informational on this one-token fixture (too
        // small to make a non-brittle relative-size claim per spec) and is
        // not asserted; the structural absence of indent is the contract.
        let metadata = sample_metadata_with_raw_signals();
        let bytes = serde_json::to_vec(&metadata).expect("compact serialize");
        let s = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            !s.contains("\n  "),
            "compact JSON must not contain newline+spaces indent"
        );
        assert!(
            !s.contains("\n    "),
            "compact JSON must not contain newline+4-spaces indent"
        );
    }
}

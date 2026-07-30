use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ============================================================================
// Plan B Epic 1 (T10): TranscriptMetadata + raw_signals projection
// ============================================================================
//
// Per 0010 (raw_signals schema, schema_version): the per-video JSON artifact
// at `{transcripts_root}/{shard}/{video_id}.json` carries Plan A's existing
// provenance fields (video_id, source_url, fetcher, transcript_source, model,
// transcribed_at, language_detected, duration_s) PLUS an optional
// `raw_signals` sub-object pass-through (schema_version, language,
// lang_probs, segments[]).
//
// Module dependency direction (0016 worker-thread invariants):
// `src/transcribe.rs` MUST NOT import from this module. The transcribe layer
// is the source-of-truth domain type; the artifacts layer is the consumer
// that knows how to project domain types into JSON. The conversion lives on
// THIS side as `RawSignals::from_transcribe_output(&TranscribeOutput)`.
//
// T11 will wire the actual construction site at `src/pipeline.rs` once the
// Plan A whisper-cli call path is replaced by the Plan B whisper-rs engine.
// T10 just freezes the artifact schema and makes the struct compile +
// serialize correctly.

/// On-wire raw_signals schema version. 0010 + comment-2: this is a JSON
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
/// See 0010 for the schema contract; T9's `TranscribeOutput` is the
/// source-of-truth domain type that this projection consumes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawSignals {
    pub schema_version: String,
    pub language: String,
    /// 0010: serialize as `null` when absent (NOT omitted) — opt-in
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
/// etc.) per 0010's pass-through rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawToken {
    pub id: i32,
    pub text: String,
    pub p: f32,
    pub plog: f32,
}

impl RawSignals {
    /// Project T9's `TranscribeOutput` domain type into the artifact-side
    /// schema. 0016: the conversion lives on the artifact side so the
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

/// Process-wide tmp-name sequence: combined with the pid it makes each
/// atomic_write's tmp file unique across concurrent processes AND within
/// this process — two writers racing on the same video each rename their
/// OWN complete file onto the target (last rename wins; both are complete,
/// so either outcome is a valid artifact). Epic 4c hardening; 0008's
/// idempotence contract preserved.
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Atomic write for one file: write to `{path}.tmp-{pid}-{seq}`, fsync,
/// rename to `{path}`, fsync the parent directory. Caller is responsible for
/// parent existence. The tmp name is unique per writing process and per call
/// (see [`TMP_SEQ`]) so a concurrent writer of the same target never
/// truncates or renames away this call's in-flight tmp file.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("path {} has no parent", path.display()))?;

    let mut tmp_path = path.to_path_buf();
    let tmp_name = format!(
        "{}.tmp-{}-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .with_context(|| format!("path {} has no filename", path.display()))?,
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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

/// Sweep every file whose name contains `.tmp` under the transcripts root
/// — covers both the historical fixed `{name}.tmp` leftovers and the Epic 4c
/// suffixed `{name}.tmp-{pid}-{seq}` scheme. Artifact names are
/// `{video_id}.txt` / `{video_id}.json` with numeric video ids, so a `.tmp`
/// substring cannot occur in a real artifact name. Called at process startup
/// so leftover tmp files from crashed runs don't accumulate. The returned
/// count reports ONLY files this sweep actually deleted; failures are
/// warn-logged and not counted.
///
/// Only tmps whose mtime is older than `older_than` are deleted: a tmp
/// younger than the stale-claim window may belong to a live sibling process
/// mid-[`atomic_write`] (two-instance deployment), and deleting it makes that
/// sibling's rename fail — which aborts its whole batch run. A tmp older than
/// the stale-claim window cannot belong to a live claim (the claim itself
/// would have been swept). Unreadable mtime ⇒ skip + warn (never destroy on
/// uncertainty); an mtime in the future (clock anomaly) counts as fresh. Fresh
/// orphans from a crash survive one sweep and are collected next start.
pub fn cleanup_tmp_files(transcripts_root: &Path, older_than: Duration) -> Result<usize> {
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
                let is_tmp = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.contains(".tmp"));
                if is_tmp {
                    let old_enough = match std::fs::metadata(&p).and_then(|m| m.modified()) {
                        Ok(mtime) => match mtime.elapsed() {
                            Ok(age) => age > older_than,
                            // mtime in the future / clock anomaly: treat as fresh.
                            Err(_) => false,
                        },
                        Err(e) => {
                            tracing::warn!(path = %p.display(), error = %e, "tmp mtime unreadable; sparing file (never destroy on uncertainty)");
                            false
                        }
                    };
                    if !old_enough {
                        continue;
                    }
                    match std::fs::remove_file(&p) {
                        Ok(()) => removed += 1,
                        Err(e) => {
                            tracing::warn!(path = %p.display(), error = %e, "tmp cleanup failed; not counted");
                        }
                    }
                }
            }
        }
    }
    Ok(removed)
}

/// Sweep leftover per-acquire fetch directories under the `.work` root
/// (`ytdlp-{video_id}.{pid}-{seq}`, plus any pre-5b `ytdlp-{video_id}` dirs).
/// Called at process startup so attempt dirs orphaned by a crash, a `kill`, or
/// a cancelled fetch don't accumulate — the live paths clean up after
/// themselves (see [`crate::pipeline::FetchedAudio`]). The returned count
/// reports ONLY directories this sweep actually deleted; failures are
/// warn-logged and not counted (ADR-0006 shape).
///
/// Only dirs whose mtime is older than `older_than` are deleted, the same
/// argument [`cleanup_tmp_files`] makes for tmp files: a dir younger than the
/// stale-claim window may belong to a live sibling process mid-fetch
/// (two-instance deployment), and deleting it destroys that fetch's output
/// while yt-dlp is still writing into it. A dir older than the stale-claim
/// window cannot belong to a live claim (the claim itself would have been
/// swept). Unreadable mtime ⇒ skip + warn (never destroy on uncertainty); an
/// mtime in the future (clock anomaly) counts as fresh. Fresh orphans from a
/// crash survive one sweep and are collected next start.
///
/// Shallow and prefix-scoped on purpose: only `work_dir`'s own entries, only
/// directories named like attempt dirs. Anything else an operator parked under
/// `.work` is out of scope.
pub fn cleanup_work_dirs(work_dir: &Path, older_than: Duration) -> Result<usize> {
    if !work_dir.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(work_dir)
        .with_context(|| format!("reading work dir {}", work_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let is_attempt_dir = path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n.starts_with(crate::fetcher::ytdlp::ATTEMPT_DIR_PREFIX));
        if !is_attempt_dir {
            continue;
        }
        // One `metadata` call decides both questions (still a directory? old
        // enough?) — and its failure is the "never destroy on uncertainty"
        // case, e.g. a dangling symlink wearing the prefix.
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "work dir mtime unreadable; sparing dir (never destroy on uncertainty)");
                continue;
            }
        };
        if !meta.is_dir() {
            continue;
        }
        let old_enough = match meta.modified() {
            Ok(mtime) => match mtime.elapsed() {
                Ok(age) => age > older_than,
                // mtime in the future / clock anomaly: treat as fresh.
                Err(_) => false,
            },
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "work dir mtime unreadable; sparing dir (never destroy on uncertainty)");
                false
            }
        };
        if !old_enough {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => removed += 1,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "work dir cleanup failed; not counted");
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
        // Epic 4c: tmp names are `{name}.tmp-{pid}-{seq}`, so assert on the
        // substring rather than a hardcoded name — no tmp file of any shape
        // may survive the rename.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "tmp file should be renamed away, found: {leftovers:?}"
        );
    }

    /// Epic 4c hardening: a concurrent writer's tmp file (the OLD fixed
    /// name) must not be touched by this process's atomic_write — unique
    /// tmp names mean no cross-process collision.
    #[test]
    fn atomic_write_does_not_disturb_other_writers_tmp() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("video.txt");
        let decoy = tmp.path().join("video.txt.tmp");
        std::fs::write(&decoy, b"other writer's in-flight bytes").unwrap();

        atomic_write(&target, b"mine").expect("write succeeds");

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "mine");
        assert_eq!(
            std::fs::read_to_string(&decoy).unwrap(),
            "other writer's in-flight bytes",
            "the fixed-name tmp belongs to another process and must survive"
        );
    }

    /// cleanup_tmp_files: removes both old-style `.tmp` and new suffixed
    /// `.tmp-{pid}-{seq}` leftovers, and reports ONLY actual deletions.
    #[test]
    fn cleanup_tmp_files_counts_only_real_deletions() {
        let tmp = TempDir::new().unwrap();
        let shard = tmp.path().join("ab");
        std::fs::create_dir_all(&shard).unwrap();
        std::fs::write(shard.join("v1.txt.tmp"), b"old style").unwrap();
        std::fs::write(shard.join("v2.json.tmp-1234-7"), b"new style").unwrap();
        std::fs::write(shard.join("v3.txt"), b"real artifact, kept").unwrap();
        // A directory whose name matches: remove_file on it fails — it must
        // not be counted.
        std::fs::create_dir(shard.join("v4.txt.tmp")).unwrap();

        let removed = cleanup_tmp_files(tmp.path(), Duration::ZERO).unwrap();

        assert_eq!(
            removed, 2,
            "two files deleted; the directory failure is not counted"
        );
        assert!(!shard.join("v1.txt.tmp").exists());
        assert!(!shard.join("v2.json.tmp-1234-7").exists());
        assert!(shard.join("v3.txt").exists());
    }

    /// Backdate a file's mtime so the age guard sees it as old.
    fn set_mtime_secs_ago(path: &Path, secs: u64) {
        let t = std::time::SystemTime::now() - Duration::from_secs(secs);
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(t))
            .unwrap();
    }

    /// Epic 5a: a tmp younger than the stale-claim window may be a live
    /// sibling process's in-flight `atomic_write` — it must survive the sweep.
    #[test]
    fn cleanup_spares_fresh_tmp_and_removes_old_tmp() {
        let tmp = TempDir::new().unwrap();
        let shard = tmp.path().join("ab");
        std::fs::create_dir_all(&shard).unwrap();
        let fresh = shard.join("v1.txt.tmp-1234-0");
        let old = shard.join("v2.txt.tmp-5678-0");
        std::fs::write(&fresh, b"x").unwrap();
        std::fs::write(&old, b"x").unwrap();
        set_mtime_secs_ago(&old, 3600);

        let removed = cleanup_tmp_files(tmp.path(), Duration::from_secs(1800)).unwrap();
        assert_eq!(removed, 1, "only the old tmp is collected");
        assert!(
            fresh.exists(),
            "a fresh tmp may belong to a live sibling — spared"
        );
        assert!(!old.exists());
    }

    #[test]
    fn cleanup_with_zero_threshold_keeps_prior_behavior() {
        let tmp = TempDir::new().unwrap();
        let shard = tmp.path().join("cd");
        std::fs::create_dir_all(&shard).unwrap();
        let tmp_file = shard.join("v3.json.tmp-1-1");
        std::fs::write(&tmp_file, b"x").unwrap();
        set_mtime_secs_ago(&tmp_file, 2);
        let removed = cleanup_tmp_files(tmp.path(), Duration::ZERO).unwrap();
        assert_eq!(removed, 1);
        assert!(!tmp_file.exists());
    }

    // ------------------------------------------------------------------
    // Epic 5b Task 07 — `.work` attempt-dir sweep. Mirrors the 5a
    // `cleanup_tmp_files` family above, including the age-guard argument:
    // a fresh entry may belong to a LIVE sibling process.
    // ------------------------------------------------------------------

    /// Backdate a directory's mtime. `set_mtime_secs_ago` opens with
    /// `write(true)`, which is `EISDIR` on a directory — a read-only handle
    /// is enough for `futimens` on an owned inode.
    fn set_dir_mtime_secs_ago(path: &Path, secs: u64) {
        let t = std::time::SystemTime::now() - Duration::from_secs(secs);
        let f = File::open(path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(t))
            .unwrap();
    }

    fn stage_attempt_dir(work: &Path, name: &str) -> std::path::PathBuf {
        let d = work.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("vid.wav"), b"riff").unwrap();
        d
    }

    /// The 5a argument verbatim, applied to attempt dirs: a dir younger than
    /// the stale-claim window may be a live sibling's in-flight fetch —
    /// removing it destroys that fetch's output mid-download.
    #[test]
    fn cleanup_work_dirs_spares_fresh_and_collects_old() {
        let tmp = TempDir::new().unwrap();
        let work = tmp.path();
        let fresh = stage_attempt_dir(work, "ytdlp-vid_a.1234-0");
        let old = stage_attempt_dir(work, "ytdlp-vid_b.5678-0");
        set_dir_mtime_secs_ago(&old, 3600);

        let removed = cleanup_work_dirs(work, Duration::from_secs(1800)).unwrap();

        assert_eq!(removed, 1, "only the old attempt dir is collected");
        assert!(
            fresh.exists(),
            "a fresh attempt dir may belong to a live sibling — spared"
        );
        assert!(!old.exists(), "the old attempt dir and its wav are gone");
    }

    #[test]
    fn cleanup_work_dirs_with_zero_threshold_collects() {
        let tmp = TempDir::new().unwrap();
        let d = stage_attempt_dir(tmp.path(), "ytdlp-vid_a.1-1");
        set_dir_mtime_secs_ago(&d, 2);
        let removed = cleanup_work_dirs(tmp.path(), Duration::ZERO).unwrap();
        assert_eq!(removed, 1);
        assert!(!d.exists());
    }

    /// Never destroy on uncertainty: an entry whose mtime cannot be read
    /// (here a broken symlink) is skipped and warned about, not removed.
    /// Non-attempt entries are out of scope entirely.
    #[test]
    fn cleanup_work_dirs_spares_unreadable_and_unrelated_entries() {
        let tmp = TempDir::new().unwrap();
        let work = tmp.path();
        let broken = work.join("ytdlp-vid_c.9-9");
        std::os::unix::fs::symlink(work.join("nonexistent"), &broken).unwrap();
        let unrelated_dir = work.join("scratch");
        std::fs::create_dir_all(&unrelated_dir).unwrap();
        set_dir_mtime_secs_ago(&unrelated_dir, 3600);
        let unrelated_file = work.join("notes.txt");
        std::fs::write(&unrelated_file, b"x").unwrap();
        let old = stage_attempt_dir(work, "ytdlp-vid_d.7-0");
        set_dir_mtime_secs_ago(&old, 3600);

        let removed = cleanup_work_dirs(work, Duration::from_secs(60)).unwrap();

        assert_eq!(removed, 1, "only the one collectible attempt dir counts");
        assert!(
            std::fs::symlink_metadata(&broken).is_ok(),
            "unreadable mtime ⇒ spared"
        );
        assert!(unrelated_dir.exists(), "non-attempt dirs are out of scope");
        assert!(unrelated_file.exists());
        assert!(!old.exists());
    }

    /// ADR-0006 shape: the count reports ONLY dirs this sweep actually
    /// removed. A removal that fails (here: a dir whose parent denies write)
    /// is warn-logged and not counted.
    #[test]
    fn cleanup_work_dirs_counts_only_real_deletions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let work = tmp.path();
        let removable = stage_attempt_dir(work, "ytdlp-vid_a.1-0");
        set_dir_mtime_secs_ago(&removable, 3600);

        // A nested read-only parent: `remove_dir_all` on the inner attempt
        // dir needs write permission on THIS dir, so the removal fails.
        let locked_parent = work.join("locked");
        std::fs::create_dir_all(&locked_parent).unwrap();
        let stuck = stage_attempt_dir(&locked_parent, "ytdlp-vid_b.2-0");
        set_dir_mtime_secs_ago(&stuck, 3600);
        std::fs::set_permissions(&locked_parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        // Root ignores the permission bits; skip the assertion there.
        let denied = std::fs::write(locked_parent.join("probe"), b"x").is_err();

        let removed = cleanup_work_dirs(work, Duration::from_secs(60)).unwrap();
        let stuck_removed = cleanup_work_dirs(&locked_parent, Duration::from_secs(60)).unwrap();

        assert_eq!(
            removed, 1,
            "the sweep is shallow: only `work`'s own entries"
        );
        assert!(!removable.exists());
        if denied {
            assert_eq!(stuck_removed, 0, "a failed removal is not counted");
            assert!(stuck.exists());
        }
        // Restore so TempDir's own cleanup can run.
        std::fs::set_permissions(&locked_parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn cleanup_work_dirs_missing_root_is_zero() {
        let tmp = TempDir::new().unwrap();
        let removed = cleanup_work_dirs(&tmp.path().join("absent"), Duration::ZERO).unwrap();
        assert_eq!(removed, 0);
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

        // 0010: lang_probs MUST be present as `null` when not opted in
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

        let removed = cleanup_tmp_files(root, Duration::ZERO).unwrap();
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

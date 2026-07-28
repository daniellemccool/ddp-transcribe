# Task 05: Pre-production hardening — unique tmp names, artifact IO outside the store lock, honest cleanup count

Three operator-review findings, fixed together because they all harden the artifact write path for the 3M-video production run. A fourth finding (main.rs re-declares the lib module tree) is deliberately NOT fixed here — it is ADR-0002's deferred bin/lib reassessment and Task 06 files it as a FOLLOWUPS entry.

**Files:**
- Modify: `src/output/artifacts.rs` (`atomic_write` unique tmp names; `cleanup_tmp_files` matcher + honest count)
- Modify: `src/pipeline/mod.rs` (split `write_artifacts_and_mark` into write-then-mark pair)
- Modify: `src/pipeline/pipelined.rs` (transcribe worker: artifact writes BEFORE the store lock; lock only for the mark)
- Modify (via write-lean-adr skill ONLY): ADR-0008's record — ordering ownership moves from the single function to the write→mark pair
- Test: existing tests in `src/output/artifacts.rs` `mod tests` + new ones; `tests/pipeline_fakes/` suite must stay green untouched

**Interfaces:**
- Consumes: current `atomic_write(path, contents)`, `cleanup_tmp_files(root) -> Result<usize>`, `write_artifacts_and_mark(store, transcribe_output, claim, samples_len, wav_path, fetcher_name, transcript_source, opts) -> Result<ProcessOutcome>`, pipelined transcribe worker's phase-4 block (locks `store` around the whole helper call).
- Produces (exact, for the close docs):
  - `atomic_write` unchanged signature; tmp files now named `{target_name}.tmp-{pid}-{seq}` (`seq` = process-wide `AtomicU64`).
  - `cleanup_tmp_files` unchanged signature; matches any file whose name contains `.tmp` (covers old `.tmp` leftovers AND new suffixed names); increments `removed` ONLY on successful deletion, warn-logs failures.
  - `pub(crate) fn write_artifacts_durable(transcribe_output: &TranscribeOutput, claim: &Claim, samples_len: usize, opts: &ProcessOptions, fetcher_name: &'static str, transcript_source: &'static str) -> Result<Option<f64>>` — creates shard dir, writes txt then json via `atomic_write`, returns `duration_s`. NO store access.
  - `pub(crate) fn mark_after_artifacts(store: &mut Store, claim: &Claim, duration_s: Option<f64>, language: &str, wav_path: PathBuf, fetcher_name: &'static str, transcript_source: &'static str, opts: &ProcessOptions) -> Result<ProcessOutcome>` — `mark_succeeded` + stale-claim handling + wav cleanup (current tail of the old function, unchanged semantics).
  - `write_artifacts_and_mark` KEPT with its current signature as the composition (`write_artifacts_durable` then `mark_after_artifacts`) — the serial path keeps calling it; its doc comment now documents the pair as the 0008 ordering owner.

## Why each fix matters (context for the implementer)

1. **Fixed `<target>.tmp` names collide across processes.** A stale claim can be reclaimed while the original worker is still alive; two processes then write the same video's artifacts concurrently, and one can truncate/rename the other's tmp file mid-fsync. Unique tmp names make each writer rename its OWN complete file onto the target (last rename wins — both are complete artifacts, so either outcome is valid). Preserves ADR-0008's idempotence contract.
2. **The transcribe worker holds the store mutex across directory creation, two file writes, two file fsyncs, and a directory fsync** (`src/pipeline/pipelined.rs` phase-4 block). Every fetch worker's claim/failure dispatch serializes behind those fsyncs. At 3M videos this is real wall-clock. ADR-0008 requires disk-before-DB — which means the writes need NO lock at all; only `mark_succeeded` does.
3. **`cleanup_tmp_files` counts deletions it didn't perform** (`let _ = remove_file(); removed += 1;`) — the startup log can overstate cleanup.

- [ ] **Step 1: Write the failing artifacts tests**

In `src/output/artifacts.rs` `mod tests`:

```rust
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

        let removed = cleanup_tmp_files(tmp.path()).unwrap();

        assert_eq!(removed, 2, "two files deleted; the directory failure is not counted");
        assert!(!shard.join("v1.txt.tmp").exists());
        assert!(!shard.join("v2.json.tmp-1234-7").exists());
        assert!(shard.join("v3.txt").exists());
    }
```

- [ ] **Step 2: Run to confirm both fail**

Run: `cargo test --lib atomic_write_does_not_disturb cleanup_tmp_files_counts -- --test-threads=1` (adapt filter as needed)
Expected: FAIL — the decoy gets clobbered (old behavior renames over it or overwrites it), and the count comes back 3.

- [ ] **Step 3: Implement unique tmp names + honest count**

In `src/output/artifacts.rs`:

```rust
/// Process-wide tmp-name sequence: combined with the pid it makes each
/// atomic_write's tmp file unique across concurrent processes AND within
/// this process — two writers racing on the same video each rename their
/// OWN complete file onto the target (last rename wins; both are complete,
/// so either outcome is a valid artifact). Epic 4c hardening; 0008's
/// idempotence contract preserved.
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
```

In `atomic_write`, replace the tmp_name construction:

```rust
    let tmp_name = format!(
        "{}.tmp-{}-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .with_context(|| format!("path {} has no filename", path.display()))?,
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    );
```

In `cleanup_tmp_files`, replace the extension match + count:

```rust
                let is_tmp = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.contains(".tmp"));
                if is_tmp {
                    match std::fs::remove_file(&p) {
                        Ok(()) => removed += 1,
                        Err(e) => {
                            tracing::warn!(path = %p.display(), error = %e, "tmp cleanup failed; not counted");
                        }
                    }
                }
```

Update both functions' doc comments (`{path}.tmp` → the suffixed scheme; "all `*.tmp` files" → "files whose name contains `.tmp`"). Artifact names are `{video_id}.txt/.json` with numeric video ids, so a `.tmp` substring cannot occur in a real artifact name.

- [ ] **Step 4: Run artifacts tests to verify they pass** — plus the pre-existing `atomic_write_*` tests (the "no tmp remains" assertion must still hold: check whether it globs `*.tmp` — if it asserts on the exact old name, update it to assert no file containing `.tmp` remains).

- [ ] **Step 5: Split the write/mark helper**

In `src/pipeline/mod.rs`, restructure `write_artifacts_and_mark` (current body lines ~444–538) into:

```rust
/// Phase 4a — durable artifact writes (0008 first half). NO store access:
/// callers run this OUTSIDE any store lock; the fsyncs in atomic_write are
/// the slow part and must not serialize other workers' DB dispatch.
/// Writes txt then json (crash between the two leaves a complete txt and
/// missing json — preferable to the reverse). Returns duration_s.
pub(crate) fn write_artifacts_durable(
    transcribe_output: &TranscribeOutput,
    claim: &Claim,
    samples_len: usize,
    opts: &ProcessOptions,
    fetcher_name: &'static str,
    transcript_source: &'static str,
) -> Result<Option<f64>> {
    …current body through the json atomic_write (duration_s computation,
    shard dir creation, txt write, TranscriptMetadata build, json write),
    adjusted to borrow `&TranscribeOutput` (clone the strings it already
    clones today)…
    Ok(duration_s)
}

/// Phase 4b — DB acknowledgement (0008 second half) + wav cleanup. The
/// ONLY part that needs the store; pipelined callers lock exactly around
/// this. Semantics identical to the old tail: guarded mark_succeeded,
/// StaleAfterSuccess on 0 rows (wav intentionally left), wav cleanup
/// after the commit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mark_after_artifacts(
    store: &mut Store,
    claim: &Claim,
    duration_s: Option<f64>,
    language: &str,
    wav_path: PathBuf,
    fetcher_name: &'static str,
    transcript_source: &'static str,
    opts: &ProcessOptions,
) -> Result<ProcessOutcome> {
    …current body from `store.mark_succeeded(…)` to the end, with
    `Some(transcribe_output.language.clone())` becoming
    `Some(language.to_string())`…
}

/// 0008 ordering owner (revised, Epic 4c): the pair
/// write_artifacts_durable → mark_after_artifacts IS the invariant —
/// disk first, DB acknowledgement last. This composition serves the
/// serial path; the pipelined transcribe worker calls the two halves
/// directly so the store lock covers only the mark.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_artifacts_and_mark(…unchanged signature…) -> Result<ProcessOutcome> {
    let duration_s = write_artifacts_durable(
        &transcribe_output, claim, samples_len, opts, fetcher_name, transcript_source,
    )?;
    mark_after_artifacts(
        store, claim, duration_s, &transcribe_output.language, wav_path,
        fetcher_name, transcript_source, opts,
    )
}
```

(Keep every existing comment — the 0008 block comments, the T4 compact-JSON note, the T5-review stale-claim note — attached to the half where its code now lives.)

- [ ] **Step 6: Re-scope the pipelined lock**

In `src/pipeline/pipelined.rs`, phase-4 block (the `Ok(transcribe_output)` arm): replace the locked `write_artifacts_and_mark` call with:

```rust
                // Phase 4a (0008 first half): durable artifact writes with
                // NO store lock — the fsyncs must not serialize other
                // workers' claim/failure dispatch behind this mutex.
                let duration_s = write_artifacts_durable(
                    &transcribe_output,
                    &claim,
                    samples_len,
                    &opts,
                    fetcher_name,
                    transcriber.name(),
                )
                .with_context(|| format!("write_artifacts_durable for {}", claim.video_id))?;

                // Phase 4b (0008 second half): lock exactly around the DB
                // acknowledgement.
                let outcome = {
                    let mut guard = store.lock().await;
                    mark_after_artifacts(
                        &mut guard,
                        &claim,
                        duration_s,
                        &transcribe_output.language,
                        wav_path,
                        fetcher_name,
                        transcriber.name(),
                        &opts,
                    )
                    .with_context(|| format!("mark_after_artifacts for {}", claim.video_id))?
                };
```

Update the `use` list (`write_artifacts_and_mark` → the two new names) and delete the now-stale "under the store mutex" comment. Adjust for how the arm actually binds `transcribe_output` (it currently moves it into the helper; the split borrows it — check the surrounding ownership and adapt minimally).

- [ ] **Step 7: Revise ADR-0008 (via the write-lean-adr skill ONLY)**

The record currently states `write_artifacts_and_mark` owns the ordering for every pipeline variant. Revise: the ordering owner is the `write_artifacts_durable` → `mark_after_artifacts` pair; `write_artifacts_and_mark` remains the serial-path composition; pipelined calls the halves directly so the store lock covers only the DB acknowledgement; the invariant itself (disk first, DB last; idempotent atomic_write; recovery semantics) is UNCHANGED. Also note the unique-tmp scheme in the atomic_write guidance line. Validate with `adg lean review` / `adg lean index --root .`.

- [ ] **Step 8: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green — Task 04's 301 + 2 new = 303 passed. Every pre-existing `pipeline_fakes` and artifacts test must pass UNCHANGED (any pre-existing test needing edits beyond the tmp-name glob noted in Step 4 signals a semantics change — stop and re-examine).

- [ ] **Step 9: Commit**

```bash
git add src/output/artifacts.rs src/pipeline/mod.rs src/pipeline/pipelined.rs docs/decisions
git commit -m "fix(output,pipeline): pre-production hardening — unique atomic-write tmp names, artifact fsyncs outside the store lock, honest tmp-cleanup count"
```

Body cites the operator review findings and the ADR-0008 revision (ADR-0003 disclosure).

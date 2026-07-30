# Task 07: fetch hardening — attempt-dir lifecycle, exactly-one-WAV, redaction (Phase 2)

**Files:**
- Modify: `src/fetcher/ytdlp.rs` (fresh per-acquire dir + exactly-one-WAV discovery), `src/fetcher/mod.rs` (`Acquisition` gains the attempt-dir handle; `FetchOpts` Debug redaction; `scrub_cookie_path` guard), `src/pipeline/mod.rs` (~:594 — success cleanup removes the attempt dir, not just the WAV; failure paths likewise), `src/commands.rs` (Process arm: `.work` age-gated startup sweep call), `src/output/artifacts.rs` or a fetcher-local helper (the sweep fn — place beside `cleanup_tmp_files` and mirror its shape)
- Modify: `docs/reference/architecture/data-input.md` (~:86 work-dir description), `docs/operations/src-vm.md` (`.work` discussion)
- Test: inline `#[cfg(test)]` in ytdlp.rs + the existing pipeline_fakes/integration suites that exercise acquire/cleanup

**Interfaces:**
- Consumes: `YtDlpFetcher::acquire` (persistent `work_dir/ytdlp-{video_id}`, ytdlp.rs:240); tmp-sweep age-guard pattern (`cleanup_tmp_files(root, older_than)`, 5a); `cfg.stale_claim_threshold`.
- Produces: `Acquisition` (or its wrapper) exposes `attempt_dir: PathBuf`; `cleanup_work_dirs(work_dir: &Path, older_than: Duration) -> Result<usize>` (age-gated, skip+warn on unreadable mtime — destroy-on-uncertainty forbidden).

**Semantics (binding):**
- **Fresh unique per-acquire directory:** `work_dir/ytdlp-{video_id}.{pid}-{seq}` (the `atomic_write` tmp-name convention: pid + process-local monotonic seq). NO pre-run cleanup of prior dirs inside `acquire` (can delete a sibling's live output).
- **Exactly-one-WAV discovery:** after yt-dlp exits, scan ONLY this attempt's dir; exactly one `*.wav` ⇒ success; zero ⇒ the existing no-output failure path; >1 ⇒ a distinct failure ("ambiguous output") — never pick one. NEVER parse a reported path from stdout (stdout is the unparsed metadata capture; an untagged line would corrupt `load-metadata`).
- **Ownership:** after `acquire` returns, the CALLER (pipeline) owns the attempt dir. Cleanup points: success — remove the whole attempt dir where the WAV alone is removed today (pipeline/mod.rs:594 region, still after the DB commit per the existing comment's rationale); fetch failure / decode failure / transcribe failure / stale-claim outcomes — remove the dir at the same points the WAV is (or should be) dropped today; keep-on-`StaleAfterSuccess` semantics unchanged (deleting bytes the next claim might want stays forbidden).
- **Crash/cancel residue:** `cleanup_work_dirs(work_dir, cfg.stale_claim_threshold)` runs in the Process arm next to `cleanup_tmp_files` — collects only attempt dirs whose mtime is older than the threshold (a fresh dir may belong to a live sibling — the 5a argument verbatim); unreadable mtime ⇒ skip + warn; count only successful removals (ADR-0006-style `Result<usize>`).
- Redaction pair (from FOLLOWUPS bodies — read the two entries in `docs/followups/epic-5.md` before implementing): manual `Debug` impl (or field skip) so `FetchOpts`'s cookie path never appears in debug output; `scrub_cookie_path` returns input unchanged for an empty path instead of panicking/mangling.

- [ ] **Step 1: Failing tests.** (a) inline: two acquires for the same video id produce distinct dirs; exactly-one-WAV logic (0/1/2 WAV fixtures in a temp attempt dir) returns failure/Ok/ambiguous-failure respectively; (b) `cleanup_work_dirs` mirrors the 5a tests: fresh dir spared, old dir collected, `Duration::ZERO` collects, unreadable-mtime spared (reuse `set_mtime_secs_ago`); a live sibling's fresh dir is never removed while an old one beside it is; (c) pipeline: extend the existing fake-driven success test to assert the attempt dir is GONE after success, and a failure case leaves no dir; (d) `FetchOpts` debug string does not contain the cookie path; `scrub_cookie_path("")` is a no-op.
- [ ] **Step 2: Run to confirm failure** — targeted: `cargo test --features test-helpers fetcher -- --test-threads=1` etc.
- [ ] **Step 3: Implement** per the binding semantics (adapt to the files' local style; the ytdlp module owns naming/scan, the pipeline owns lifecycle, the sweep mirrors `cleanup_tmp_files` including its doc-comment rationale).
- [ ] **Step 4: Docs.** `data-input.md` work-dir paragraph + runbook `.work` note: per-attempt dirs, lifecycle owner, age-gated startup sweep, and that vanished-file rsync warnings for `.work/` remain expected during syncs.
- [ ] **Step 5: Green + full gate** (incl. release build).
- [ ] **Step 6: Commit**

```bash
git add src/fetcher/ src/pipeline/mod.rs src/commands.rs src/output/artifacts.rs docs/reference/architecture/data-input.md docs/operations/src-vm.md
git commit -m "feat(fetcher): unique per-acquire work dirs with exactly-one-WAV discovery and full lifecycle cleanup; age-gated .work sweep; cookie-path redaction hardening"
```

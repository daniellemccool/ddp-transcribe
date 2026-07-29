# Task 03: `src/backfill.rs` — stats + serial best-effort loop

**Files:**
- Create: `src/backfill.rs` (bin-only module: stats + orchestration)
- Modify: `src/main.rs` (add `mod backfill;` to the module list, lines ~7-23 — bin-only per the `metadata_loader` precedent; do NOT add it to `src/lib.rs`)

**Interfaces:**
- Consumes (exact, landed in Tasks 01–02):
  - `crate::state::Store` with `succeeded_missing_metadata_page(&self, after_video_id: Option<&str>, limit: usize) -> Result<Vec<MissingMetadataVideo>>`, `insert_metadata_raw_if_missing(&mut self, video_id: &str, envelope_json: &str) -> Result<usize>`; `crate::state::queries::MissingMetadataVideo { video_id: String, source_url: String }` (import path per how `main.rs`/`metadata_loader.rs` import queries types — check `rg 'MissingMetadataVideo|RawMetadataRow' src/` and match).
  - `crate::fetcher::ytdlp::{build_metadata_envelope, build_metadata_only_args, STDOUT_CAP}`.
  - `crate::process::{run, CommandSpec}` — `CommandSpec { program: &'static str, args: Vec<String>, timeout: Duration, stderr_capture_bytes: usize, stdout_capture_bytes: usize, redact_arg_indices: &[usize] }`; `run` returns `Ok(CommandOutcome { exit_code, stdout: Option<Vec<u8>>, stderr_excerpt, signal, .. })` on ANY exit code, `Err(RunError)` only on timeout/spawn/io.
- Produces (Task 04 relies on these exact names):
  - `pub struct BackfillStats { pub videos_examined: u64, pub envelopes_captured: u64, pub captures_failed: u64, pub rows_already_filled: u64, pub inserts_failed: u64 }` with `Display` + `Serialize`
  - `pub async fn backfill_metadata(store: &mut Store, ytdlp_timeout: Duration, limit: Option<u64>) -> anyhow::Result<BackfillStats>`

**Semantics (binding):**
- Serial loop, **deliberately no concurrency** (natural rate limiting toward TikTok; ~10K requests fit the 2–4 h budget). Review rejects worker pools here.
- Best-effort per video: yt-dlp failure (nonzero exit, timeout, spawn/io) or an unusable printed line logs + counts and moves on. The loop returns `Err` only for Store/query errors (DB broken ⇒ nothing sensible to continue with).
- Envelope is built from stdout **regardless of exit code** — mirrors `acquire`'s envelope-first order (ADR-0042): a print line that landed before a late failure is still data.
- Write path is `insert_metadata_raw_if_missing` ONLY. 0 rows changed ⇒ `rows_already_filled` (a fetch-path envelope landed concurrently; theirs wins). NEVER `upsert_metadata_raw`.
- **Never touches video status/lifecycle** — no other Store call exists in this module. Review rejects any status/claim/attempt write.
- ADR-0007: input-side, verb-named, parallel counters — every examined video increments `videos_examined` plus exactly one of the four outcome counters.
- `--limit` caps `videos_examined` across pages (smoke runs).
- No cookies, so no argv redaction (`redact_arg_indices: &[]`) and stderr excerpts are safe to log verbatim.

- [ ] **Step 1: Write the failing unit tests**

In `src/backfill.rs` (write the module skeleton first: types above, `mod tests` below):

```rust
    #[test]
    fn stats_display_is_operator_legible() {
        let stats = BackfillStats {
            videos_examined: 10,
            envelopes_captured: 7,
            captures_failed: 2,
            rows_already_filled: 1,
            inserts_failed: 0,
        };
        assert_eq!(
            stats.to_string(),
            "examined 10 / captured 7 / capture-failed 2 / already-filled 1 / insert-failed 0"
        );
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test stats_display_is_operator_legible -- --test-threads=1`
Expected: COMPILE FAIL (module absent).

- [ ] **Step 3: Implement `src/backfill.rs`**

```rust
//! Post-hoc metadata backfill (production ops, post-v0.3.0): recovers
//! `video_metadata_raw` envelopes for succeeded videos that predate
//! fetch-time capture (the rc1-era cohort). One metadata-only yt-dlp
//! invocation per video — no media, no GPU, and by construction no
//! writes to video status/lifecycle. Best-effort per video; re-running
//! converges (rows leave the cohort as envelopes land, and only
//! `insert_metadata_raw_if_missing` is ever called, so a fetch-path
//! envelope is never overwritten).

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::fetcher::ytdlp::{build_metadata_envelope, build_metadata_only_args, STDOUT_CAP};
use crate::process::{run, CommandSpec};
use crate::state::Store;

/// Cohort page size. Small next to the loader's 10k: every row costs a
/// network invocation, so paging overhead is noise, and a shorter page
/// keeps the cursor fresh against the shrinking cohort.
const PAGE_SIZE: usize = 1_000;

/// Backfill stats: input-side counters, verb-named (ADR-0007). Every
/// examined video increments exactly one outcome counter.
#[derive(Debug, Default, Serialize)]
pub struct BackfillStats {
    /// Cohort videos attempted this run.
    pub videos_examined: u64,
    /// Envelopes captured and inserted.
    pub envelopes_captured: u64,
    /// yt-dlp failed (nonzero exit, timeout, spawn/io) or printed no
    /// usable line — logged and skipped, never fatal.
    pub captures_failed: u64,
    /// Envelope built but a row already existed (fetch path landed one
    /// concurrently; theirs wins).
    pub rows_already_filled: u64,
    /// Envelope built but the DB insert failed (best-effort, counted).
    pub inserts_failed: u64,
}

impl std::fmt::Display for BackfillStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "examined {} / captured {} / capture-failed {} / already-filled {} / insert-failed {}",
            self.videos_examined,
            self.envelopes_captured,
            self.captures_failed,
            self.rows_already_filled,
            self.inserts_failed
        )
    }
}

/// One serial best-effort pass over the backfill cohort. `limit` caps
/// attempted videos (smoke runs). Returns `Err` only for Store/query
/// failures; per-video capture problems count and continue.
pub async fn backfill_metadata(
    store: &mut Store,
    ytdlp_timeout: Duration,
    limit: Option<u64>,
) -> Result<BackfillStats> {
    let mut stats = BackfillStats::default();
    let mut after: Option<String> = None;

    'pages: loop {
        let page = store.succeeded_missing_metadata_page(after.as_deref(), PAGE_SIZE)?;
        let Some(last) = page.last() else { break };
        after = Some(last.video_id.clone());

        for video in &page {
            if limit.is_some_and(|cap| stats.videos_examined >= cap) {
                break 'pages;
            }
            stats.videos_examined += 1;

            let envelope = match run(CommandSpec {
                program: "yt-dlp",
                args: build_metadata_only_args(&video.source_url),
                timeout: ytdlp_timeout,
                stderr_capture_bytes: 8 * 1024,
                stdout_capture_bytes: STDOUT_CAP,
                redact_arg_indices: &[],
            })
            .await
            {
                Ok(outcome) => {
                    // Envelope-first, exit code second (ADR-0042): a
                    // printed line that landed before a late failure is
                    // still data.
                    let envelope = build_metadata_envelope(outcome.stdout.as_deref(), STDOUT_CAP);
                    if envelope.is_none() {
                        tracing::warn!(
                            video_id = video.video_id.as_str(),
                            exit_code = outcome.exit_code,
                            stderr = outcome.stderr_excerpt.as_str(),
                            "backfill: no usable metadata line; skipped"
                        );
                    }
                    envelope
                }
                Err(err) => {
                    tracing::warn!(
                        video_id = video.video_id.as_str(),
                        error = %err,
                        "backfill: yt-dlp did not run to completion; skipped"
                    );
                    None
                }
            };

            let Some(envelope_json) = envelope else {
                stats.captures_failed += 1;
                continue;
            };

            match store.insert_metadata_raw_if_missing(&video.video_id, &envelope_json) {
                Ok(1) => stats.envelopes_captured += 1,
                Ok(_) => stats.rows_already_filled += 1,
                Err(err) => {
                    tracing::warn!(
                        video_id = video.video_id.as_str(),
                        error = %err,
                        "backfill: metadata insert failed; continuing"
                    );
                    stats.inserts_failed += 1;
                }
            }
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    // (Step 1's test lives here.)
}
```

Register the module in `src/main.rs`'s `mod` list (alphabetical placement among the existing declarations): `mod backfill;`. Do NOT touch `src/lib.rs`.

If clippy flags `backfill_metadata`/`BackfillStats` as dead code before Task 04 wires the CLI arm, add `#[allow(dead_code)]` with the ADR-0002 justification naming Task 04, and note it in the report; Task 04 removes it.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test stats_display_is_operator_legible -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green, suite total = Task 02's total + 1.

- [ ] **Step 6: Commit**

```bash
git add src/backfill.rs src/main.rs
git commit -m "feat(backfill): serial best-effort metadata backfill loop with ADR-0007 stats"
```

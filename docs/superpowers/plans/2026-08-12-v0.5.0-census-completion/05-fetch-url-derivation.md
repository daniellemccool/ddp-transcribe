# Task 05 — Claim-time canonical fetch-URL derivation

**Files:**
- Modify: `src/canonical.rs` (new helper), `src/state/mod.rs:424-435` +
  `:678-682` + `:717-722` (`Claim` gains `canonical`), `src/pipeline/mod.rs:374-384`
  (`acquire_audio` derives), `src/fetcher/mod.rs:137-174` + `:249+`
  (`FakeFetcher` records received URLs)
- Test: `tests/pipeline_fakes/pipelined_tests.rs`, `tests/state_claims.rs`
- Sweep: every `Claim {` and `FakeFetcher {` struct literal in tests
  (`rg 'Claim \{' src/ tests/`; `rg 'FakeFetcher \{' tests/ src/`)

**Interfaces:**
- Consumes: the fetch-URL ADR (Task 04); recency claim order (Task 03 —
  claim sequence in tests is deterministic: descending `video_id`).
- Produces:
  - `pub(crate) fn derived_fetch_url(video_id: &str) -> String` in `src/canonical.rs`
  - `Claim.canonical: bool` (SELECT column added in `claim_next`)
  - `FakeFetcher.received_urls: std::sync::Mutex<Vec<String>>`
  Task 06 reuses `derived_fetch_url`; Task 08's tests may construct
  `Claim`/`FakeFetcher` literals with the new fields.

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth: the **single production `fetcher.acquire` call** is inside
`acquire_audio` (`src/pipeline/mod.rs:374-384`):
```rust
let (capture, acquisition) = fetcher
    .acquire(&claim.video_id, &claim.source_url, opts)
    .await;
```
Both serial (`fetch_and_decode` :487) and pipelined (`pipelined.rs:330`)
paths flow through it. Artifacts take `claim.source_url` at
`src/pipeline/mod.rs:625` — that line must NOT change. `Claim` is
`{ video_id, source_url, attempt_count, last_retryable_kind }`
(`src/state/mod.rs:424-435`); `claim_next`'s SELECT is at :678-682 (as
amended by Task 03). Ingest sets `canonical = 1` for every production row
(`src/ingest.rs:407`). `FakeFetcher` has 7 public fields
(`src/fetcher/mod.rs:137-174`), constructed by struct literal in tests.

- [ ] **Step 1: Write the failing tests**

In `tests/pipeline_fakes/pipelined_tests.rs` (copy the setup shape of
`run_pipelined_honors_max_videos_cap:248` — TempDir, canned WAV map,
`FakeTranscriber::echo()`, full `ProcessOptions` literal):

```rust
#[tokio::test]
async fn canonical_claim_fetches_derived_url_but_artifact_keeps_provenance()
-> anyhow::Result<()> {
    // one canonical row whose STORED url is the share form
    let vid = "7700000000000000001";
    let stored = "https://www.tiktokv.com/share/video/7700000000000000001/";
    // seed: upsert_video(vid, stored, /*canonical=*/ true)
    // canned: map vid -> silence fixture wav
    // ... run run_pipelined to drain ...
    let urls = fetcher.received_urls.lock().unwrap().clone();
    assert_eq!(urls, vec![
        "https://www.tiktok.com/@x/video/7700000000000000001/".to_string()
    ], "fetcher must receive the derived canonical form");
    let json = std::fs::read_to_string(
        transcripts.join("01").join(format!("{vid}.json")))?;
    assert!(json.contains(stored),
        "artifact source_url must stay the stored provenance: {json}");
    Ok(())
}

#[tokio::test]
async fn non_canonical_claim_fetches_stored_url() -> anyhow::Result<()> {
    let vid = "7700000000000000002";
    let stored = "https://example.test/opaque";
    // seed with canonical = false; canned wav; drain
    let urls = fetcher.received_urls.lock().unwrap().clone();
    assert_eq!(urls, vec![stored.to_string()],
        "non-canonical rows keep their stored source_url");
    Ok(())
}
```
(Note the artifact shard is the last two digits of the video id — `01` for
`…0001`; ADR-0004.)

Also in `tests/state_claims.rs`:

```rust
#[test]
fn claim_carries_canonical_flag() -> anyhow::Result<()> {
    // seed one row upsert_video(vid, url, true), one with false
    // claim both; assert claim.canonical matches what was inserted
}
```
(Write the body concretely against `fresh_store_with`/`upsert_video` — the
existing helpers at `tests/state_claims.rs:7` show the seeding idiom.)

- [ ] **Step 2: Run to verify they fail for the real reason**

Run: `cargo test --features test-helpers --test state_claims -- --test-threads=1 claim_carries_canonical`
Expected: does not compile (`Claim` has no `canonical` field) — that IS
the real reason here; the field is the deliverable. For the pipelined
tests: compile failure on `received_urls`.

- [ ] **Step 3: Implement**

`src/canonical.rs` (beside `CANONICAL_RE`):
```rust
/// The WAF-surviving fetch form (2026-08-10 incident): canonical host,
/// non-empty placeholder user segment — CANONICAL_RE requires `@[^/]+`,
/// and any non-empty segment fetches identically (verified 2026-08-11).
/// Fetch-URL ADR: transport form lives here, provenance stays in the DB.
pub(crate) fn derived_fetch_url(video_id: &str) -> String {
    format!("https://www.tiktok.com/@x/video/{video_id}/")
}
```

`src/state/mod.rs` — `Claim` gains `pub canonical: bool` (:424-435); the
SELECT (:678-682, post-Task-03) gains the column:
```sql
SELECT video_id, source_url, attempt_count, last_retryable_kind, canonical
                 FROM videos
                 WHERE status = 'pending'
                 ORDER BY attempt_count ASC, video_id DESC
                 LIMIT 1
```
and the `Claim` construction (:717-722) maps it (`r.get::<_, i64>(4)? != 0`
idiom, same as `VideoRow` at :1586-1594 — match how the row tuple is read
in this function).

`src/pipeline/mod.rs` — inside `acquire_audio` (:374-384):
```rust
let fetch_url = if claim.canonical {
    crate::canonical::derived_fetch_url(&claim.video_id)
} else {
    claim.source_url.clone()
};
let (capture, acquisition) = fetcher.acquire(&claim.video_id, &fetch_url, opts).await;
```

`src/fetcher/mod.rs` — `FakeFetcher` gains
`pub received_urls: std::sync::Mutex<Vec<String>>`; in
`impl VideoFetcher for FakeFetcher` (:249+) record `source_url` at the top
of `acquire` before any canned/failure branching. Update the builders
(`always_fails:196`, `fails_with_stderr:214`, `gated_then_always_fails:231`)
and **every struct literal** (`rg 'FakeFetcher \{' tests/ src/`).

Sweep `Claim {` literals in tests the same way (add `canonical: true`
unless the test is specifically about non-canonical rows).

- [ ] **Step 4: Run the touched suites**

Run: `cargo test --features test-helpers --test state_claims -- --test-threads=1 && cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1`
Expected: PASS, including both new pipelined tests and the flag test.

- [ ] **Step 5: Full gate and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "feat(pipeline): derive canonical fetch URL at claim time; source_url stays provenance"`

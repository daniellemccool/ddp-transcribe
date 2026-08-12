# Task 06 — `backfill-metadata` uses the derived URL

**Files:**
- Modify: `src/backfill.rs:82-89` (call site), plus the page-row source it
  iterates (`succeeded_missing_metadata_page`, `src/state/mod.rs` — extend
  the row struct with `canonical` if it lacks it)
- Test: `tests/backfill_metadata.rs`

**Interfaces:**
- Consumes: `canonical::derived_fetch_url` (Task 05).
- Produces: backfill fetches metadata via the derived form for canonical
  rows — un-403-ing the rc1 cohort whose stored share-URLs the Aug-6 WAF
  gate blocks (incident-2 "Corpus consequence" note).

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth: `src/backfill.rs:82-89` runs
`build_metadata_only_args(&video.source_url)` per row from
`store.succeeded_missing_metadata_page(after.as_deref(), PAGE_SIZE)?`
(:72, cursor at :74). `build_metadata_only_args(source_url: &str)` is
`src/fetcher/ytdlp.rs:174-185` — its signature does not change; only what
we pass it does. Per the fetch-URL ADR, backfill must not grow a second
URL-format literal.

- [ ] **Step 1: Read the existing test harness**

Open `tests/backfill_metadata.rs` and identify how it fakes yt-dlp
(PATH-shim recording argv, or equivalent). The new test follows that
file's existing idiom exactly — do not invent a new harness.

- [ ] **Step 2: Write the failing test**

Following the file's idiom, add:

```rust
// Shape (adapt to the harness): seed one SUCCEEDED canonical row whose
// stored source_url is the share form and which lacks metadata; run the
// backfill pass; assert the recorded yt-dlp argv's final element is
// "https://www.tiktok.com/@x/video/<id>/" — NOT the stored share URL.
#[test]
fn backfill_fetches_derived_canonical_url() -> anyhow::Result<()> {
    /* per the file's existing seed/run/assert helpers */
    Ok(())
}
```
The assertion target is concrete: the last argv element the shim recorded
equals `derived_fetch_url(<seeded id>)`. If the page-row struct lacks a
`canonical` field, the test will drive you to add it (Step 4).

- [ ] **Step 3: Run to verify it fails for the real reason**

Run: `cargo test --features test-helpers --test backfill_metadata -- --test-threads=1 backfill_fetches_derived`
Expected: FAIL with the recorded argv carrying the share URL.

- [ ] **Step 4: Implement**

Extend `succeeded_missing_metadata_page`'s SELECT + row struct with
`canonical` (same `!= 0` mapping idiom as `Claim`), then at
`src/backfill.rs:82-89`:

```rust
let fetch_url = if video.canonical {
    crate::canonical::derived_fetch_url(&video.video_id)
} else {
    video.source_url.clone()
};
// …
args: build_metadata_only_args(&fetch_url),
```

- [ ] **Step 5: Run the backfill suites**

Run: `cargo test --features test-helpers --test backfill_metadata --test backfill_cohort -- --test-threads=1`
Expected: PASS.

- [ ] **Step 6: Full gate and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "feat(backfill): metadata backfill fetches the derived canonical URL"`

This closes **Phase 2** — controller writes `PHASE-2-CLOSE.md` and ends its
session per ADR-0019.

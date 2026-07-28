# Task 06: Live e2e, capture-chain ADR, doc updates, FOLLOWUPS, EPIC-4C-CLOSE

**Files:**
- Modify: `tests/e2e_real_tools.rs` (one new `#[ignore]`d live test — file exists and is auto-discovered; copy its ignore/attribute idiom)
- Create (via adg ONLY): one lean ADR in `docs/decisions/` — raw-first metadata capture chain
- Modify: `docs/reference/architecture/data-input.md` (fetch stage now captures the envelope), `docs/reference/architecture/state-machine.md` (schema v5 + new mutators), `docs/reference/architecture/index.md` (ADR table + stage summary)
- Modify: `docs/operations/src-vm.md` (operator workflow gains the post-run `load-metadata` step; GlobalArgs flags BEFORE the subcommand in every example — Epic 4b review caught this class)
- Modify: `docs/FOLLOWUPS.md` (+ scope index) — file the two entries listed below
- Create: `docs/superpowers/plans/2026-07-28-plan-b-epic-4c/EPIC-4C-CLOSE.md`

**Interfaces:**
- Consumes: everything landed in Tasks 01–05 (verify exact names against the code, not this plan, before writing docs — ADR-0003 honesty applies to docs too). Task 05's hardening (unique tmp names, lock-scope split, ADR-0008 revision) belongs in the close narrative and the state-machine/data-input doc checks.
- Produces: the epic's close-out record; no code surface.

- [ ] **Step 1: Live e2e test (ignored)**

In `tests/e2e_real_tools.rs`, following the existing ignored test's setup idiom (real `YtDlpFetcher`, network + yt-dlp required):

```rust
/// Epic 4c: real fetch captures a parseable metadata envelope. Ignored:
/// needs network + yt-dlp; run manually (also the designated re-verification
/// hook after any yt-dlp upgrade, per ADR-0033's pin-and-reverify rule).
#[tokio::test]
#[ignore]
async fn real_fetch_captures_metadata_envelope() {
    // Use the same known-good URL constant/style as the existing ignored
    // e2e test in this file. Drive YtDlpFetcher::acquire directly.
    // Assert: capture.is_some(); envelope parses as JSON; envelope["schema"] == 1;
    // envelope["printed"] parses as JSON and contains a non-empty "id" and
    // a non-null "description".
}
```

Write it fully against the file's real fixtures (the pseudocode names the mandatory assertions). Run it once manually to prove it: `cargo test --test e2e_real_tools real_fetch_captures_metadata_envelope -- --ignored --test-threads=1` → PASS (paste the output into the task report).

- [ ] **Step 2: The ADR (via write-lean-adr ONLY)**

Invoke the `write-adr:write-lean-adr` skill and author ONE record (`adg lean new --from-stdin`; never hand-edit `docs/decisions/`):

- **Title:** "Fetch-time metadata is captured raw-first; parsing is a replayable post-run step"
- **applies_to:** `src/fetcher/ytdlp.rs`, `src/fetcher/mod.rs`, `src/metadata_loader.rs`, `src/state/schema.rs`
- **Decision core:** metadata rides the existing yt-dlp invocation (`--no-simulate --print` + subs sidecars, bounded 64 KB stdout); the UNPARSED versioned envelope is upserted into `video_metadata_raw` before outcome interpretation (failure paths included, zero mutator changes); only `load-metadata` parses, into nullable schema-v5 columns — so parse bugs are re-parse-fixable, never re-fetch (3M-video corpus: fetch is the irreplaceable operation).
- **Guidance must include:** metadata never creates a new failure mode (capture/insert/parse errors log + count, video outcome unchanged — review rejects violations); envelope `schema` field governs loader compatibility (unknown version ⇒ skip, count); the printed field set is deliberately wider than the typed columns; caption content (not URLs) embedded best-effort, platform-served tracks only; engagement counts are point-in-time snapshots keyed by `metadata_fetched_at`.
- **Why must cite:** the 2026-07-28 probe evidence (46/46 title coverage, 0/46 caption tracks, sticker text structurally absent from yt-dlp), Research API unavailability, and the 2,982,471-unique-video production measurement. Alternatives rejected: `--write-info-json` (per-video file I/O ×3M), separate enrichment pass (doubles network), direct-to-schema parsing (unrecoverable without re-fetch), JSONL sidecar log (torn lines, new operational artifact).

- [ ] **Step 3: Architecture + ops docs**

Verify every claim against the landed code first (`rg` the actual names). Then:
- `data-input.md`: fetch-stage section gains the capture chain (print template → envelope → `video_metadata_raw`, failure-path inclusion, best-effort invariant) and the loader's place in the data flow.
- `state-machine.md`: SCHEMA_VERSION note (v5, what v4→v5 added — table + 9 columns, nothing else), migrate-ladder paragraph (four stages), mutator contract list gains `upsert_metadata_raw` and `apply_metadata_batch` with exact signatures.
- `index.md`: stage summary + ADR table row for the new record.
- `src-vm.md`: operator sequence becomes ingest → process → **load-metadata** → status; one fenced example with `--state-db` BEFORE `load-metadata`; one sentence on replayability and on the ~0% caption-coverage expectation (PI-facing honesty).

- [ ] **Step 4: FOLLOWUPS entries**

Per ADR-0020 structure (scope index + grouped entries), file:
1. **(target: pre-production-run)** "Capacity estimate for the production batch": 2,982,471 unique videos measured 2026-07-28; fetch+transcribe throughput, window narrowing, and disk (raw table 6–12 GB; transient WAVs) need a sizing pass before the full run.
2. **(target: post-first-production-batch)** "video_metadata_raw prune/VACUUM decision": after a successful `load-metadata`, decide whether the operator prunes blobs for a lean export copy or keeps them for future re-parse.
3. **(target: Epic 5, cites ADR-0002's deferred bin/lib reassessment)** "main.rs re-declares the library's module tree": `src/main.rs` re-declares nearly every module `src/lib.rs` exposes — double compilation of most of the crate, broadened public surface, and a driver of the accumulated `dead_code`/`unused_imports` suppressions. Idiomatic fix: `main.rs` imports `ddp_transcribe::…` from one canonical module tree. Operator review finding 2026-07-28; too broad for an Epic 4c rider — belongs with the Epic 5 hygiene bundle (`run_serial` retirement, `state/mod.rs` split, sync-IO sweep).

- [ ] **Step 5: EPIC-4C-CLOSE.md**

≤1 page: task table (commits per task), suite count progression (283 baseline → final; state the final total), deviations disclosed during the epic (pull from the progress ledger — record each honestly), the live-e2e run evidence, what was deliberately omitted (spec non-goals restated in one line), and the two FOLLOWUPS filed.

- [ ] **Step 6: Full verification + commit(s)**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` (docs must leave the tree green; the new e2e is ignored by default and must NOT count against the suite).
Pre-commit hook must pass (`adg lean index --root .` + `adg lean check`) — fix root causes, never bypass.

```bash
git add tests/e2e_real_tools.rs
git commit -m "test(e2e): ignored live check — real fetch captures a parseable metadata envelope"
# ADR commit is produced by the write-lean-adr flow (adg regenerates README.md in the same commit)
git add docs/reference docs/operations docs/FOLLOWUPS.md docs/superpowers/plans/2026-07-28-plan-b-epic-4c/EPIC-4C-CLOSE.md
git commit -m "docs(epic-4c): close — capture-chain ADR companions, architecture/ops updates, FOLLOWUPS, EPIC-4C-CLOSE"
```

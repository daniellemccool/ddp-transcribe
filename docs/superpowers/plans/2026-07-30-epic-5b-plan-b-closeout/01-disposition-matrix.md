# Task 01: FOLLOWUPS disposition matrix (Phase 0)

**Files:**
- Create: `docs/superpowers/plans/2026-07-30-epic-5b-plan-b-closeout/DISPOSITION-MATRIX.md`

**Interfaces:**
- Consumes: `docs/FOLLOWUPS.md` scope index (Epic 5 + cross-epic groups), `docs/followups/epic-5.md`, `docs/followups/cross-epic.md`, `docs/archive/followups-resolved.md`.
- Produces: one matrix row per entry with a terminal disposition that Tasks 09–13 execute; the controller uses the OPERATOR-DECIDES rows to ask the human once, before Phase 1 starts.

**Semantics (binding):**
- Docs-only task; no code changes. Every entry in the Epic 5 group and the cross-epic group of `docs/FOLLOWUPS.md` gets exactly one row. Dispositions: `fix (Task NN)` / `verify-and-archive (Task 13)` / `accepted-archive (Task 13)` / `re-route Plan C` / `out of Plan-B scope` / `archive-integrity check (Task 13)`.
- Pre-resolved entries are **archive-integrity check** rows, never re-archive: multi-fetcher provenance (archived, Epic 1 T11), `From<RunError> for FetchError` (@9974d69), `--whisper-model` global flag (@7dfa771). A regression found during the check becomes a NEW active FOLLOWUPS entry.
- `reset-stale-claims` gets a row: **superseded sketch item** (operator ruling 2026-07-30; no active entry exists, nothing archived).
- Rows the matrix must mark **OPERATOR-DECIDES** (the controller presents them in ONE batched question after this task): ingest-ledger basename-collision + 1s-fingerprint-resolution (design choices, PR #23 bundle); tmp-sweep TOCTOU entry (accepted-archive vs keep); `upsert_metadata_raw` claim-guard (resolve vs Plan C); `run_serial` retire-or-keep (affects Task 13 wording only — Phase 1 retains it regardless); closed-reply logging (verify whether the transcribe work item carries a video/request id — read `src/pipeline/pipelined.rs`'s transcribe channel item struct and record the finding in the row).
- T9 `updated_at` row is pre-ruled (spec): document as **lifecycle-mutation time; a no-op ingest is clock-neutral** — executed in Task 10.

- [ ] **Step 1: Build the matrix.** Read the two scope-index groups and each referenced body; write `DISPOSITION-MATRIX.md` as a table: `| Entry (index line) | Body ref | Disposition | Executing task | Notes |`. Verify each "already archived" claim against `docs/archive/followups-resolved.md` before writing its row.

- [ ] **Step 2: Cross-check completeness.** `grep -c '^- ' docs/FOLLOWUPS.md` for the Epic 5 and cross-epic sections; row count must match (every index line accounted for). State the counts in the matrix header.

- [ ] **Step 3: Verification** — `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` (docs-only; must stay green).

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/plans/2026-07-30-epic-5b-plan-b-closeout/DISPOSITION-MATRIX.md
git commit -m "docs(plans): Epic 5b Phase 0 — FOLLOWUPS disposition matrix; operator-decision rows flagged"
```

**After this task:** the controller presents all OPERATOR-DECIDES rows to the human as one batched question and records rulings directly in the matrix file (one follow-up commit) before dispatching Task 02.

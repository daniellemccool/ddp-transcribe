# Task 08: Close — ADR slate, architecture docs, operations doc, FOLLOWUPS lifecycle, EPIC-4B-CLOSE

**Files:**
- Create: two lean ADRs via the `write-adr:write-lean-adr` skill (`adg lean new --from-stdin`; numbers assigned by adg — NEVER hand-create files in `docs/decisions/`)
- Modify: `docs/reference/architecture/index.md`, `docs/reference/architecture/state-machine.md`, `docs/reference/architecture/data-input.md`
- Modify: `docs/operations/src-vm.md`
- Modify: `docs/FOLLOWUPS.md`, `docs/followups/epic-4.md`, `docs/archive/followups-resolved.md`
- Create: `docs/superpowers/plans/2026-07-13-plan-b-epic-4b/EPIC-4B-CLOSE.md`

**Interfaces:** consumes everything (real commit SHAs from `git log --oneline`, Task 01's verdict + ADR number); produces the epic's close-out. No new code interfaces.

- [ ] **Step 1: ADR — window-flag semantics**

Via `write-adr:write-lean-adr`. Content to convey:

- **Title:** "Analysis window is computed at ingest; recompute-window is the only flag mutator"
- **Decision:** `watch_history.in_window` is computed once at ingest from inclusive UTC calendar dates (`--window-start` 00:00:00Z inclusive; `--window-end` covers its whole day — next-midnight exclusive; absent side unbounded; both absent = everything in-window). After ingest, only the explicit `recompute-window` subcommand changes flags: it requires at least one of `--window-start`/`--window-end`/`--clear` (bare invocation is a usage error), `--clear` is the deliberate no-filter opt-in, `--dry-run` counts without writing, and the mutator reports the actually-changed row count (0006). The verbatim DDP `Date` string persists in `watched_at_raw` (schema v4); re-ingest backfills NULL raws but NEVER touches existing `in_window` values.
- **applies_to:** `src/ingest.rs`, `src/state/mod.rs`, `src/state/schema.rs`; companions `tests/ingest.rs`, `tests/recompute_window.rs`.
- **Guidance:** consumers filter `WHERE in_window = 1` and never re-derive window membership from `watched_at`; day-granularity windows absorb the sub-day ambiguity documented in the timezone ADR (Task 01's number); no code path may set flags implicitly (review rejects ingest-time "helpful" recomputes); `watched_at_raw` is never dropped or normalized.
- **Why:** the spec-era guardrail — silently wiping a study's window filter via a bare invocation is an unrecoverable-in-practice operator mistake; raw preservation makes the timezone verdict non-fatal either way.

- [ ] **Step 2: ADR — status output schema / done-contract fulfillment**

Via `write-adr:write-lean-adr`. Content to convey:

- **Title:** "status is the read-only operator surface; the 0017 done-contract lives behind --verify"
- **Decision:** `status` is DB-only and read-only by default (counts by status, failed_retryable by kind, in-progress claim ages, full `batch_runs` history with interrupted rows rendered honestly); detail modes `--video-id` (legible event `detail_json` rendering), `--respondent-id`, `--errors`, `--retryable`; `--json` emits the serialized report structs as the stable tooling schema. `status --verify` implements the archived MADR-0017 done-contract — per-shard batched artifact-existence, full `raw_signals.schema_version` parse, pause-safe verdict (`pending == 0 ∧ in_progress == 0 ∧ zero artifact/schema/read failures`) — and exits 1 when not pause-safe, 0 otherwise. **This record is the lean successor to archived `docs/madr-archive/0017-…`** (0017 predates the lean migration; the archive stays frozen).
- **applies_to:** `src/status.rs`, `src/state/queries.rs`; companions `tests/status.rs`.
- **Guidance:** `status` never mutates study state and bails (does not create) on a missing DB; interrupted `batch_runs` rows are never skipped and never crash on NULLs; JSON output carries raw stored values (the legacy-`"Fetch"` placeholder annotation is human-render-only); schema-version **sampling** is a Plan C concern — do not add it at Plan B scale; new operator surfaces extend `status` in-tool per the 0032 operator-interface premise, not wrapper scripts.
- **Why:** the 2026-07-08 production batch demonstrated both needs this record froze: an interrupted run whose only honest record is the open row, and the by-kind retryable breakdown the operator repeatedly hand-wrote as SQL. Ground-truthed against that batch's snapshot (51,903/3,928/789; six-kind breakdown).

Then `adg lean index --root .` — expect a clean pass (the pre-commit hook re-checks).

- [ ] **Step 3: Architecture-doc drift pass (standing cross-epic followup: status touches the state-machine surface)**

- `docs/reference/architecture/index.md`: add `status` (+ `--verify` done-contract) and `recompute-window` to the CLI/lifecycle narrative; §4's ADR table gains rows for the three new ADRs (timezone, window, status) with one-line summaries; ingest stage text mentions window flags + `watched_at_raw`.
- `docs/reference/architecture/state-machine.md`: add a short "Operator visibility" subsection: `status` renders the state machine (counts, kinds, claim ages, event history with the Epic 4a `detail_json` vocabulary); the pause-safe predicate consumed by 0011's spin-down practice now exists in-tool; note the schema v4 bump.
- `docs/reference/architecture/data-input.md`: document `--window-start`/`--window-end` semantics (inclusive UTC dates), `watched_at_raw` preservation + backfill-on-re-ingest, and the timezone verdict with a pointer to Task 01's ADR; fix any lingering description of `in_window` as "always 1".
- Read each file before editing; keep the surrounding voice; do not restructure beyond these additions.

- [ ] **Step 4: Operations doc**

`docs/operations/src-vm.md` (read first; match its format):
- Update instructions gain: after pulling/building a 4b binary, run `ddp-transcribe --state-db ~/ddp-state/state.sqlite migrate` (v3→v4, idempotent — the binary refuses un-migrated DBs with a typed error).
- Operate section gains the status quickstart: `ddp-transcribe --state-db ~/ddp-state/state.sqlite status` (counts + batch history), `status --retryable` (by-kind pools; the 301 cookie-parked rows carry the legacy placeholder kind `Fetch`), and `status --verify --transcripts ~/ddp-work/transcripts` before any workspace pause (exit 0 = pause-safe per 0011).

- [ ] **Step 5: FOLLOWUPS lifecycle (0020)**

With the real resolving SHAs from `git log --oneline` (tasks 01–07):
1. Move ALL FIVE `docs/followups/epic-4.md` entries to `docs/archive/followups-resolved.md`, each with its resolving SHA and a one-line resolution note:
   - `parse_watched_at` UTC assumption → **record Task 01's verdict explicitly** (whichever way it landed — the entry resolves with the verdict, not just "closed"); cite the ADR number.
   - Open `batch_runs` row rendering → resolved by the status core task (INTERRUPTED rendering + the reconstructable-from-videos note made it into the renderer text).
   - `--retries` i64 validation → resolved by the hardening task (RangedI64 0..=1_000_000).
   - Config-echo scoping → resolved by the hardening task (per-subcommand echo).
   - Operator-interface premise → archive as "honored and now embodied: Epic 4b baked the operator surface (`status`, `recompute-window`) into the tool; the durable record remains the 0032 ADR comment."
2. `docs/followups/epic-4.md`: all entries gone — replace the body with a pointer line ("Epic 4b closed <date>; entries archived in ../archive/followups-resolved.md") or delete the file if that matches how closed epics' files were handled for epic-3 (check `docs/followups/` for the precedent and follow it).
3. `docs/FOLLOWUPS.md`: remove the Epic 4b scope-index group (and its "Full entries" pointer line if the sub-file is gone).

- [ ] **Step 6: EPIC-4B-CLOSE.md**

Create `docs/superpowers/plans/2026-07-13-plan-b-epic-4b/EPIC-4B-CLOSE.md` following `EPIC-4A-CLOSE.md`'s structure:
- Header: branch, status, one-paragraph summary (status surface + done-contract; window/timezone; hardening; ADR numbers).
- Task→commit(s) table with REAL SHAs.
- Verification line: full command output summary (test count, clippy clean, `adg lean index` clean).
- **Acceptance section:** rerun the ground-truth acceptance one final time against a FRESH scratch copy of `ddp-run-export.sqlite` (copy → `migrate` → `status` / `status --retryable` / `status --respondent-id preview --json`) and paste the outputs proving: 51,903/3,928/789 counts, the six-kind breakdown, run 1 INTERRUPTED / run 2 closed with census, `watch_events` 64,931 for `preview`. Any disagreement is a release blocker.
- Timezone verdict paragraph: the verdict, its evidence chain, the ADR number.
- "Deferred / not in 4b" list: cookie-efficacy run (operational; `status` reads its results), `status --logs` sketch idea (unscoped — file a FOLLOWUPS entry only if the operator wants it), Epic 5 cleanup bundle, schema-version sampling (Plan C).

- [ ] **Step 7: Full verification + commit + wrap**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green. The pre-commit hook runs `adg lean index --root .` + `adg lean check` on staged files — fix any inconsistency, never bypass.

```bash
git add docs/
git commit -m "docs(epic-4b): close — ADR slate (timezone, window semantics, status/0017 fulfillment), architecture + operations docs, FOLLOWUPS archived, acceptance rerun"
```

(If the ADR-authoring steps produced separate commits per `adg` conventions, keep them — disclose the split in the task report.)

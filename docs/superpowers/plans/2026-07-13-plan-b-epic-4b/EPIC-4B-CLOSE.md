# Plan B Epic 4b — Epic Close

**Branch:** `feat/plan-b-epic-4b`
**Status:** all 8 tasks complete. Operator-facing `status` command (counts, retryable-by-kind, claim ages, honest `batch_runs` history, detail modes, `--json`) with the archived MADR-0017 done-contract now living behind `status --verify` (ADR-0041); schema v4 window/timezone work — inclusive-UTC-date `--window-start`/`--window-end` computed once at ingest, the sole `recompute-window` mutator, and `watched_at_raw` preservation as the hedge against ADR-0039's unresolved timezone verdict (ADR-0040); CLI hardening (`--retries` range validation, per-subcommand config echo). ADRs accepted at close.

## What landed

| Task | Commit(s) | Subject |
|---|---|---|
| 01 | `1a8bc49` | ADR-0039 — DDP timestamp timezone verdict: **"UTC-assumed (documentary evidence), empirically unresolved"**; `parse_watched_at` format-provenance comments corrected |
| 02 | `d9d8125`, `5ac0bfa` | `status` core: `src/state/queries.rs`, `src/status.rs`, `Command::Status{json}` — counts, retryable-by-kind, claim ages, honest batch-run history (INTERRUPTED rendered, never skipped); fix loop added policy-provenance test coverage |
| 03 | `730d887`, `2cfebfc`, `c6694eb` | Detail surfaces: `--video-id` (legible `detail_json`), `--respondent-id`, `--errors`/`--retryable`; fix loop made detail modes conflict at parse time |
| 04 | `9278539`, `7e5966d` | `status --verify` — the ADR-0017 done-contract: per-shard artifact existence, full `raw_signals.schema_version` parse, pause-safe verdict, exit 1 on violation; fix loop distinguished infra faults from absent artifacts |
| 05 | `bdc4723`, `2484a23` | Schema v4: `watch_history.watched_at_raw` + `in_window`; `ingest --window-start`/`--window-end`; reversed-range rejection |
| 06 | `a13fe41`, `ab86b89` | `recompute-window` subcommand — explicit one-shot recompute, refuses bare invocation, `--clear`/`--dry-run`; shared `cli::validate_window_order` guard extended to it |
| 07 | `0d1b7a2` | `--retries` bounded `0..=1_000_000` at parse time; `log_resolved_config` scoped per subcommand (exhaustive match) |
| 08 | `50c9d46` + this commit | ADR slate (0040 window semantics, 0041 status/0017 fulfillment) via `adg lean new`; architecture + operations docs; FOLLOWUPS archived with SHAs; acceptance rerun; this close doc |

Verification at close: `cargo fmt` clean; `cargo clippy --all-targets -- -D warnings` clean (0 warnings); `cargo test --features test-helpers -- --test-threads=1` green — **283 passed, 0 failed, 8 ignored** (model-gated, require `./models/ggml-tiny.en.bin`); `adg lean index --root .` — 0 failures, 2 advisory warnings (0040/0041 Decision sections run long — content, not correctness).

## Three adjudicated deviations from the plan's literal briefs (Tasks 05–07)

Each was disclosed per ADR-0003 in its task commit:

1. **Task 05:** `IngestStats` field landed as `computed_out_of_window`, not the brief's `marked_out_of_window` — the brief's wording conflicted with ADR-0007's input-side-counter constraint (the counter increments on computation, not on a write that may not happen for duplicate-PK rows); controller-adjudicated rename.
2. **Tasks 05+06:** `cli::validate_window_order` — both `ingest` and `recompute-window` reject `--window-start > --window-end` before `Store::open`, beyond either brief's verbatim code (reviewer+codex finding, extended to `recompute-window` as a Task-05-convention continuation). Equal dates are a valid single-day window.
3. **Task 05:** the migrate-ladder test fixtures gained the pre-v4 `watch_history` table — a latent gap the v3→v4 `ALTER TABLE` exposed (the fixtures previously never modeled a DB old enough to lack the column being added).

## Ground-truth acceptance — final rerun (fresh scratch copy, 2026-07-28)

Per the brief: `cp ddp-run-export.sqlite <scratch>` → `migrate` (v3→v4) → `status` / `status --retryable` / `status --respondent-id preview --json`. The original snapshot (repo root, untracked, read-only) was never touched.

```
$ ddp-transcribe --state-db <scratch>/epic4b-acceptance.sqlite migrate
migrate: complete from=3 to="4"

$ ddp-transcribe --state-db <scratch>/epic4b-acceptance.sqlite status
videos: 56620 total
  pending                  0
  in_progress              0
  succeeded            51903
  failed_terminal       3928
  failed_retryable       789
failed_retryable by kind:
  NoPermission                 418
  Fetch                        301  (legacy placeholder kind)
  FfprobePostprocess            36
  NoVideoFormats                32
  HttpError                      1
  NoDataBlocks                   1
in_progress claims: none
batch runs (2):
  run 1  started 2026-07-08 11:41:50Z  INTERRUPTED (never closed; no census — outcomes remain reconstructable from the videos table)  retries=1 workers=3 cookies=no  policy: compiled default (3071 B)
  run 2  started 2026-07-08 15:47:11Z  finished 2026-07-08 16:32:12Z  retries=2 workers=3 cookies=no  policy: compiled default (3071 B)
         census: sweep examined 1388, claimed 3084, succeeded 2333, failed 751

$ ddp-transcribe --state-db <scratch>/epic4b-acceptance.sqlite status --retryable
failed_retryable (789):
  <789 rows, kinds matching the by-kind breakdown above>

$ ddp-transcribe --state-db <scratch>/epic4b-acceptance.sqlite status --respondent-id preview --json
{
  "respondent": {
    "respondent_id": "preview",
    "watch_events": 64931,
    "videos_seen": 56600,
    "videos_in_window": 56600,
    "videos_succeeded": 51884,
    "videos_failed_terminal": 3927,
    "videos_failed_retryable": 789,
    "videos_pending": 0,
    "videos_in_progress": 0
  }
}
```

**All claims verified, exactly, against this final rerun:**

- 56,620 total = **51,903** succeeded / **3,928** failed_terminal / **789** failed_retryable / 0 pending / 0 in_progress. ✓
- Six-kind `failed_retryable` breakdown (NoPermission 418, Fetch 301, FfprobePostprocess 36, NoVideoFormats 32, HttpError 1, NoDataBlocks 1 — sums to 789). ✓
- Run 1 renders **INTERRUPTED**; run 2 closed with census. ✓
- `preview` respondent: `watch_events` = **64,931**. ✓
- Policy provenance: compiled default, **3,071 B** (bytes, not the plan prose's 3,065 chars — three em-dashes each cost 2 extra bytes in UTF-8). ✓

No disagreement — not a release blocker.

## Timezone verdict

**Verdict:** "UTC-assumed (documentary evidence), empirically unresolved" (ADR-0039, `1a8bc49`).

**Evidence chain:** TikTok's May-2026 export pipeline stamps its own output with a literal `" UTC"` suffix (commit `2d89860`) — the documentary anchor: the most economical reading is that the backend convention is UTC and the unlabeled July-2026 renderer format simply dropped the label rather than changing convention. An operator empirical spot-check compared memory of two known watch sessions (2026-02-18 20:15, 2025-12-21 01:42) against the parsed UTC timestamps and could not discriminate UTC from local time at ±1h precision — the verbatim finding was "Can't discriminate — one hour is within my memory's error bars for these sessions." The verdict is therefore recorded as unresolved, not confirmed, and `parse_watched_at` continues to interpret both the labeled and unlabeled `Date` formats as UTC on the documentary-evidence reading.

**The hedge (ADR-0040, `bdc4723`/`2484a23`):** `watch_history.watched_at_raw` (schema v4) preserves the verbatim DDP `Date` string alongside the parsed `i64`, so a wrong guess is a query fix, never a re-ingest. Window filters use day-granularity bounds (`--window-start`/`--window-end`, inclusive UTC calendar dates) precisely because those bounds absorb the sub-day ambiguity this verdict leaves open for all but boundary-adjacent rows — only rows within the ambiguity offset (~1h) of a window edge can be misclassified, and the count of such rows is bounded by the offset, not open-ended.

## Deferred / not in 4b

- **Cookie-efficacy run** — operational, not code; the first real `--cookies-file` run against the 301 `SensitiveLoginGated`-pool rows is the experiment. `status --retryable` already surfaces the pool size for that run.
- **`status --logs` sketch idea** — unscoped; no FOLLOWUPS entry filed (file one only if the operator wants it pursued).
- **Epic 5 cleanup bundle** — unrelated pre-existing debt (`docs/followups/epic-5.md`); untouched by 4b.
- **`raw_signals.schema_version` sampling** — a Plan C concern per ADR-0041's guidance; out of scope at Plan B batch sizes (`status --verify` parses every artifact, which is affordable at the current corpus size).

## Next

FOLLOWUPS: all five Epic 4b active-scope entries resolved and archived (`docs/archive/followups-resolved.md`, "Resolved by Plan B Epic 4b"); `docs/followups/epic-4.md` is now a closed-epic pointer stub. No epic is queued next in this plan set as of this close; see `docs/FOLLOWUPS.md`'s remaining scope groups (Epic 2, Epic 5, Plan C, cross-epic) for open debt.

# Plan B Epic 4a — Epic Close

**Branch:** `feat/plan-b-epic-4a`
**Status:** all 8 tasks complete. In-pipeline capped retry with end-of-queue claim ordering (ADR 0036), operator-editable TOML classification with a compiled evidence-derived default and per-batch policy provenance (ADR 0037), and the retirement of the `triage` subcommand + oEmbed probe + `curl` runtime dependency (ADR 0036 supersedes 0034). ADRs accepted at close; ADR 0033 carries a 4a comment.

## What landed

| Task | Commit(s) | Subject |
|---|---|---|
| 01 | `c2bdd6f` + `9a9e675` | `src/classification.rs`: operator-editable TOML policy table + evidence-derived compiled default + `toml` dep + 10240 fixture; three validation gaps closed in review |
| 02 | `49ff3ae` | Schema v3: `batch_runs` table, attempt-aware pending index, v2→v3 migrate ladder, open/close mutators |
| 03 | `a183c86` + `7cb6bac` | `failure.rs` rewire: classification table drives message classes; label strings replace enums; `requires_cookie` coverage + stale-comment scrub |
| 04 | `ecce663` + `16a113a` | `Store::record_fetch_failure`: one-transaction requeue/exhaust/park decision + typed outcome; uniform `{"kind","message"}` event detail; hardened retry tests |
| 05 | `11ef379` | `claim_next` orders `attempt_count ASC` (end-of-queue retries) + ordering tests |
| 06 | `472ab9f` + `c7c4f1b` | Workers + serial dispatch through `record_fetch_failure`; `--retries` (default 1) + `--classification` CLI; review-adjudicated accounting (census gated on landed writes; `ProcessStats` input-side per ADR-0007; `compute_process_stats` deleted) |
| 07 | `f24a433` + `7cc2a6e` | Start-of-batch sweep; `batch_runs` open/close in Process arm; census struct + persist + print; review fixes (UTF-8-safe truncation, preserve-kind-on-fallback, honest sweep-census doc) |
| 08 | `551580a` + `38857c1` + `6067c08` + final-review docs fixes | Retire triage/probe/`curl`; ADR slate (0036, 0037; 0034 superseded; 0033 comment) via `adg`; architecture-doc + src-vm updates; FOLLOWUPS archived with SHAs; this close doc |

Verification at close: `cargo fmt` clean, `clippy --all-targets -D warnings` clean, `cargo test --features test-helpers -- --test-threads=1` green (244 pass, 0 fail; the triage retirement dropped 9 tests — 4 triage integration + 3 cli parse-rate + 2 probe unit; corrects `551580a`'s disclosed delta of 7, which overlooked `probe.rs`'s 2 unit tests), `adg validate` clean (34 ADRs).

## Five shipped-behavior deltas from the plan's literal snippets

Each was disclosed per ADR-0003 in its task commit; the docs and ADRs describe the shipped behavior:

1. `record_fetch_failure` event details use the uniform `{"kind": ..., "message": ...}` JSON shape (not `{"label": ...}`).
2. `ProcessStats` `claimed`/`succeeded`/`failed` are input-side **per-attempt** counters (fail-once-recover ⇒ claimed=2, failed=1, succeeded=1); `compute_process_stats` was deleted; the census copies these semantics.
3. The pipelined workers share ONE outcome-dispatch helper (`handle_record_fetch_failure_outcome` in `src/pipeline/pipelined.rs`); serial keeps its own copy (adjudicated deviation from verbatim duplication).
4. The sweep preserves a row's existing `last_retryable_kind` on FALLBACK classification hits — except empty/NULL/legacy-`"Fetch"` placeholder kinds, which take the fallback label; matched rules and terminal write-offs relabel (`MessageMatch` carries `matched_rule: bool`).
5. `open_batch_run` returns `anyhow::Result<i64>` (identity-insert carve-out from ADR-0006, documented in its doc comment).

## Operator runbook — first 4a batch (pilot DB, 7,087 parked rows)

1. Update the VM per docs/operations/src-vm.md (pull → build → sudo cp).
2. `ddp-transcribe --state-db ~/ddp-state/state.sqlite migrate` — v2→v3
   (batch_runs + index). Idempotent.
3. `ddp-transcribe --state-db ~/ddp-state/state.sqlite --transcripts
   ~/ddp-work/transcripts --whisper-model
   ~/ddp-work/models/ggml-large-v3-turbo-q5_0.bin process
   [--cookies-file ~/tiktok-cookies.txt]` — the sweep runs first
   (expect ~3,915 swept_terminal, ~2,871 requeued, 301 parked without
   cookies), then the drain retries them behind any fresh work; census
   prints at the end and persists to batch_runs.
4. `~/sync-to-storage.sh` after the batch (not while a transfer reads the
   volume).
5. Expected recovery ≈ +2,400 videos (census alive counts); corpus ≈
   91.5–92%. Exhausted/parked remainders are visible in failed_retryable
   by kind.

## Deferred to 4b

- **`status` subcommand** rendering `batch_runs` history (census + policy per run); the ADR-0017 done-contract checks (artifacts-on-disk, `raw_signals.schema_version`, pause-safe predicate) land here.
- **Time-window filter + DDP timezone resolution** (`parse_watched_at` UTC assumption — FOLLOWUPS, Epic 4b).
- **Cookie-efficacy verdict** — the first real cookie run is the experiment; prove whether cookies recover the `SensitiveLoginGated` pool.
- **CLI hardening**: `--retries` / `max_attempts` i64 range validation; config-echo scoped to consumed config (both FOLLOWUPS, Epic 4b).

## First production batch — actual results (2026-07-08; recorded 2026-07-13)

The runbook above ran against the pilot DB (7,087 parked rows) on the SRC A10
workspace as **two `process` invocations** (`batch_runs` rows 1–2):

| run | started (UTC) | finished (UTC) | `--retries` | census |
|---|---|---|---|---|
| 1 | 2026-07-08 11:41 | — (interrupted; row never closed) | 1 | none persisted |
| 2 | 2026-07-08 15:47 | 2026-07-08 16:32 | 2 | persisted ✓ |

Run 1 performed the big sweep and most of the drain, then was interrupted
(workspace pause at 12:11 UTC), leaving an open `batch_runs` row with no
census — see the Epic 4b FOLLOWUPS entry on rendering open runs. Run 2 swept
the leftovers (examined 1,388; requeued 1,087; parked 301 for cookies) and
closed cleanly. Numbers below are reconstructed from the `videos` table
(authoritative current state), cross-checked against run 2's census;
snapshot synced to Research Drive 2026-07-13 and copied locally as
`ddp-run-export.sqlite` (repo root, untracked).

**Disposition of the 7,087 parked rows** (sums exactly):

| Disposition | n | % |
|---|---|---|
| Terminal — 3,915 sweep write-offs + 13 adjudicated dead on retry | 3,928 | 55.4% |
| Recovered on retry | 2,370 | 33.4% |
| Parked awaiting cookies (`SensitiveLoginGated`, placeholder kind `Fetch`) | 301 | 4.2% |
| Retried but exhausted (remain `failed_retryable`) | 488 | 6.9% |

**Retry effectiveness:** 2,871 rows were actually retried (matches the
runbook's ~2,871 projection). 2,370 recovered — **82.5%** overall;
excluding the known-impure `NoPermission` class, **2,344 of 2,420 = 96.9%**.

**Per-kind outcomes** (from `videos`: `last_retryable_kind` × `status`):

| kind | recovered | exhausted | terminal-on-retry |
|---|---|---|---|
| NoDataBlocks | 2,315 | 1 | 6 |
| NoPermission | 26 | 418 | 7 |
| NetworkTransient | 17 | 0 | 0 |
| HttpError | 12 | 1 | 0 |
| FfprobePostprocess | 0 | 36 | 0 |
| NoVideoFormats | 0 | 32 | 0 |

**Calibration verdicts** (vs. the 2026-07-07 probe census in
`PLAN-B-EPIC-4-KICKOFF-PROMPT.md` Step 4):

- `NoPermission`: probe predicted 25 alive of 452 → **26 recovered**. The
  impure-class ruling (keep `retryable`) is vindicated at population scale;
  a blanket `terminal` would have silently discarded these videos.
- `NoDataBlocks`: predicted 2,311/2,318 alive → 2,315 recovered, 7 dead.
- Corpus: **51,903 / 56,620 succeeded = 91.7%** — inside the projected
  91.5–92% band. Remaining `failed_retryable` pool: 789 = 301 cookie-gated
  (Epic 4b live experiment) + 488 exhausted awaiting adjudication.
- New signal: `FfprobePostprocess` (36) and `NoVideoFormats` (32) recovered
  **zero** rows on retry — candidate evidence for a future disposition
  review, though n is small and one retry is a weak oracle for these.

## Next

Epic 4b (operator-facing `status` command, done-contract, timestamps) — sketch at `docs/superpowers/plans/2026-05-12-plan-b/EPIC-4-SKETCH.md`.

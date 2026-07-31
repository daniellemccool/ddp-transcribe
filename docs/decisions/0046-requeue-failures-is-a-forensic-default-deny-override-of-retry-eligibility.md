---
status: accepted
date: "2026-07-30"
source: docs/superpowers/specs/2026-07-30-epic-5b-plan-b-closeout-design.md
category: State machine
applies_to:
    - src/state/mod.rs
    - src/cli.rs
    - src/commands.rs
    - tests/requeue_failures.rs
priority: invariant
---

# requeue-failures is a forensic default-deny override of retry eligibility

## Decision

In-pipeline failure-time requeue (`record_fetch_failure`) remains the normal
retry authority. An operator may explicitly
restore failed rows to pending after an external condition has materially
changed. This is a forensic, default-deny override of eligibility, not an
alternate classifier or retry scheduler; the subsequent fetch remains the
liveness oracle.

## Guidance

- **Eligibility is default-deny.** `requeue-failures` needs at least one *qualifying* selector — `--error-kind <K>` (repeatable), `--max-attempts <N>` (skips rows with `attempt_count >= N`), `--older-than <DUR>` — or an explicit `--all`; a bare invocation is an error, and the *modifiers* `--max <N>` and `--dry-run` never grant eligibility. `--all` means all `failed_retryable` rows and conflicts with every qualifying selector: `--all --older-than 30d` is a parse error, never a silent intersection. Review rejects any relaxation that lets an unqualified invocation touch rows.
- Terminal rows are opt-in twice over: `--include-terminal` requires a qualifying terminal selector alongside it, so `--include-terminal --all` and `--include-terminal --max N` are rejected. Retryable `--error-kind` matches `last_retryable_kind`; terminal matching uses `terminal_reason`, never a retained retryable kind. Kind matching is exact byte equality — no case folding, and no comma splitting (one kind per flag, because operator-authored classification labels may legally contain commas). `--max` and `--max-attempts` are range-checked positive.
- The failure clock is `last_failure_at := MAX(video_events.at)` over the allowlist `'failed_retryable'`, `'failed_terminal'`, `'retry_requeued'`, `'cookie_parked'`. `--older-than D` matches `last_failure_at < now - D` against one `now` per invocation, and a row with no qualifying event never matches. Administrative events — `'requeued'`, `'swept_stale'`, `'swept_terminal'`, `'claimed'`, `'succeeded'` — never reset the clock; tests pin the allowlist and that `swept_terminal` in particular does not reset it. `videos.updated_at` is not touched.
- Eligible rows go to `pending` with `claimed_by`/`claimed_at` defensively cleared. `attempt_count` is never reset or decremented and `last_retryable_*`/terminal fields are retained — the command grants another claim, it does not erase history. Each row gets exactly one `operator_requeued` event whose `detail_json` carries prior status, prior kind/reason, and attempt count, attributed as `worker_id = operator:<hostname>-<pid>` (distinct from the sweep's literal `worker_id = 'sweep'`).
- Post-override arithmetic is exact and belongs in the tests: for pre-requeue `attempt_count = A`, the next claim bumps it to `A + 1`, and in-pipeline requeue on failure happens only while `attempt_count < retries + 1` — so an *automatic* retry after the forced fetch requires `A + 1 < retries + 1`, i.e. `--retries > A` strictly (`--retries = A` is insufficient). Worked example: a row exhausted at `A = 3` under `--retries 2` gets exactly one forced attempt unless the operator runs `process` with `--retries 4` or higher.
- Mechanics are fixed: one `BEGIN IMMEDIATE` transaction shaped like `sweep_stale_claims` (select → update → events); `--max` takes rows in the deterministic order `attempt_count ASC, video_id ASC` (mirroring the claim-order family); `--dry-run` is a read-only SELECT that prints per-kind counts plus a total and writes nothing; zero matches exit 0 with an explicit `0 rows matched` line.
- Sequencing: the transition bypasses the start-of-batch sweep by construction, and the command itself claims nothing — after the next ordinary claim, ordinary in-pipeline retry behavior resumes unchanged.

## Why

Rows blocked by the lifetime attempt cap or already terminalized are
unreachable by any automatic mechanism once the external condition that
failed them changes, and the only other escape hatch is hand-written SQL
that mutates status without leaving the event trail the state machine and
every audit depend on. Default-deny is what keeps a forensic escape hatch
from degrading into a routine "requeue everything" button that launders
history and re-drives dead work.

## Context

The live example is the cap-exhausted tail of the cookie-gated cohort:
`batch::run_sweep` already re-adjudicates parked retryables under the cap
once cookies are fresh, so only cap-exhausted and terminalized rows need an
operator. The command is named `requeue-failures` rather than
`requeue-retryables` because terminal rows are eligible under
`--include-terminal`.

## Alternatives

- **Manual SQL** — remains unsupported emergency repair; acceptable only when it preserves the forensic event invariant, which in practice it does not.
- **Auto-requeue on fresh cookies / raising the cap** — re-drives genuinely dead work indiscriminately and hides the operator's judgement call from the event log.
- **Resetting `attempt_count` instead of overriding eligibility** — destroys retry history and makes the cap unauditable; the override deliberately grants one claim, not a clean slate.

# Plan B Epic 3 — pre-kickoff review of `EPIC-3-SKETCH.md`

**Date:** 2026-06-02
**Reviewer context:** written immediately after merging the architecture doc set
(`docs/reference/architecture/`), so the reviewer had fresh, verified knowledge of
the current failure-classification code state.
**Verified against:** `main` at merge of the architecture-doc-set PR (#8).
**What this reviews:** `EPIC-3-SKETCH.md` (the Epic 3 sketch) + `docs/followups/epic-3.md`.

> This is a sketch-stage review. Epic 3's detailed expansion happens at kickoff.
> These are issues to resolve **before / during** task expansion — not defects in
> shipped code. Several are drift: the sketch was accurate when written, but later
> epics consumed ADR numbers, reshaped modules, and changed what's "done."

---

## Red flags (ordered by likelihood of derailing the epic)

### 🔴 1. Classification without a retry *executor* = rich labels on a dead-end state

**The single most important issue.** On current `main`, `failed_retryable` is a
**sink**: `claim_next` selects only `status = 'pending'` (`src/state/mod.rs`), and
**no code path resets `failed_retryable` → `pending`**. The merged architecture docs
state this explicitly (`state-machine.md`: *"failed_retryable is a sink; Epic 3 adds
retry policy"*; the state-transition diagram draws it as a terminal sink).

The sketch's scope, however, is almost entirely classification **types** — it names
failures richly (`RetryableKind::RateLimited`, `TransientNetwork`, …) but says nothing
about the **requeue / backoff / `attempt_count`-cap machinery** that makes a
`RateLimited` verdict actually cause a retry. Classifying a failure as retryable is
inert if nothing ever re-claims the row.

**Resolve at kickoff:** decide explicitly whether the **retry executor** is in Epic 3
scope or deferred again:
- *In scope:* add a requeue path (`failed_retryable` → `pending` under an
  `attempt_count` cap, with backoff), and define where it runs (startup sweep? a
  dedicated requeue pass? per-kind backoff?). This is a separate, non-trivial piece
  of state-machine + orchestrator work beyond the taxonomy.
- *Deferred:* fine, but then the merged docs' "Epic 3 adds retry policy" claim is
  wrong and must be corrected, and the epic should be framed honestly as
  "classification recording only, retry execution is Epic N."

Either answer is defensible; it must be a **conscious decision**, and the docs must
match it.

### 🔴 2. The stderr fixtures the entire test strategy depends on do not exist

The sketch (lines 48, 54) assumes real failure-stderr was captured *"during Epic 1's
bake"* into `tests/fixtures/yt_dlp_responses/`. **That directory does not exist, and
there are no yt-dlp / stderr fixtures anywhere under `tests/`** (verified). So the
planned table-driven classification tests have **no realistic inputs**.

Capturing them is now a **blocking prerequisite** that needs live network access plus
TikTok URLs still in the right failure state (deleted / private / region-blocked /
age-restricted) — and those states drift over time, so the longer this waits the
harder it gets.

**Resolve at kickoff:** make fixture capture **task 1** (or a pre-epic chore). Budget
it — the sketch's "~1 week" estimate does not. Record, per fixture, the yt-dlp +
ffmpeg + whisper.cpp versions that produced it (ties into the version-pinning risk the
sketch already flags well).

### 🟠 3. `ClassifiedFailure` (and `Acquisition`) cannot represent "terminal / unavailable"

As sketched (line 13), `ClassifiedFailure = Retryable { kind, ctx } | Bug { ctx }` —
two arms. But `UnavailableReason` (`Deleted`, `Private`, `LoginRequired`, …) is fully
specced (line 12), and a stated goal is wiring `mark_terminal_failure` — which has
**no caller** on current `main` (`#[allow(dead_code)]`, `src/state/mod.rs`).

A deleted video is **neither `Retryable` nor a `Bug`** (the sketch reserves `Bug` for
*our* defects, default-cautious). There is no arm to route an "unavailable" verdict to
the terminal mutator. Likewise the fetcher today exposes only:

```rust
// src/fetcher/mod.rs
pub enum Acquisition { AudioFile(PathBuf) }   // no Unavailable variant
```

so the sketch's "emit `Acquisition::Unavailable` for terminal verdicts" requires a new
variant too.

**Resolve at kickoff (design):** the taxonomy needs a third `ClassifiedFailure` arm
(e.g., `Unavailable { reason: UnavailableReason, ctx }`) and a matching
`Acquisition::Unavailable { reason }`, designed coherently so an unavailable verdict
flows fetcher → classifier → `mark_terminal_failure`. This is the wiring that gives
`mark_terminal_failure` its first caller.

### 🟡 4. ADR numbers in the sketch collide with `main`

The sketch reserves **AD0024 / AD0025 / AD0026**. All are taken: `0024` stale-claim
sweep, `0025` shutdown order, `0026` claim contention, `0027` topology. **Next free is
`0028`+.** Trivial, but `adg validate` (pre-commit hook) rejects duplicate numbers, so
renumber when drafting the Epic 3 ADRs.

### 🟡 5. `src/pipeline.rs` is stale

The sketch's "files affected" lists `src/pipeline.rs`. It's a module dir now:
`src/pipeline/{mod,pipelined,serial}.rs`. (Exactly the drift that tripped the
architecture-doc orchestration task — verify all paths against `main`.)

### 🟡 6. The estimate predates absorbed scope

"~5–7 tasks / ~1 week" predates routing these into Epic 3 (per `docs/followups/epic-3.md`):
- `tests/pipeline_fakes.rs` 1000-line refactor (split into ~5 files).
- worker-level vs `run_pipelined`-level test audit.
- `ExitStatusExt::signal()` capture (`signal: Option<i32>`, cfg-gated) to distinguish
  OOM-kill / SIGSEGV / SIGINT.
- `From<RunError> for FetchError` variant split (`ToolNotFound` / `ConfigError` /
  `SystemIo`) and `From<AudioDecodeError>` reclassification.
Plus fixture capture (#2) and the retry executor (#1, if in scope). Realistically more
than 7 tasks; re-estimate at kickoff.

---

## What the sketch gets right (don't relitigate)

- **Default-cautious posture** (unknown stderr → `Retryable`, never `Bug`) — load-bearing, well-stated.
- **stderr-pattern version pinning** — the ADR must record the yt-dlp/ffmpeg/whisper.cpp versions patterns were validated against; re-verify when `whisper-rs` bumps.
- **Capturing signal info** and **refusing `exit_code == 0` as success** — both correct instincts.

---

## Recommended approach for the new session

The flags split cleanly: **#1 and #3 are design under-specification** (settle them in
brainstorming *before* writing tasks); **#2, #4, #5, #6 are mechanical** corrections
the planner folds into task expansion.

So the sequence is: **orient → brainstorm the two design questions → expand the sketch
into a per-task plan**. Do **not** jump straight to `writing-plans` — the taxonomy
shape (#3) and the classification-vs-execution boundary (#1) gate everything
downstream, and getting them wrong means re-cutting tasks.

### Suggested kickoff prompt (copy-paste into a fresh session)

```
We're kicking off Plan B Epic 3 (failure-classification taxonomy). Before any
task planning, two design questions must be settled — read these first:

  - docs/superpowers/plans/2026-05-12-plan-b/EPIC-3-SKETCH.md
  - docs/superpowers/plans/2026-05-12-plan-b/EPIC-3-PREKICKOFF-REVIEW.md
  - docs/followups/epic-3.md

Then orient on the CURRENT code (the sketch has drifted — trust the code, not
the sketch's specifics). The architecture doc set is the fast map:
  - docs/reference/architecture/state-machine.md  (failure classification,
    failed_retryable-as-sink, mark_terminal_failure has no caller)
  - docs/reference/architecture/orchestration.md  (where errors are routed today)
  - src/errors.rs, src/state/mod.rs, src/fetcher/mod.rs, src/pipeline/*.rs

Use superpowers:brainstorming to resolve the two design questions from the
review BEFORE writing any plan:

  1. Scope boundary: does Epic 3 include the RETRY EXECUTOR (requeue
     failed_retryable -> pending under an attempt_count cap + backoff), or only
     classification recording? failed_retryable is currently a dead-end sink;
     the merged architecture docs promise "Epic 3 adds retry policy." Decide,
     and if "classification only," correct that promise in state-machine.md.
  2. Type design: ClassifiedFailure as sketched (Retryable | Bug) can't express
     terminal/unavailable, but UnavailableReason is specced and the epic must
     give mark_terminal_failure its first caller. Design the third arm
     (Unavailable { reason, ctx }) and the matching Acquisition::Unavailable,
     fetcher -> classifier -> mark_terminal_failure end to end.

codex-advisor is appropriate to consult DIRECTLY during this design brainstorm
(it's a design session, not a per-task review — see the using-codex-advisor
skill; the pinned session carries Plan B context). Get its read on the scope
boundary and the taxonomy shape.

Only after those two are settled, use superpowers:writing-plans to expand the
sketch into per-task files. Fold in the mechanical corrections from the review:
  - Task 1 = capture real failure-stderr fixtures into
    tests/fixtures/yt_dlp_responses/ (needs live failing TikTok URLs; record
    tool versions per fixture). This is a prerequisite for the table-driven
    classification tests.
  - Allocate ADR numbers 0028+ (NOT the sketch's 0024-0026, which are taken).
  - Use real module paths (src/pipeline/{mod,pipelined,serial}.rs, not
    src/pipeline.rs).
  - Re-estimate: the ~5-7 task figure predates the pipeline_fakes.rs refactor,
    worker-test audit, signal-capture, and From-impl split now routed here.

Do NOT begin implementation until the plan is reviewed and approved (per
CLAUDE.md).
```

### Notes for whoever runs it

- The **architecture doc set is the onboarding shortcut** — point the new session at
  `state-machine.md` and `orchestration.md` rather than having it re-derive the
  failure-routing reality from source. That's exactly what the doc set is for.
- Treat the sketch as a **snapshot, not live state.** Every concrete specific (ADR
  numbers, file paths, "already done" claims, the estimate) should be re-verified
  against `main`. The flags above are the known-stale ones as of 2026-06-02; assume
  there may be more.

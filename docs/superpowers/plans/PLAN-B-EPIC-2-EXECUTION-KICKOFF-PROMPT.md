# Plan B Epic 2 execution kickoff — paste into a fresh Claude Code session

> **Author note:** Plan B Epic 2's design spec (`docs/superpowers/specs/2026-05-13-plan-b-epic-2-design.md`) and per-task implementation plan (`docs/superpowers/plans/2026-05-13-plan-b-epic-2/`) are both complete. This kickoff drives **execution** — not brainstorming, not plan writing. If you need the brainstorming-phase kickoff, see `PLAN-B-EPIC-2-KICKOFF-PROMPT.md` (sibling file).

---

## Prompt to paste

I want to begin **executing Plan B Epic 2** for the UU TikTok donation-data
transcription pipeline (`/home/dmm/src/uu-tiktok`). The design spec and the
per-task implementation plan are both complete and committed; this session
drives execution.

### Current state

- **Branch:** `feat/plan-b-epic-2` (commit `75668e7` — plan; commit `1f1ba3c`
  on `main` — spec). Confirm with `git branch --show-current`; if not on the
  feat branch, `git switch feat/plan-b-epic-2`. **If the perf-tweaks PR has
  landed on `main` since the plan was committed, the feat branch may need a
  rebase first** — see "Coordination check-ins" below.
- **Spec:** `docs/superpowers/specs/2026-05-13-plan-b-epic-2-design.md`
  (32 KB; decided). **Do not load into subagent context** — the per-task
  briefs are self-contained per AD0001.
- **Plan:** `docs/superpowers/plans/2026-05-13-plan-b-epic-2/` (21 files,
  5,669 lines, 20 tasks across 2 phases). `00-overview.md` is the
  authoritative entry.
- **Codex-advisor session:** pinned UUID is available via `codex-advisor id`.

### Step 1: Orient yourself before dispatching anything

Read in order, **only these**:

1. `docs/superpowers/plans/2026-05-13-plan-b-epic-2/00-overview.md` — file
   structure, ADR slate (AD0021–AD0027), task index for both phases, exit
   criteria, phase boundary discipline, and the three spec corrections the
   plan locks in (tokio-util cargo-deps gap → T13; `src/transcribe.rs`
   additive `WhisperEngineHandle` → T18; 4-arg mutator signatures forced by
   AD0022's `WHERE claimed_by = ?` predicate).
2. `CLAUDE.md` — project-wide ADRs and disciplines (AD0001–AD0020).
3. `docs/decisions/AD0018-*` and `docs/decisions/AD0019-*` — the three-tier
   review protocol and the subagent report format / phase-boundary
   controller restart rules.
4. **Do not** read the spec yourself or via subagents during execution —
   the per-task briefs are self-contained per AD0001, and the overview
   consolidates the cross-cutting context. Loading the spec into a subagent
   burns context for no benefit.
5. **Do not** preemptively read every per-task file. Open them per dispatch.

### Step 2: Surface the execution-model choice to me before dispatching

The plan-writing session deliberately deferred this choice. Present the
options and wait for my answer:

1. **Subagent-driven** (matches AD0018/AD0019 + Plan B Epic 1 precedent):
   per task, you dispatch a fresh Opus implementer with the task's per-task
   file + curated ADRs; a Sonnet spec-compliance reviewer checks the diff
   and delegates code-quality review to codex-advisor; controller commits.
2. **Inline execution** in this session via `superpowers:executing-plans`.
3. **Hybrid** — inline for ADR/schema-heavy early tasks (T1–T4),
   subagent-driven from T5 onward where TDD increments are larger.

Default recommendation if I don't override: option 1 (subagent-driven).
It matches the discipline I've been running for Plan B.

### Step 3: Process inheritance — non-negotiables

Apply all project-wide ADRs (AD0001–AD0020) per `CLAUDE.md`. Highlights
specifically load-bearing for Epic 2 execution:

- **AD0001** — per-task file split. Dispatch one task file per subagent,
  not the whole plan.
- **AD0002** — dead-code suppression strategy; cleanup on consumption.
  T7's `mark_terminal_failure` ships with `#[allow(dead_code)]`; Epic 3
  removes it. T18 explicitly cleans up `Config::whisper_use_gpu` /
  `whisper_threads`.
- **AD0003** — deviation honesty in commits. Every brief deviation
  (clippy-driven cosmetic, structural choice diverging from brief
  verbatim) gets prominent commit-message disclosure.
- **AD0005** — `test-helpers` Cargo feature for library items needed by
  integration tests.
- **AD0006** — `Store` mutators return `Result<usize>` with row-change
  count.
- **AD0008** — artifacts on disk BEFORE `mark_succeeded`. T15's
  `transcribe_and_write` and T17's transcribe worker both preserve this;
  do not let any subagent reverse the order.
- **AD0009–AD0017** — Plan B Epic 1 feature ADRs; compose with, don't
  supersede.
- **AD0018** — **three-tier review with codex-advisor delegated via the
  Sonnet reviewer.** Orchestrator does NOT call codex-advisor directly
  during task reviews. Sonnet spec-compliance reviewer invokes codex via
  `codex-advisor ask <prompt>` (using the pinned session UUID from
  `codex-advisor id`) and distills the response into a written review.
- **AD0019** — subagent report format (≤250-word STATUS / SUMMARY /
  CHANGED / DEVIATIONS) and phase-boundary controller restart. At Phase 1
  close (after T11), write `PHASE-1-CLOSE.md` (≤1 page: what landed,
  current state, Phase 2 entry point) and END THIS SESSION. Phase 2
  starts fresh with the spec + close-out + Phase 2 task list.
- **AD0020** — FOLLOWUPS document structure and lifecycle. At Epic 2
  close (T20), move resolved entries from `docs/followups/epic-2.md` to
  `docs/archive/followups-resolved.md` with the resolving commit SHAs.

### Step 4: Verification before any "done" claim

Per `superpowers:verification-before-completion`: before claiming a task
complete or committing, run:

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers
```

The pre-commit hook in `.githooks/pre-commit` runs `adg validate` — if
the hook doesn't fire on a fresh clone, `git config core.hooksPath
.githooks` once. Do not bypass.

### Step 5: Single-flight Agent dispatch

The thermal lock at `~/.claude/hooks/agent-lock-acquire.sh` enforces one
Agent dispatch at a time (per Plan A retro). Don't try to parallelize
subagent tasks within the same phase — TDD dependency order across
T1→T11 makes that unsafe anyway.

### Coordination check-ins

- **perf-tweaks merge state**: a sibling worktree at
  `/home/dmm/src/uu-tiktok-perf-tweaks` (branch `feat/perf-tweaks`) landed
  several efficiency items and **its own `AD0021` (bounded subprocess
  output capture)**. The PR's expectation is "Epic 2 session: please
  rebase `feat/plan-b-epic-2` onto main after this lands." Before
  dispatching, check:

  ```bash
  git -C /home/dmm/src/uu-tiktok log --oneline main..HEAD | head -5
  git fetch origin && git log --oneline HEAD..origin/main 2>&1 | head -5
  ```

  If perf-tweaks has merged to main, the rebase is required AND Epic 2's
  ADR numbering needs to shift (perf-tweaks owns AD0021 = bounded
  capture; Epic 2's AD0021–AD0025 shift +1 to AD0022–AD0026; Epic 2's
  AD0026 = bounded capture is **dropped** because perf-tweaks' AD0021
  covers it; Epic 2's AD0027 = orchestrator topology stays at AD0027).
  Update plan files accordingly before drafting ADRs in T1/T12. **If
  unsure, ask the operator before mutating plan numbering.**
- **T14 scope after rebase**: spec § "Coordination check-ins" (line 184)
  states T14's scope reduces if perf-tweaks lands bounded capture first.
  After rebase, T14 becomes: rename `ring_buffer_tail` → `tail_excerpt`
  (if not already done) + symmetric `stdout_capture_bytes` (Epic 2's
  contribution; per the PR note, no new ADR needed — composes with
  perf-tweaks' AD0021) + verify the existing flood test passes.

### What NOT to do

- Do NOT load the full spec or all 20 per-task files into a subagent's
  context. Curated dispatch per AD0019 — one task file + the overview +
  the curated ADRs declared in that task's brief.
- Do NOT call codex-advisor directly during task reviews. Sonnet reviewer
  delegates per AD0018. (You may consult codex-advisor at orchestrator
  level for spec/plan-level questions, but not as part of the per-task
  review tier.)
- Do NOT skip the PHASE-1-CLOSE.md handoff. Phase 2 in the same session
  context violates AD0019 and tends to drag Phase 1's still-warm
  decisions into Phase 2 by accident.
- Do NOT fold Epic 3 typed-enum classification work into Epic 2. The
  overview's "What Epic 2 Deliberately Omits" section is authoritative.
- Do NOT push to `origin` without explicit approval.
  `feat/plan-b-epic-2` is local-only as of the plan commit.
- Do NOT auto-renumber ADRs unprompted. If perf-tweaks has merged,
  surface the renumber plan to the operator before applying it.

### Done state for this session

- (Phase 1 controller, subagent-driven) T1–T11 committed; `cargo test
  --features test-helpers` green on the feat branch; `PHASE-1-CLOSE.md`
  written next to `00-overview.md`; session ends after `PHASE-1-CLOSE.md`
  commit. Operator restarts for Phase 2.
- (Phase 1 controller, inline) same outcomes; whether to continue into
  Phase 2 within the same session is the operator's call at T11 close.
- (Operator runbook) the operator may open a PR against `main` from
  `feat/plan-b-epic-2` at any time (typically after Phase 1 ships) but
  doing so before Phase 2 is unusual and warrants a check-in.

### Related references

- Plan A retro and Plan B Epic 1's bake notes: `docs/SRC-BAKE-NOTES.md`
  and `docs/SRC-BAKE-CHECKLIST.md`.
- The brainstorming kickoff that produced the Epic 2 spec (for context
  on design decisions, not for re-litigation):
  `docs/superpowers/plans/PLAN-B-EPIC-2-KICKOFF-PROMPT.md`.
- Active FOLLOWUPS for Epic 2: `docs/followups/epic-2.md`.
- Test fixture for bake runbooks:
  `tests/fixtures/ddp/news_orgs/` (with README explaining naming).

---

[End of execution kickoff prompt]

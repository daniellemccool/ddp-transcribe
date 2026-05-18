# UU TikTok Pipeline — Perf-tweaks Worktree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-worktree-setup.md` … `11-followups-update.md`). Open only the task you're working on. Do NOT load the full design spec or all task files into a subagent's context — the per-task files are self-contained.

**Goal:** Land six small efficiency/robustness tweaks across yt-dlp, whisper.cpp invocation, and subprocess plumbing on a dedicated `feat/perf-tweaks` worktree, plus AD0021 (bounded subprocess capture) and FOLLOWUPS retirements. Designed to land **before** Plan B Epic 2 begins T11 to avoid collision on `src/pipeline.rs` and `src/process.rs`.

**Architecture:** Eleven sequential commits on a `feat/perf-tweaks` branch branching off `main`. Safe-first ordering: pure cleanups land first (#1 lazy lang_state, #6 ffmpeg flags, #3a compact JSON), then the bigger #5 bounded-capture redesign + its AD0021, then the two bake-gated behavioral changes (#4 yt-dlp `-S` sort, #2 `set_no_timestamps`) each followed by a bake-validation commit. Final commit retires resolved FOLLOWUPS entries with backfilled commit SHAs.

**Tech Stack:** Existing Rust 2021 stack (`tokio`, `whisper-rs = 0.16.0`, `hound`, `serde_json`). No new dependencies; one new test-only dependency may be needed for #5 Test C (a child-process emitter — covered in T5).

**Reference:** Full design in `docs/superpowers/specs/2026-05-13-perf-tweaks-design.md` (commit `62776ab` on main). The plan implements that spec verbatim; the spec is the source of truth for "why." **Subagents implementing tasks should not need to open the spec** — each task file is self-contained.

**Coordination with Plan B Epic 2 (other session):** This worktree must merge to `main` before Epic 2 begins T11. After this worktree merges, Epic 2 rebases on the updated `main` and inherits the changes. Item #5 in this worktree pre-empts Epic 2's anticipated T13 bounded-capture work; Epic 2's T13 will only need to absorb symmetric stdout policy decisions on top of AD0021, not re-author the ADR.

---

## File Structure (after the worktree merges)

```
uu-tiktok/
├── Cargo.toml                    # +tests/transcribe_lang_state, +tests/process_bounded_capture
├── src/
│   ├── main.rs                   # unchanged
│   ├── cli.rs                    # unchanged
│   ├── config.rs                 # unchanged
│   ├── errors.rs                 # unchanged (reuses existing TranscribeError::Bug)
│   ├── canonical.rs              # unchanged
│   ├── process.rs                # REWRITTEN (T5): streaming bounded capture, Option<Vec<u8>> stdout, ring_buffer_tail removed, peak_buffer_len test instrumentation
│   ├── state/                    # unchanged
│   ├── fetcher/
│   │   ├── mod.rs                # unchanged
│   │   └── ytdlp.rs              # MODIFIED (T3 ffmpeg flags, T7 -S sort flag)
│   ├── transcribe.rs             # MODIFIED (T2 lazy lang_state, T9 set_no_timestamps)
│   ├── output/
│   │   ├── mod.rs                # unchanged
│   │   └── artifacts.rs          # unchanged (test extension in T4 only)
│   ├── ingest.rs                 # unchanged
│   └── pipeline.rs               # MODIFIED (T4 one-line compact JSON swap)
└── tests/
    ├── canonical.rs              # unchanged
    ├── ingest.rs                 # unchanged
    ├── pipeline_fakes.rs         # unchanged
    ├── e2e_real_tools.rs         # unchanged
    ├── output_artifacts.rs       # EXTENDED (T4 compact JSON tests)
    ├── transcribe_lang_state.rs  # NEW (T2)
    └── process_bounded_capture.rs # NEW (T5)
```

**Files NOT changed in this worktree** (note for reviewers and for the Epic 2 session reading this plan):

- `src/state/*` — Plan B Epic 2 work
- `src/main.rs`, `src/cli.rs` — Plan B Epic 4 work
- `src/output/artifacts.rs` (the `RawToken` struct itself) — partial-deferral; only the test file `tests/output_artifacts.rs` gets an extension in T4. Dropping the `text` field is deferred to Plan C per AD0010 amendment.

---

## Task Conventions (inherited from Plan A/B unchanged)

- **TDD where it applies.** Implementation-bearing tasks (T2, T5) write the failing test first, run it to confirm the failure, write the minimum implementation, run to confirm pass, then commit. Mechanical tasks (T3 ffmpeg flag string, T4 single-line swap, T7 `-S` flag, T9 single-line `set_no_timestamps`) compress the TDD ceremony — extend an existing arg-presence assertion and verify it passes after the change.
- **Commit per task** with the focused message supplied in each task file. The plan supplies the message body verbatim where it's load-bearing (especially for T1's worktree-setup-only commit and T8/T10's bake commits).
- **`cargo test` runs cleanly at the end of every task.** If a step adds a test that depends on later code, mark `#[ignore]` until the supporting code lands. None of the tasks in this worktree need that — each task's test passes by the end of its own commit.
- **No `unwrap()` in non-test code** unless justified by an invariant the type system enforces.
- **Run `cargo fmt --all && cargo clippy --all-targets -- -D warnings` before each commit.** If clippy fires, fix the lint or `#[allow]` it with a one-line justification comment.
- **AD0003 deviation honesty.** Every brief deviation (clippy-driven cosmetic fixes, structural choices that diverge from verbatim brief) gets prominent disclosure in commit message bodies.
- **AD0005 test-helpers feature.** New integration test files get `[[test]] required-features = ["test-helpers"]` in `Cargo.toml`. T2 (`transcribe_lang_state.rs`) and T5 (`process_bounded_capture.rs`) both register.
- **Branch placement (AD0001).** All commits in this plan land on `feat/perf-tweaks`. The AD0021 ADR (T6) is feature-derived and rides the branch into main with the code.

## Review cycle (inherited from Plan B Epic 1)

Three-tier review per dispatch (per AD0018):

| Tier | Role | What it checks |
|------|------|----------------|
| **Opus or Sonnet implementer** | Writes the code per the task brief | TDD discipline; brief-verbatim implementation; ADR compliance; AD0003 deviation honesty in commits |
| **Sonnet spec-compliance reviewer** | Mechanical "does this match the brief" check | Brief steps were followed verbatim modulo documented deviations; ADRs declared in the task brief are honored; AD0002 dead-code cleanup applied; clippy/fmt clean |
| **codex-advisor code-quality reviewer** | Qualitative correctness, called by the Sonnet reviewer per AD0018 (not directly by the controller) | Subtle correctness issues; cross-file consistency; race conditions; lifetime/Send/Sync hazards; perf footguns; testing gaps |

**Cost-quality calibration** (carrying over from Plan B Epic 1):

- **Opus implementer** for tasks with subtle interactions: T2 (lazy alloc + worker thread + new test pattern), T5 (streaming reader + cancellation-safe + multi-call-site update).
- **Sonnet implementer** for mechanically tractable tasks: T1 (worktree setup, no code), T3 (string change with validation), T4 (one-line swap), T6 (ADR via `adg`), T7 (one new arg), T9 (one new line).
- **Sonnet reviewer** is sufficient across the board for spec-compliance — that work is mechanical.
- T8 (bake #4) and T10 (bake #2) are operator-runbook commits with no Rust changes; they go to whoever has access to the bake fixture. No Agent dispatch.
- T11 (FOLLOWUPS update) is a docs-only sweep that can be done inline by the controller after the implementation commits land.

**Single-flight Agent dispatch** still applies (thermal lock from Plan A).

---

## Architectural Decision Records (ADRs) — perf-tweaks worktree

ADRs live in `docs/decisions/` and are managed via the `adg` tool. The format is MADR.

This worktree adds exactly one ADR:

| Proposed ADR # | Title | Branch | Drafted in |
|---|---|---|---|
| AD0021 | Bounded subprocess output capture via streaming `VecDeque<u8>` | feat/perf-tweaks | T6 |

**Numbering note.** The Epic 2 sketch at `docs/superpowers/plans/2026-05-12-plan-b/EPIC-2-SKETCH.md` anticipated this as AD0023, but AD0018/AD0019/AD0020 (meta-process ADRs) landed on `main` after the sketch was written. The next available number at plan write is **AD0021**. Epic 2's remaining anticipated ADRs renumber from AD0022 onward.

**Curated dispatch** (RETRO meta-process improvement #3): each per-task brief in this plan declares which ADRs are directly relevant to that task. Subagents read those plus the overview, not all ADRs.

**Authorship convention** (from Plan A): the controller writes ADRs. T6's task brief is the canonical content; the implementer subagent runs `adg add` and pastes the brief's body into the ADR shell.

---

## Task Index — perf-tweaks worktree

| # | File | Subject | Spec commit | Implementer dispatch | ADRs touched |
|---|------|---------|------------|----------------------|--------------|
| 1 | [01-worktree-setup.md](./01-worktree-setup.md) | Create `feat/perf-tweaks` worktree, no code change | 0 | Sonnet (or operator) | — |
| 2 | [02-lazy-lang-state.md](./02-lazy-lang-state.md) | Lazy-allocate `lang_state` on first opt-in request | 1 | Opus | AD0001, AD0005, AD0016 |
| 3 | [03-explicit-ffmpeg-flags.md](./03-explicit-ffmpeg-flags.md) | Make ffmpeg postprocessor flags explicit (with pre-impl `yt-dlp -v` validation) | 2 | Sonnet | AD0014 |
| 4 | [04-compact-json.md](./04-compact-json.md) | Compact JSON for transcript metadata (`to_vec`, not `to_vec_pretty`) | 3 | Sonnet | AD0008, AD0010 |
| 5 | [05-bounded-process-capture.md](./05-bounded-process-capture.md) | Streaming bounded stdout/stderr capture, `Option<Vec<u8>>` stdout, remove `ring_buffer_tail` | 4 | Opus | AD0001, AD0005, AD0021 (next task) |
| 6 | [06-ad0021-bounded-capture-adr.md](./06-ad0021-bounded-capture-adr.md) | Author AD0021 via `adg add` | 5 | Sonnet | AD0001, AD0021 |
| 7 | [07-ytdlp-sort-fallback.md](./07-ytdlp-sort-fallback.md) | Add `-S +size,+br,+res,+fps` to yt-dlp args | 6 | Sonnet | — |
| 8 | [08-bake-ytdlp-sort.md](./08-bake-ytdlp-sort.md) | Bake #4 against `news_orgs` fixture; capture pre/post; write SRC-BAKE-NOTES.md | 7 | Operator (or Sonnet w/ A10 access) | AD0017 |
| 9 | [09-no-timestamps.md](./09-no-timestamps.md) | `params.set_no_timestamps(true)` | 8 | Sonnet | — |
| 10 | [10-bake-no-timestamps.md](./10-bake-no-timestamps.md) | Bake #2 with tight signal-equality gate (token sequence + p/plog within 1e-6 + per-window no_speech_prob); revert if any check fails | 9 | Operator (or Sonnet w/ A10 access) | AD0010, AD0017 |
| 11 | [11-followups-update.md](./11-followups-update.md) | Retire L47/L48/L87 to archive; amend L89; backfill commit SHAs | 10 | Sonnet (or controller inline) | AD0020 |

---

## Exit Criteria

After Task 11 is committed, the following hold:

1. `cargo test --all-features` passes on the dev machine (no GPU needed for any test in this worktree — T2 + T5 use injected counters; bake tests run separately).
2. `cargo fmt --all && cargo clippy --all-targets -- -D warnings` is clean.
3. **Bake gates passed for #2 and #4** (or revert commits landed if a gate failed, with `docs/bake-findings.md` entries documenting why).
4. `docs/decisions/AD0021-*.md` exists as `decided` and `docs/decisions/index.yaml` reflects it.
5. `docs/FOLLOWUPS.md` no longer references resolved L47/L48/L87 scope-index lines; `docs/archive/followups-resolved.md` carries the resolved bodies with their resolving commit SHAs; L89 is amended to reflect the partial #3a landing.
6. The `feat/perf-tweaks` branch is mergeable to `main` with no conflicts (an Epic 2 rebase on `main` post-merge inherits the changes cleanly).
7. The cross-session contract holds: Epic 2's T11 has not started yet, and the merge happens before Epic 2's T11 begins.

**The worktree is done when the above hold and a PR is opened or a direct merge to main is executed**, per CLAUDE.md's HTTPS-via-gh-CLI git workflow.

---

## What This Worktree Deliberately Omits

These are deferred to other epics or worktrees. Listed so the engineer doesn't accidentally implement them now:

- Dropping the `text` field from `RawToken` in `src/output/artifacts.rs` (the "#3b" half of FOLLOWUPS L89) — **Plan C**, gated on AD0010-amendment ADR + bake validation that downstream filtering still works.
- Symmetric stdout policy decisions beyond `stdout_capture_bytes` (e.g., per-tool defaults) — **Plan B Epic 2 T13** absorbs these on top of AD0021.
- Bin/lib structural reassessment per AD0002 deferred decision — **Plan B Epic 5**.
- Failure classification (`RetryableKind`, `UnavailableReason`) — **Plan B Epic 3**.
- Schema-version handling on `Store::open` — **Plan B Epic 2 first task**.
- `cli.rs` time-window flags — **Plan B Epic 4**.
- The `TranscribeError::StateCreate` variant proposed during brainstorm — **rejected in spec**; lazy `create_state` failure uses existing `TranscribeError::Bug { detail }` until Epic 3 reclassifies (matches AudioDecodeError → Bug convention).

---

## Self-Review Checklist (run by author after writing)

**Spec coverage:** Each of the six items + FOLLOWUPS update + AD0021 from the spec maps to exactly one task. T2 ↔ #1, T3 ↔ #6, T4 ↔ #3a, T5 ↔ #5, T6 ↔ AD0021 ADR, T7 ↔ #4, T9 ↔ #2, T8/T10 ↔ bakes, T11 ↔ FOLLOWUPS updates. T1 is worktree mechanics (spec commit 0). Total: 11 plan files matching 11 spec commits.

**Placeholder scan:** None of the no-placeholder anti-patterns appear. Each TDD step has actual code; each command has expected output; each commit message body is the verbatim text to use (multi-line via heredoc).

**Type consistency:** `CommandSpec`, `CommandOutcome`, `WhisperEngine`, `TranscribeError::Bug`, `Arc<AtomicUsize>` counter pattern, `peak_buffer_len`, `lang_state_allocations`, `stdout_capture_bytes`, `Option<Vec<u8>>` shape — all used consistently across the relevant tasks (T2 + T5).

**Scope:** 11 tasks, each producing a meaningful increment with TDD + commit (or bake notes + commit for T8/T10). Total worktree footprint: ~10 commits across ~6 source files + ~2 new test files + 1 new ADR + FOLLOWUPS archive moves.

**Ambiguity:** Each step shows exact code, exact commands, and expected output. Module wiring (`mod` declarations) where needed is called out per task. Test-feature gating is documented inline. ADR numbering coordination with the Epic 2 session is explicit.

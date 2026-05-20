# Architecture Doc Set — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-skeleton-pass.md` … `09-linkcheck.md`). Open only the task you're working on. Each task file is self-contained — you should not need to open the design spec to execute a task.

**Goal:** Produce a 5-file architecture reference doc set under `docs/reference/architecture/` that lets a new collaborator load the uu-tiktok pipeline into their head without reading the code linearly. The doc covers the entire pipeline (DDP ingest → fetch → transcribe → output) at *design depth*, including the integration choices made at external-tool boundaries (yt-dlp, whisper-rs/whisper.cpp, sqlite via rusqlite). It redirects to ADRs for rationale rather than restating it.

**Architecture:** Five markdown files grouped by data-lifecycle stage. The index file is the spine — orientation, glossary, donor's-journey walk-through, ADR map. Four deepdive files cover one lifecycle stage each: data input (ingest + fetcher), state machine, transcription (audio prep + whisper-rs + output), and orchestration (pipeline + supervision). Two deepdive files (`state-machine.md`, `orchestration.md`) carry an in-flight stamp because Plan B Epic 2 is actively reshaping them; the stamp lifts at Epic 2 close.

**Tech Stack:** Markdown only. Fenced ASCII diagrams (no mermaid). Inline `src/path/file.rs:N` citations matching the `whisper-cpp-deepdive.md` precedent. Inline ADR refs matching the `(per [ADR NNNN](../../decisions/NNNN-…md))` form.

**Reference:** Full design in `docs/superpowers/specs/2026-05-20-architecture-doc-set-design.md`. Each task file repeats the relevant spec content inline; **executors should not need to open the spec**.

---

## Cross-cutting conventions (applies to every task)

1. **ADR-redirect-first writing posture.** Where an ADR captures the rationale for something, the architecture doc points at the ADR rather than restating it. The architecture doc owns the noun layer (what's there) and the narrative layer (how a donor's data flows through). ADRs own the verb layer (why we chose this).

   Inline form:
   ```markdown
   The orchestrator runs an n=3-fetch + 1-transcribe topology with an mpsc
   channel of capacity 2 (per [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md)).
   The capacity bounds backpressure between fetchers and the transcriber.
   ```

   At most half a sentence of generic *why* in the architecture doc. Anything deeper goes in the ADR.

2. **Citation style.** Inline `src/path/file.rs:N` for any specific behavioral claim. Cite the file in all cases; cite the line when a specific location is in mind. Line numbers drift over time — that's accepted; the file path stays valid.

3. **In-flight stamp.** At the top of `state-machine.md` and `orchestration.md`:

   ```markdown
   > ⚠ **As of commit `<sha>`.** This subsystem is being reshaped in
   > [Plan B Epic 2](../../superpowers/plans/2026-05-13-plan-b-epic-2/).
   > Expect revision at epic close — names, contracts, and topology may move.
   ```

   Replace `<sha>` with the output of `git rev-parse --short HEAD` at the time the deepdive is written.

4. **Diagrams.** ASCII only, in fenced code blocks. Two diagrams: a topology diagram in `orchestration.md` and a state-transition diagram in `state-machine.md`. No other diagrams.

5. **What stays out.**
   - External-tool documentation in general (no yt-dlp man-page; no whisper.cpp internals — `whisper-cpp-deepdive.md` already covers those; no sqlite tutorial).
   - ADR rationale (link, don't restate).
   - Test enumeration / how-to-run-tests.
   - Build instructions, CUDA flags.
   - CLI flag reference.

---

## File map

```
docs/reference/architecture/
├── index.md                ~230 lines   (orientation, glossary, donor's journey, ADR map)
├── data-input.md           ~210 lines   (ingest + fetcher, yt-dlp integration depth)
├── state-machine.md        ~190 lines   ★ in-flight stamp
├── transcription.md        ~220 lines   (audio prep + whisper-rs + output)
└── orchestration.md        ~190 lines   ★ in-flight stamp
```

Plus cross-cutting additions in T08:
- One pointer in `CLAUDE.md` under "Default working patterns".
- One entry in `docs/FOLLOWUPS.md` under the active epic.

---

## Task index

| # | Task | File | Output |
|---|------|------|--------|
| T01 | Skeleton pass | `01-skeleton-pass.md` | 5 files with section headers; commit |
| T02 | Index foundations + walk-through stubs | `02-index.md` | `index.md` populated except walk-through prose |
| T03 | data-input.md | `03-data-input.md` | Full ingest + fetcher coverage with yt-dlp integration depth |
| T04 | transcription.md | `04-transcription.md` | Full audio + whisper-rs + output coverage |
| T05 | state-machine.md | `05-state-machine.md` | Full state coverage; ASCII state-transition diagram |
| T06 | orchestration.md | `06-orchestration.md` | Full orchestrator coverage; ASCII topology diagram |
| T07 | Walk-through fill-in | `07-walkthrough-fillin.md` | Replace `index.md` walk-through stubs with prose threading the deepdives |
| T08 | Cross-cutting additions | `08-cross-cutting.md` | `CLAUDE.md` pointer + `FOLLOWUPS.md` entry |
| T09 | Linkcheck pass | `09-linkcheck.md` | Verify every ADR ref, file:line citation, inter-file link resolves |

**Write order is the same as task order.** Stable subsystems (data-input, transcription) go before in-flight subsystems (state-machine, orchestration) so the in-flight content reflects the most recent code state.

---

## Per-task commit policy

One commit per task, message format `docs(reference): <task summary>`. The architecture doc set is not subject to the codex-advisor / Sonnet review tier per ADR 0018 — that tier governs code review. Verification before completion (`cargo fmt && cargo clippy && cargo test`) does **not** apply to these tasks because no code is changed.

The only mechanical check that applies during commits is the `adg validate` pre-commit hook, which validates the ADR collection — adding a markdown file under `docs/reference/architecture/` should not affect ADR-model validation, but a failed validate run during one of these tasks would surface an unrelated bug in `docs/decisions/` that the executor should escalate to the user rather than bypass.

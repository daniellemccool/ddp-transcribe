# Architecture doc set — design

**Status:** decided (design); user review pending before `writing-plans` transition.
**Author:** Danielle McCool (with Claude Opus 4.7 as drafting collaborator).
**Date:** 2026-05-20.
**Inputs:** existing repo doc layout (`docs/`), 27 ADRs in `docs/decisions/`, the `whisper-cpp-deepdive.md` reference precedent, `CLAUDE.md` working disciplines.

## Goal

Produce a multi-file architecture reference doc set under `docs/reference/architecture/` that lets a new collaborator load the uu-tiktok pipeline into their head without reading the code linearly. The doc set covers the *entire pipeline* (DDP ingest → fetch → transcribe → output) at *design depth* — including the integration choices we made at external-tool boundaries (yt-dlp, whisper-rs/whisper.cpp, sqlite via rusqlite) — and redirects to ADRs for design rationale rather than restating it.

The doc set is written for active development: two subsystems (`state/` and `pipeline`) are being reshaped by Plan B Epic 2 right now, so those files carry an explicit in-flight stamp pointing at the active plan, and accept one revision pass at Epic 2 close.

## Audience and scope

**Primary audience:** a new developer or collaborator joining the project who knows Rust but not the project's vocabulary or conventions. The doc assumes Rust competence; it does not assume familiarity with TikTok's DDP, whisper.cpp, yt-dlp specifics, or the project's lifecycle vocabulary.

**Scope rule — "design depth":**
- Cover everything we designed, in our code or at external-tool boundaries.
- *At external boundaries:* cover the exact integration surface — the flags we pass to yt-dlp and why, the whisper-rs `WhisperContextParameters` and `FullParams` we set, our cancellation callback wiring, our sqlite schema and connection model. If a flag's name is self-explanatory, naming it is enough; if not, describe what it does and why we chose it.
- *Stop at:* the external tool's general documentation, command-line surface, or internals. No yt-dlp man-page material, no whisper.cpp internals (the existing `whisper-cpp-deepdive.md` already covers those at external-tool depth), no sqlite tutorial.
- *Redirect for rationale:* where an ADR captures the *why*, the architecture doc points at the ADR rather than restating it. The architecture doc owns the noun layer (what's there) and the narrative layer (how a donor's data flows through). ADRs own the verb layer (why we chose this over alternatives).

## File structure

```
docs/reference/architecture/
├── index.md                ~230 lines
├── data-input.md           ~210 lines
├── state-machine.md        ~190 lines    ★ in-flight stamp
├── transcription.md        ~220 lines
└── orchestration.md        ~190 lines    ★ in-flight stamp
```

~1040 lines total across 5 files. Grouped by data-lifecycle stage rather than by source-tree module — `data-input.md` covers `src/ingest.rs` and `src/fetcher/`; `transcription.md` covers `src/transcribe.rs`, `src/audio.rs`, and `src/output/`; `orchestration.md` covers `src/pipeline.rs` and `src/process.rs`. Utility modules (`src/canonical.rs`, `src/cli.rs`, `src/config.rs`, `src/errors.rs`) are not load-bearing in the architecture and are mentioned only where they appear in flows.

### Per-file responsibilities

| File | Subject | Source-tree coverage |
|------|---------|----------------------|
| `index.md` | The spine — orientation, glossary, end-to-end walk-through, ADR map | (none directly; threads through all subsystems) |
| `data-input.md` | DDP ingest path + video fetcher (yt-dlp integration depth) | `src/ingest.rs`, `src/fetcher/` |
| `state-machine.md` | SQLite schema, lifecycle states, claim contention, mutators, crash-recovery | `src/state/` |
| `transcription.md` | Audio prep, whisper-rs integration depth, artifact writing | `src/transcribe.rs`, `src/audio.rs`, `src/output/` |
| `orchestration.md` | Topology, control loop, supervision, shutdown | `src/pipeline.rs`, `src/process.rs` |

## Per-file internal structure

### `index.md` — the spine (six sections)

1. **What this system is and who it serves** (~30 lines, 2-3 paragraphs). What the system does end-to-end. Who the donor / researcher / DDP relationship is. Explicit out-of-scope statement: no UI, no scheduling layer, no multi-tenant story.
2. **Glossary** (~50 lines). Alphabetical. Project vocabulary a new reader will encounter immediately: `claim`, `lifecycle state`, `mark_succeeded`, `stale claim`, `artifact`, `engine state`, `donor`, `DDP`, `retryable failure`, `mpsc payload`, etc. External integrations (yt-dlp, whisper-rs/whisper.cpp, rusqlite/sqlite, hound) appear as one-line glossary entries. Each term gets 1-2 sentences plus the file where it's defined.
3. **The donor's-journey walk-through** (~80 lines). One paragraph per stage; each paragraph ends with `→ see {deepdive}.md`:
   - Stage 1: DDP export lands, parsed (→ `data-input.md`).
   - Stage 2: Videos enqueued in state, claimed by orchestrator (→ `state-machine.md` + `orchestration.md`).
   - Stage 3: Video fetched, audio extracted, transcribed (→ `data-input.md` + `transcription.md`).
   - Stage 4: Artifact written, then `mark_succeeded` — per ADR 0008 (→ `transcription.md` + `state-machine.md`).
   - Stage 5: Failure / retry / cancellation / supervision (→ `orchestration.md` + `state-machine.md`).
4. **ADR map** (~50 lines). Table mapping every ADR present in `docs/decisions/` at the time of writing (currently 0001-0027) to its governing subsystem/group, sorted by group. Deepdive files carry the relevant subset of this table at their end.
5. **Where to look for X** (~20 lines). Pointer table: builds (`Cargo.toml`, CUDA flag), tests (`cargo test --features test-helpers -- --test-threads=1`), active plans, FOLLOWUPS, decisions (`scripts/adr`), scripts.
6. **How this doc is maintained** (~20 lines). The in-flight stamp convention, the citation style, what triggers an update, how revision at epic-close works.

### Deepdive files — organic structure per file

The deepdive files do *not* use a uniform template (the whisper-cpp-deepdive's "Layout / Key types / Key flows / Invariants / Failure modes / ADRs" structure was right for internalizing an external codebase; uu-tiktok's doc has a different job). Each file uses the section structure that fits its content.

- **`data-input.md`** — two halves.
  - *Ingest* (`src/ingest.rs`): the DDP-export shape we accept, the parsing strategy, what becomes a row in state.
  - *Fetcher* (`src/fetcher/`): subprocess wrapping pattern, the exact yt-dlp flags we pass with effect-and-rationale for each, bounded output capture (per ADR 0021), timeout policy, retry classification, audio-extraction handoff to `src/audio.rs`.
- **`state-machine.md`** — in-flight stamp at top; then schema and lifecycle states, claim contention (per ADRs 0026 and `BEGIN IMMEDIATE`), stale-claim sweep (per ADR 0024), mutator contracts (per ADRs 0006, 0023), schema-version policy (per ADR 0022), crash-recovery durability (per ADR 0008), failure-classification surface (retryable vs terminal).
- **`transcription.md`** — three sections.
  - *Audio prep* (`src/audio.rs`): the float32 PCM 16kHz mono invariant (per ADR 0014), how hound is used, where the data comes from.
  - *Transcription* (`src/transcribe.rs`): version pin (per ADR 0009), `WhisperContextParameters` and `FullParams` choices, cancellation callback wiring (per ADR 0012), GPU verification (per ADR 0013), explicit non-use of `whisper_full_parallel` (per ADR 0015), engine-state model (per ADR 0016), raw-signals extraction (per ADR 0010). Cites into `whisper-cpp-deepdive.md` for the upstream "why" where needed.
  - *Output* (`src/output/`): artifact shape, schema versioning (per ADR 0010), sharding (per ADR 0004), the artifact-before-`mark_succeeded` invariant (per ADR 0008).
- **`orchestration.md`** — in-flight stamp at top; then topology (per ADR 0027 — n=3 fetch + 1 transcribe, mpsc payload, capacity 2), the control loop (claim → dispatch → join), supervision and shutdown order (per ADR 0025 — JoinSet + CancellationToken sequencing), failure handling and retry classification, the batch validation contract (per ADR 0017).

## Cross-cutting conventions

### ADR-redirect-first writing posture (the headline rule)

Where an ADR captures the rationale for something, the architecture doc points at the ADR rather than restating it. The architecture doc's job is the *noun* layer (what's there, what connects to what, what each thing's responsibility is) and the *narrative* layer (how the donor's journey threads through the parts). The ADRs own the *verb* layer of "why we chose this over alternatives" and stay the single source of truth for that.

Inline form — one-line *what* + ADR ref, with at most half a sentence of generic *why*:

> The orchestrator runs an n=3-fetch + 1-transcribe topology with an mpsc channel of capacity 2 (per [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md)). The capacity bounds backpressure between fetchers and the transcriber.

Not:

> The orchestrator runs an n=3-fetch + 1-transcribe topology because GPU saturation occurs at ~1 transcribe task on the A10 dev workspace while CPU/network are mostly idle, so the remaining slack goes to parallel fetches... (reproducing 0027's content).

Anyone reading top-to-bottom gets the shape in 2-3x less text. The reader who wants the full *why* clicks through.

### Citation style

Inline `src/path/file.rs:N` for any specific behavioral claim. Matches the `whisper-cpp-deepdive.md` convention. The discipline is: cite the file in all cases, cite the line when a specific location is in mind. Line numbers will drift over time — that's accepted.

### In-flight stamp

At the top of `state-machine.md` and `orchestration.md`, before any other content:

```markdown
> ⚠ **As of commit `<sha>`.** This subsystem is being reshaped in
> [Plan B Epic 2](../../superpowers/plans/2026-05-13-plan-b-epic-2/).
> Expect revision at epic close — names, contracts, and topology may move.
```

Removed at Epic 2 close when the subsystem stabilizes. The `<sha>` is the commit hash at the time the file was last revised.

### Diagrams

ASCII only, in fenced code blocks. Two diagrams in the doc set:

- **Topology diagram** in `orchestration.md` — the n=3 fetch + 1 transcribe + mpsc shape.
- **State-transition diagram** in `state-machine.md` — the lifecycle-state graph.

Everything else stays prose. Rationale: ASCII renders in any markdown viewer (CLI included), survives copy-paste, is grep-friendly. The `whisper-cpp-deepdive.md` precedent used no diagrams at all and was readable, so two ASCII diagrams already overprovisions.

### What stays out (explicit exclusions)

- **External-tool documentation in general.** No yt-dlp man-page material; no whisper.cpp internals (that's `whisper-cpp-deepdive.md`); no sqlite tutorial.
- **ADR rationale.** A reader who wants *why* clicks the ADR ref.
- **Test enumeration / how-to-run-tests.** Belongs in `CLAUDE.md` and the test files themselves.
- **Build instructions, CUDA flags.** Belongs in `docs/SRC-BAKE-*.md` and the README.
- **CLI flag reference.** Belongs in `--help` output and `src/cli.rs`. The architecture doc names what the CLI does, not how to invoke it.

## Lifecycle and maintenance

### Maintenance triggers

Not on every PR. The goal is *current within an epoch*, not *current within a commit*. Triggers:

1. **New ADR added.** ADR map in `index.md` gets a new row. Other files get an inline reference *if* the ADR's content is relevant to their subsystem.
2. **Subsystem code structure changes significantly.** Renaming a module, splitting a file, changing a public type's shape → relevant file's layout/key-types description updates. Not triggered by line-level changes.
3. **Integration surface changes.** yt-dlp flags change, whisper-rs version bumps, sqlite usage pattern changes → integration depth section updates in `data-input.md` or `transcription.md`.
4. **Epic close.** When Plan B Epic 2 closes, both in-flight stamps lift; `state-machine.md` and `orchestration.md` get a revision pass.

### Drift detection (lightweight, no automation)

No automated validation. Drift detection happens at planning time: when a new plan is being written, the planner checks whether the plan touches surfaces covered by the architecture doc, and if so, includes a "update `docs/reference/architecture/<file>.md` for <change>" task in the relevant phase. The Sonnet spec-compliance reviewer per ADR 0018 adds the "did this touch arch surfaces?" question to its plan review.

Optionally, a single FOLLOWUPS entry per epic — "verify architecture doc currency before epic close" — keeps the check from being forgotten.

The architecture doc itself is **not** subject to the codex-advisor / Sonnet review tier per ADR 0018 — that tier governs code review. The doc's reviewer is the human user.

### First-write order

The implementation plan should follow this order:

1. **Skeleton pass.** Create all 5 files with section headers only. Cheap; catches structural issues before words are committed.
2. **Index first** (motivation + glossary + ADR map). The glossary and ADR map become reference material the deepdives cite into, so they exist first. Donor's-journey walk-through gets stub paragraphs that link forward.
3. **Deepdives in order of stability:** `data-input.md` → `transcription.md` → `state-machine.md` → `orchestration.md`. Stable subsystems first; in-flight files last so their content reflects the most recent code state.
4. **Donor's-journey walk-through fill-in pass.** Revisit the index walk-through; replace stub paragraphs with content that genuinely threads the deepdives.
5. **Linkcheck pass.** Verify every cross-reference (ADR refs, file:line citations, inter-file links) resolves. Grep-and-eyeball is sufficient at this size.

### Documentation-hierarchy placement

```
docs/
├── reference/
│   ├── architecture/         ← this design's output
│   ├── whisper-cpp-deepdive.md       ← external-tool depth (cited from architecture/)
│   └── tiktok-for-developers/
├── decisions/                ← ADRs (cited from architecture/)
├── superpowers/
│   ├── plans/                ← active plans (cited from in-flight stamps)
│   └── specs/                ← this design lives here
├── FOLLOWUPS.md              ← one entry per epic re drift check
```

### Cross-cutting additions

- **`CLAUDE.md` pointer.** Add one line under "Default working patterns" or "Working disciplines":

  > **Onboarding / system orientation:** start at `docs/reference/architecture/index.md`.

  This makes the doc discoverable to any reader (human or AI agent) starting from `CLAUDE.md`.

- **`docs/FOLLOWUPS.md` entry.** Add one line under the active epic's section:

  > Verify `docs/reference/architecture/` currency before epic close; revise files marked with in-flight stamps.

## Out of scope (this design)

- Writing the doc set itself — that's the implementation plan, produced by `writing-plans`.
- Any change to the ADR collection. The architecture doc cites existing ADRs; it does not create new ones.
- Migrating or restructuring existing reference docs (`whisper-cpp-deepdive.md`, `tiktok-for-developers/`). They stay where they are; the architecture doc cites them.
- Documenting Epic-3-and-beyond planned work. The doc captures the *current* shape and the *in-flight* shape; it does not speculate about future epics.

## Next step

User reviews this spec → `writing-plans` skill produces the implementation plan covering the 5-step first-write order.

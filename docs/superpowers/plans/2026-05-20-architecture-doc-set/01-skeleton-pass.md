# Task 1 — Skeleton pass

**Goal:** Create the 5 architecture doc files under `docs/reference/architecture/` with section headers only (no prose content). This step is cheap and catches structural issues — does each file's section breakdown actually work? — before words are committed.

**ADRs referenced:** none (this task creates structure only).

**Files:**
- Create: `docs/reference/architecture/index.md`
- Create: `docs/reference/architecture/data-input.md`
- Create: `docs/reference/architecture/state-machine.md`
- Create: `docs/reference/architecture/transcription.md`
- Create: `docs/reference/architecture/orchestration.md`

**Pre-reqs:** branch `feat/plan-b-epic-2` checked out.

---

- [ ] **Step 1: Verify the target directory does not yet exist, then create it**

```bash
ls docs/reference/architecture 2>/dev/null && echo "EXISTS — investigate before proceeding" || mkdir docs/reference/architecture
```

Expected: directory is created. If it already exists with files inside, stop and ask the user — do not overwrite.

- [ ] **Step 2: Create `index.md` with section headers**

Write `docs/reference/architecture/index.md` with exactly this content:

```markdown
# uu-tiktok — architecture

Onboarding reference for the uu-tiktok pipeline. Start here.

## 1. What this system is and who it serves

(TBD — populated in T02)

## 2. Glossary

(TBD — populated in T02)

## 3. The donor's journey

(TBD — populated in T02 as stubs, T07 fills in real prose)

## 4. ADR map

(TBD — populated in T02)

## 5. Where to look for X

(TBD — populated in T02)

## 6. How this doc is maintained

(TBD — populated in T02)
```

The `(TBD — populated in TNN)` markers are temporary section placeholders for the skeleton pass only. They MUST be replaced in T02 — leaving any `(TBD)` in the final doc is a plan failure.

- [ ] **Step 3: Create `data-input.md` with section headers**

Write `docs/reference/architecture/data-input.md` with exactly this content:

```markdown
# uu-tiktok — data input

The data-input subsystem covers two stages of the donor's journey: ingest (parsing the TikTok DDP export into rows the state machine can claim) and fetch (downloading the watched-video MP4 and extracting audio for transcription).

## Ingest

(TBD — populated in T03)

### DDP export shape

(TBD — populated in T03)

### Parsing strategy

(TBD — populated in T03)

### What becomes a row in state

(TBD — populated in T03)

## Fetcher

(TBD — populated in T03)

### Subprocess wrapping pattern

(TBD — populated in T03)

### yt-dlp invocation: flags and rationale

(TBD — populated in T03)

### Output capture

(TBD — populated in T03)

### Timeout policy

(TBD — populated in T03)

### Retry classification

(TBD — populated in T03)

### Audio extraction handoff

(TBD — populated in T03)

## ADRs governing this subsystem

(TBD — populated in T03)
```

- [ ] **Step 4: Create `state-machine.md` with section headers and in-flight stamp**

Write `docs/reference/architecture/state-machine.md` with exactly this content:

```markdown
# uu-tiktok — state machine

> ⚠ **As of commit `<sha>`.** This subsystem is being reshaped in
> [Plan B Epic 2](../../superpowers/plans/2026-05-13-plan-b-epic-2/).
> Expect revision at epic close — names, contracts, and topology may move.

The state machine is the durable record of the pipeline's progress. It lives in a sqlite database (one row per watched-video to process) and arbitrates between concurrent orchestrator workers via row-level claim contention.

## Schema and lifecycle states

(TBD — populated in T05)

### Schema overview

(TBD — populated in T05)

### Lifecycle states

(TBD — populated in T05)

### State-transition diagram

(TBD — populated in T05 with ASCII diagram)

## Claim contention

(TBD — populated in T05)

## Stale-claim sweep

(TBD — populated in T05)

## Mutator contracts

(TBD — populated in T05)

## Schema-version policy

(TBD — populated in T05)

## Crash-recovery durability

(TBD — populated in T05)

## Failure classification

(TBD — populated in T05)

## ADRs governing this subsystem

(TBD — populated in T05)
```

The `<sha>` placeholder will be filled in T05 with the output of `git rev-parse --short HEAD` at that time.

- [ ] **Step 5: Create `transcription.md` with section headers**

Write `docs/reference/architecture/transcription.md` with exactly this content:

```markdown
# uu-tiktok — transcription

The transcription subsystem covers three stages: preparing audio in the format whisper.cpp requires, running the transcription via the embedded `whisper-rs` library, and writing the resulting artifact to disk before the state machine acknowledges success.

## Audio preparation

(TBD — populated in T04)

### PCM format invariant

(TBD — populated in T04)

### Source: the fetcher handoff

(TBD — populated in T04)

## Transcription

(TBD — populated in T04)

### whisper-rs integration

(TBD — populated in T04)

### Context and full-params configuration

(TBD — populated in T04)

### Cancellation wiring

(TBD — populated in T04)

### GPU verification

(TBD — populated in T04)

### Engine-state model

(TBD — populated in T04)

### Raw signals extraction

(TBD — populated in T04)

## Output

(TBD — populated in T04)

### Artifact shape

(TBD — populated in T04)

### Sharding

(TBD — populated in T04)

### Artifact-before-mark_succeeded

(TBD — populated in T04)

## ADRs governing this subsystem

(TBD — populated in T04)
```

- [ ] **Step 6: Create `orchestration.md` with section headers and in-flight stamp**

Write `docs/reference/architecture/orchestration.md` with exactly this content:

```markdown
# uu-tiktok — orchestration

> ⚠ **As of commit `<sha>`.** This subsystem is being reshaped in
> [Plan B Epic 2](../../superpowers/plans/2026-05-13-plan-b-epic-2/).
> Expect revision at epic close — names, contracts, and topology may move.

The orchestrator drives the pipeline at runtime: it claims work from the state machine, dispatches fetcher and transcriber workers, supervises them via tokio primitives, and coordinates shutdown.

## Topology

(TBD — populated in T06)

### Topology diagram

(TBD — populated in T06 with ASCII diagram)

## Control loop

(TBD — populated in T06)

## Supervision

(TBD — populated in T06)

## Shutdown order

(TBD — populated in T06)

## Failure handling

(TBD — populated in T06)

## Batch validation contract

(TBD — populated in T06)

## ADRs governing this subsystem

(TBD — populated in T06)
```

- [ ] **Step 7: Verify all 5 files exist**

```bash
ls docs/reference/architecture/
```

Expected output (order may vary):
```
data-input.md
index.md
orchestration.md
state-machine.md
transcription.md
```

- [ ] **Step 8: Verify the in-flight stamps are present on the two files that should have them**

```bash
grep -l "in-flight" docs/reference/architecture/*.md || grep -l "reshaped in" docs/reference/architecture/*.md
```

Expected: exactly `state-machine.md` and `orchestration.md` print. If `index.md`, `data-input.md`, or `transcription.md` match, remove the stamp from them — those files do not carry one.

- [ ] **Step 9: Commit**

```bash
git add docs/reference/architecture/
git commit -m "$(cat <<'EOF'
docs(reference): architecture skeleton — 5 files under docs/reference/architecture/

Section headers only; (TBD) markers will be populated in T02-T06.
In-flight stamp templates seeded on state-machine.md and orchestration.md
pending Plan B Epic 2 close.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: clean commit, `adg validate` passes during pre-commit (the architecture doc is not part of the ADR model so validation should be unaffected).

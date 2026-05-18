# Task 12 — Draft + decide AD0024, AD0025, AD0026, AD0027 (Phase 2 ADRs)

**Goal:** Land four Phase-2 ADRs in `decided` state via `adg` so subsequent Phase-2 tasks can reference them. **AD0024 specifically encodes the load-bearing shutdown ORDER** that codex-advisor flagged during brainstorm — that order must appear in the ADR text, not just be implicit.

**ADRs touched:** AD0024 (drafts), AD0025 (drafts), AD0026 (drafts), AD0027 (drafts).

**Files:**
- Create: `docs/decisions/AD0024-bug-class-supervision-joinset-cancellationtoken-shutdown-order-load-bearing.md`
- Create: `docs/decisions/AD0025-claim-contention-no-polling-for-plan-b-batch-drain.md`
- Create: `docs/decisions/AD0026-bounded-process-run-capture-streaming-vecdeque.md`
- Create: `docs/decisions/AD0027-orchestrator-topology-n3-fetch-1-transcribe-capacity-2.md`

**Pre-reqs:** Phase 1 complete; `PHASE-1-CLOSE.md` exists per AD0019. Fresh controller per AD0019's phase-boundary discipline.

---

- [ ] **Step 1: Draft AD0024 (Bug-class supervision; load-bearing shutdown ORDER)**

```bash
adg add --model docs/decisions \
  --id 0024 \
  --title "Bug-class supervision: JoinSet + CancellationToken; shutdown ORDER is load-bearing"
```

`scripts/adr-fill 0024` body:

```markdown
## Context and Problem Statement

Phase 2 spawns N fetch workers + 1 transcribe worker as tokio tasks. The orchestrator must (a) supervise all workers and panic-propagate; (b) implement graceful shutdown on first Bug-class error or panic; (c) avoid deadlocking on shutdown — specifically, avoid the engine worker parking on `blocking_recv` until process exit.

## Considered Options

1. `tokio::task::JoinSet` + `tokio_util::sync::CancellationToken` + `tokio::select!` at engine call
2. `Arc<Mutex<bool>>` shutdown flag polled in each loop
3. Channel-based cancellation (extra signal channel per worker)
4. `tokio::spawn` + manual `JoinHandle` tracking (no JoinSet)

## Decision Drivers

- Composes cleanly with Epic 1's `AD0012` per-request cancellation (`Arc<AtomicBool>` flipped by abort_callback)
- Cancellation latency: O(milliseconds), not O(seconds)
- No deadlock on coordinated shutdown
- Panic-propagation is automatic

## Decision Outcome

Option 1. All fetch workers + the transcribe worker spawn into a shared `tokio::task::JoinSet`. A shared `tokio_util::sync::CancellationToken` is cloned to each worker. Main loops on `join_set.join_next()`; on first `Err(Bug)` or panic, `token.cancel()` and drains remaining tasks.

The transcribe worker's hot inner call wraps `engine.transcribe()` in `tokio::select! { _ = token.cancelled() => Err(Cancelled), r = engine.transcribe(...) => r }`. When `token.cancel()` fires, the select arm wins and drops the in-flight `transcribe()` future. The `CancelOnDrop` guard inside `transcribe()` (Epic 1's AD0012) fires the `Arc<AtomicBool>` cancel flag, which whisper.cpp's `abort_callback` polls — inference aborts within milliseconds. This is the composition path from Epic 1's cancellation primitive to Phase 2's orchestrator.

**Shutdown ORDER is load-bearing.** Without this exact sequence, the engine worker parks on `blocking_recv` until process exit (codex-advisor identified this during brainstorm):

1. `token.cancel()` — signals all workers to stop.
2. Drop the fetch→transcribe `mpsc::Sender` — closes the channel so transcribe_worker's `receiver.recv()` returns `None` and the worker exits its loop.
3. `join_set.join_next()` to completion — waits for all workers to return.
4. `engine.shutdown()` — LAST. This drops the engine's request-side mpsc::Sender; the engine worker thread sees the closed channel, exits its `blocking_recv` loop, and the join completes.

Reversing 2 and 4 (calling `engine.shutdown()` before draining transcribe_worker) would cause transcribe_worker's pending `engine.transcribe()` to wedge on a dead engine. Reversing 1 and 2 (dropping the sender before cancelling) would prevent the CancellationToken from interrupting in-flight `transcribe()` calls.

Process exits 1 on Bug; 0 on clean drain.

Option 2 was rejected: polling adds latency and busywork; loops would need to poll between every `await`. Option 3 was rejected: redundant with CancellationToken. Option 4 was rejected: JoinSet's built-in panic propagation is operationally simpler than manual JoinHandle tracking.

## Consequences

- Cancellation latency is bounded by the largest single `await` in the worker loops (typically `engine.transcribe()` for the transcribe worker, ~1s on `large-v3-turbo-q5_0`; whisper.cpp's abort_callback polls every few ms during inference).
- The transcribe worker MUST wrap `engine.transcribe()` in `tokio::select! { _ = token.cancelled() => ..., r = engine.transcribe(...) => r }`; otherwise cancellation only catches at await-yield points between videos.
- The orchestrator's main is responsible for the shutdown order; the workers themselves don't sequence it.
- AD0012's `Arc<AtomicBool>` per-request cancellation stays unchanged; it's the *propagation mechanism* from CancellationToken into whisper.cpp's abort_callback.
```

Decide:

```bash
adg decide --model docs/decisions --id 0024
```

- [ ] **Step 2: Draft AD0025 (No polling for Plan B)**

```bash
adg add --model docs/decisions \
  --id 0025 \
  --title "Claim contention: no polling for Plan B — batch-drain on claim_next==None"
```

`scripts/adr-fill 0025` body:

```markdown
## Context and Problem Statement

Phase 2's fetch workers race for pending rows via `claim_next`. When `claim_next` returns `None`, the worker can either (a) exit (drain semantics) or (b) sleep-poll waiting for new work. EPIC-2-SKETCH originally proposed 100ms–2s polling backoff. Should Plan B's fetch workers poll, or exit on first `None`?

## Considered Options

1. Exit on first `None` (drain semantics)
2. Sleep-poll 100ms–2s with exponential backoff
3. Block on a condition variable signaled by `claim_next`

## Decision Drivers

- Plan B is batch-drain: ingest happens in a separate `ingest` phase before `process`. The pool is frozen at process startup.
- Polling burns CPU without producing work
- Daemon mode (continuous ingest) is Plan C territory

## Decision Outcome

Option 1: exit on `claim_next == None`. Plan B's `process` invocation is batch-drain — ingest is a separate prior phase. Once the queue is empty, there's no more work; polling would be busywork.

This is an explicit deviation from EPIC-2-SKETCH's polling proposal (the sketch was written before the Plan A → Plan B operational model was clarified).

Polling is deferred to Plan C / daemon mode where ingest is live. At that point an AD0025 amendment or successor ADR documents the polling policy.

## Consequences

- A Process invocation with 0 pending rows exits immediately (claimed=0 → process exits 3 per existing main.rs behavior).
- Mid-process behavior is unaffected: workers race for whatever pending rows exist at startup; once drained, all workers see `None` and exit; the orchestrator joins them and exits 0.
- The orchestrator does NOT need a "wait for more work" signal — `JoinSet::join_next()` returning `None` after all workers exit IS the drain signal.
```

Decide:

```bash
adg decide --model docs/decisions --id 0025
```

- [ ] **Step 3: Draft AD0026 (Bounded process::run capture)**

```bash
adg add --model docs/decisions \
  --id 0026 \
  --title "Bounded process::run capture: streaming VecDeque<u8> of size stderr_capture_bytes"
```

`scripts/adr-fill 0026` body:

```markdown
## Context and Problem Statement

`src/process.rs` currently reads each subprocess's stdout and stderr via `read_to_end`, buffering the full streams in memory before truncating to `stderr_capture_bytes` via `ring_buffer_tail`. Per FOLLOWUPS T6, this is unbounded: a misbehaving subprocess that writes 100 MB of stderr buffers 100 MB before truncation. With N=3 concurrent fetch workers, peak memory is 3× whatever the worst stderr-flood looks like. `ring_buffer_tail` is also a misnomer (it's a slice operation, not a ring buffer).

## Considered Options

1. Streaming reader: maintain a `VecDeque<u8>` of size `stderr_capture_bytes`; trim oldest bytes as new bytes arrive
2. Pipe stderr to a temp file, read tail of file at completion
3. Add `--max-stderr-bytes` flag, kill the subprocess if exceeded
4. Accept unbounded buffering with a hard 10 MB cap (panic above)

## Decision Drivers

- Cap peak memory to a known constant
- Don't lose the last N bytes of stderr (operationally important for diagnosing failure)
- Don't introduce filesystem dependency for transient subprocess capture
- Don't break existing tests

## Decision Outcome

Option 1: streaming reader that drops bytes from the FRONT as new bytes arrive past the cap. Symmetric for stdout: add `stdout_capture_bytes` to `CommandSpec`. (yt-dlp writes audio to a file so stdout is small, but a misbehaving tool could flood — defense in depth.)

Rename `ring_buffer_tail` → `tail_excerpt`. The old name implied ring-buffer semantics (constant-time circular append) that the function didn't have (it's a slice operation on a complete buffer). The new name describes what it returns.

Option 2 was rejected: filesystem indirection for transient capture is operational complexity for no benefit.

Option 3 was rejected: killing the subprocess on stderr flood loses the very signal we want (diagnostic stderr).

Option 4 was rejected: unbounded buffering remains unbounded; a 10 MB cap doesn't help if the subprocess is well-behaved enough to not flood.

## Consequences

- Peak memory per subprocess is `stdout_capture_bytes + stderr_capture_bytes` (typically 1–8 KB each).
- The full stdout is no longer available to callers; `CommandOutcome::stdout` becomes `stdout_excerpt: String` symmetric with `stderr_excerpt`. Audit existing callers (Plan A: yt-dlp uses file output, transcribe uses stderr only; no caller reads stdout content). Plan A's `#[allow(dead_code)]` on `stdout: Vec<u8>` confirms there's no current bin caller.
- Tests pass against synthetic stderr-flood (write 10 MB to stderr; assert peak RSS bounded; assert excerpt is last `stderr_capture_bytes` bytes).
- The "efficiency-tweaks" worktree (separate from Epic 2) may land the bounded buffer ahead of Phase 2. If so, T14's scope reduces to rename + symmetric stdout cap + verifying the existing test passes after rename. As of plan-write time the worktree has only doc commits — T14 keeps full scope.
```

Decide:

```bash
adg decide --model docs/decisions --id 0026
```

- [ ] **Step 4: Draft AD0027 (Orchestrator topology defaults)**

```bash
adg add --model docs/decisions \
  --id 0027 \
  --title "Orchestrator topology: N=3 fetch + 1 transcribe; mpsc payload (Claim, Vec<f32>, PathBuf); capacity 2"
```

`scripts/adr-fill 0027` body:

```markdown
## Context and Problem Statement

Phase 2's orchestrator design must pick a worker topology and channel-payload shape. Three coupled questions: (a) how many fetch workers, (b) what crosses the fetch→transcribe channel, (c) channel capacity. The choices have empirical ground truth from Epic 1's bake numbers.

## Considered Options

1. N=3 fetch + 1 transcribe; payload `(Claim, Vec<f32>, PathBuf)`; capacity 2
2. N=2 fetch + 1 transcribe; payload `(Claim, PathBuf)`; capacity 4
3. N=6 fetch + 1 transcribe; payload `(Claim, PathBuf)`; capacity 1
4. N=1 fetch + 1 transcribe (degenerate; equivalent to serial loop)

## Decision Drivers

- Steady-state throughput = `min(N / avg_fetch, 1 / avg_transcribe)` — curve-flattening point
- Outlier handling (one stuck fetch shouldn't halve effective capacity)
- CPU envelope (each fetch spawns yt-dlp + ffmpeg postprocessor; ~6 subprocesses concurrent at N=3)
- Memory envelope (Vec<f32> is ~3 MB per clip; multiply by in-flight count)

## Decision Outcome

Option 1. **N=3 fetch + 1 transcribe**, channel payload `(Claim, Vec<f32>, PathBuf)`, capacity 2.

**N=3 anchoring** (from Epic 1 bake on `news_orgs` fixture, n=8):
- avg_fetch=5.5s, avg_transcribe=1s
- N=1: 0.18 v/s fetch, fetch-bound, ~6.5s/video
- N=2: 0.36 v/s fetch, fetch-bound, ~2.75s/video
- **N=3: 0.55 v/s fetch, fetch-bound, ~1.83s/video** (curve flattens here)
- N=4: 0.73 v/s fetch, fetch-bound, ~1.38s/video (marginal returns)
- N=6+: transcribe-bound at ~1 v/s

N=3 is the curve-flattening point: ~3.5× speedup vs serial. Outlier handling: with N=2 one stuck fetch halves capacity; with N=3 it drops by only a third. CPU envelope: N=3 spawns ~6 concurrent subprocesses (yt-dlp + ffmpeg per fetch), comfortable on 4–8 cores.

**Payload `(Claim, Vec<f32>, PathBuf)`**: fetch workers do WAV decode in parallel (~50–100ms per clip via `hound`), keeping the transcribe path lean — transcribe worker receives ready-to-feed samples and just calls `engine.transcribe()`. `PathBuf` rides through for cleanup after `mark_succeeded` per AD0008.

**Capacity 2**: buffers transcribe's small variance (transcribe is stable at ~1s on `large-v3-turbo-q5_0`; never accumulates). Peak memory at N=3 + capacity 2 + 1 in-flight + 3 active fetches = ~6 items in flight × ~3 MB ≈ 18 MB. Negligible against A10's 24 GB.

**All defaults flag-tunable** via `--download-workers` (default 3) and `--channel-capacity` (default 2).

Options 2, 3, 4 rejected for the throughput math reasons above (N=2 leaves capacity on the table; N=6 is transcribe-bound so the extra fetch workers idle; N=1 is the serial loop).

## Consequences

- Phase 2's CLI gains two flags; defaults match this ADR.
- The transcribe worker is single-instance (AD0015 stays; whisper.cpp's `whisper_full_with_state` is not parallel-safe across instances of the same engine).
- Multi-state intra-GPU parallelism (whisper_full_parallel) stays deferred to Plan C; the orchestrator's public shape (1 transcribe worker) doesn't change when Plan C swaps in a `WhisperPool` of N engines.
- Channel-payload size (~3 MB per item) is the right unit; if Epic 4+ moves WAV decode into the engine, payload becomes `(Claim, PathBuf)` and the channel sizing reduces.
```

Decide:

```bash
adg decide --model docs/decisions --id 0027
```

- [ ] **Step 5: Validate the ADR set**

```bash
adg validate --model docs/decisions
```

Expected: clean. All four ADRs `status: decided`.

- [ ] **Step 6: Commit**

```bash
git add docs/decisions/AD0024-*.md docs/decisions/AD0025-*.md \
        docs/decisions/AD0026-*.md docs/decisions/AD0027-*.md
git commit -m "$(cat <<'EOF'
docs(decisions): land AD0024–AD0027 (Phase 2 ADRs)

AD0024 — Bug-class supervision: tokio::JoinSet + tokio_util CancellationToken
+ tokio::select! at engine call. **Shutdown ORDER is load-bearing:**
token.cancel() → drop fetch→transcribe sender → join workers →
engine.shutdown() LAST. Without this order the engine worker parks on
blocking_recv until process exit (codex-advisor identified during
brainstorm; the ADR text now encodes the order to prevent re-litigation).

AD0025 — No polling for Plan B: fetch workers exit on claim_next == None.
Plan B is batch-drain (ingest is a separate prior phase). Polling is
deferred to Plan C / daemon mode. Explicit deviation from
EPIC-2-SKETCH's polling proposal — captured for clarity.

AD0026 — Bounded process::run capture: streaming VecDeque<u8> of size
stderr_capture_bytes; symmetric stdout_capture_bytes for defense in
depth. Rename ring_buffer_tail → tail_excerpt (FOLLOWUPS T6 — the old
name implied ring-buffer semantics the function didn't have).

AD0027 — Orchestrator topology defaults: N=3 fetch + 1 transcribe;
mpsc payload (Claim, Vec<f32>, PathBuf); capacity 2. Empirically derived
from Epic 1 bake's throughput math. Flag-tunable via --download-workers
and --channel-capacity.

Refs: AD0012 (composes with cancellation), AD0015 (single transcribe
worker stays), AD0016 (engine API stable)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] All four ADRs `status: decided`
- [ ] AD0024 text explicitly lists the four-step shutdown order
- [ ] AD0025 calls out the EPIC-2-SKETCH deviation
- [ ] AD0026 records the rename and the worktree-coordination note
- [ ] AD0027 numbers in the throughput-math table match the spec/bake
- [ ] `adg validate --model docs/decisions` clean

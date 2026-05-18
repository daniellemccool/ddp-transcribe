# Task 12 — Draft + decide 0025, 0026, 0027 (Phase 2 ADRs)

**Goal:** Land three Phase-2 ADRs in `accepted` state via `scripts/adr` so subsequent Phase-2 tasks can reference them. **0025 specifically encodes the load-bearing shutdown ORDER** that codex-advisor flagged during brainstorm — that order must appear in the ADR text (lives in `## Consequences` so it survives `adr decide`'s placeholder-overwrite of `## Decision Outcome`).

**Note on absent fourth ADR:** an earlier draft of Phase 2 included a bounded-`process::run`-capture ADR in this slot. Perf-tweaks shipped that decision as `0021` (bounded subprocess output capture via streaming `VecDeque<u8>`) before Epic 2 execution started, so Epic 2 inherits 0021 rather than authoring its own. T14's scope reduces accordingly (see `14-bounded-process-capture.md`); Phase 2 drafts three ADRs, not four.

**ADRs touched:** 0025 (drafts), 0026 (drafts), 0027 (drafts).

**Files:**
- Create: `docs/decisions/0025-bug-class-supervision-joinset-cancellationtoken-shutdown-order-load-bearing.md`
- Create: `docs/decisions/0026-claim-contention-no-polling-for-plan-b-batch-drain.md`
- Create: `docs/decisions/0027-orchestrator-topology-n3-fetch-1-transcribe-capacity-2.md`

**Pre-reqs:** Phase 1 complete; `PHASE-1-CLOSE.md` exists per 0019. Fresh controller per 0019's phase-boundary discipline.

---

- [ ] **Step 1: Draft 0025 (Bug-class supervision; load-bearing shutdown ORDER)**

```bash
ID25=$(scripts/adr new "Bug-class supervision: JoinSet + CancellationToken; shutdown ORDER is load-bearing")
echo "Assigned: $ID25"   # expected: 0025 (next available after T1's 0022/0023/0024)
```

If `$ID25` is anything other than `0025`, stop and investigate. Then load the body:

```bash
scripts/adr edit "$ID25" <<'BODY'
# Bug-class supervision: JoinSet + CancellationToken; shutdown ORDER is load-bearing

## Context and Problem Statement

Phase 2 spawns N fetch workers + 1 transcribe worker as tokio tasks. The orchestrator must (a) supervise all workers and panic-propagate; (b) implement graceful shutdown on first Bug-class error or panic; (c) avoid deadlocking on shutdown — specifically, avoid the engine worker parking on `blocking_recv` until process exit.

## Considered Options

* `tokio::task::JoinSet` + `tokio_util::sync::CancellationToken` + `tokio::select!` at engine call
* `Arc<Mutex<bool>>` shutdown flag polled in each loop
* Channel-based cancellation (extra signal channel per worker)
* `tokio::spawn` + manual `JoinHandle` tracking (no JoinSet)

## Decision Drivers

- Composes cleanly with Epic 1's 0012 per-request cancellation (`Arc<AtomicBool>` flipped by abort_callback)
- Cancellation latency: O(milliseconds), not O(seconds)
- No deadlock on coordinated shutdown
- Panic-propagation is automatic

## Decision Outcome

placeholder — `adr decide` fills this in

## Consequences

- All fetch workers + the transcribe worker spawn into a shared `tokio::task::JoinSet`. A shared `tokio_util::sync::CancellationToken` is cloned to each worker. Main loops on `join_set.join_next()`; on first `Err(Bug)` or panic, `token.cancel()` and drains remaining tasks.
- The transcribe worker's hot inner call wraps `engine.transcribe()` in `tokio::select! { _ = token.cancelled() => Err(Cancelled), r = engine.transcribe(...) => r }`. When `token.cancel()` fires, the select arm wins and drops the in-flight `transcribe()` future. The `CancelOnDrop` guard inside `transcribe()` (Epic 1's 0012) fires the `Arc<AtomicBool>` cancel flag, which whisper.cpp's `abort_callback` polls — inference aborts within milliseconds. This is the composition path from Epic 1's cancellation primitive to Phase 2's orchestrator.
- **Shutdown ORDER is load-bearing.** Without this exact sequence the engine worker parks on `blocking_recv` until process exit (codex-advisor identified this during brainstorm):
  1. `token.cancel()` — signals all workers to stop.
  2. Drop the fetch→transcribe `mpsc::Sender` — closes the channel so `transcribe_worker`'s `receiver.recv()` returns `None` and the worker exits its loop.
  3. `join_set.join_next()` to completion — waits for all workers to return.
  4. `engine.shutdown()` — LAST. Drops the engine's request-side `mpsc::Sender`; the engine worker thread sees the closed channel, exits its `blocking_recv` loop, and the join completes.

  Reversing 2 and 4 (calling `engine.shutdown()` before draining `transcribe_worker`) would cause `transcribe_worker`'s pending `engine.transcribe()` to wedge on a dead engine. Reversing 1 and 2 (dropping the sender before cancelling) would prevent the CancellationToken from interrupting in-flight `transcribe()` calls.
- Process exits 1 on Bug; 0 on clean drain.
- Cancellation latency is bounded by the largest single `await` in the worker loops (typically `engine.transcribe()` for the transcribe worker, ~1s on `large-v3-turbo-q5_0`; whisper.cpp's `abort_callback` polls every few ms during inference).
- The transcribe worker MUST wrap `engine.transcribe()` in `tokio::select! { _ = token.cancelled() => ..., r = engine.transcribe(...) => r }`; otherwise cancellation only catches at await-yield points between videos.
- The orchestrator's main is responsible for the shutdown order; the workers themselves don't sequence it.
- 0012's `Arc<AtomicBool>` per-request cancellation stays unchanged; it's the *propagation mechanism* from CancellationToken into whisper.cpp's `abort_callback`.
BODY
```

Decide (option text matches the first bullet verbatim):

```bash
scripts/adr decide "$ID25" '`tokio::task::JoinSet` + `tokio_util::sync::CancellationToken` + `tokio::select!` at engine call' 'Polling Arc<Mutex<bool>> adds latency and busywork; channel-based cancellation is redundant with CancellationToken; manual JoinHandle tracking loses JoinSet panic-propagation.'
```

- [ ] **Step 2: Draft 0026 (No polling for Plan B)**

```bash
ID26=$(scripts/adr new "Claim contention: no polling for Plan B — batch-drain on claim_next==None")
echo "Assigned: $ID26"   # expected: 0026
```

```bash
scripts/adr edit "$ID26" <<'BODY'
# Claim contention: no polling for Plan B — batch-drain on claim_next==None

## Context and Problem Statement

Phase 2's fetch workers race for pending rows via `claim_next`. When `claim_next` returns `None`, the worker can either (a) exit (drain semantics) or (b) sleep-poll waiting for new work. EPIC-2-SKETCH originally proposed 100ms–2s polling backoff. Should Plan B's fetch workers poll, or exit on first `None`?

## Considered Options

* Exit on first `None` (drain semantics)
* Sleep-poll 100ms–2s with exponential backoff
* Block on a condition variable signaled by `claim_next`

## Decision Drivers

- Plan B is batch-drain: ingest happens in a separate `ingest` phase before `process`. The pool is frozen at process startup.
- Polling burns CPU without producing work
- Daemon mode (continuous ingest) is Plan C territory

## Decision Outcome

placeholder — `adr decide` fills this in

## Consequences

- A `process` invocation with 0 pending rows exits immediately (claimed=0 → process exits 3 per existing main.rs behavior).
- Mid-process behavior is unaffected: workers race for whatever pending rows exist at startup; once drained, all workers see `None` and exit; the orchestrator joins them and exits 0.
- The orchestrator does NOT need a "wait for more work" signal — `JoinSet::join_next()` returning `None` after all workers exit IS the drain signal.
- This is an explicit deviation from EPIC-2-SKETCH's polling proposal (the sketch was written before the Plan A → Plan B operational model was clarified).
- Polling is deferred to Plan C / daemon mode where ingest is live. At that point a 0026 amendment or successor ADR documents the polling policy.
BODY
```

Decide:

```bash
scripts/adr decide "$ID26" 'Exit on first `None` (drain semantics)' 'Plan B `process` is batch-drain: ingest is a separate prior phase. Polling burns CPU without producing work; condvars add machinery that Plan C will redesign anyway. Daemon mode is Plan C territory.'
```

- [ ] **Step 3: Draft 0027 (Orchestrator topology defaults)**

```bash
ID27=$(scripts/adr new "Orchestrator topology: N=3 fetch + 1 transcribe; mpsc payload (Claim, Vec<f32>, PathBuf); capacity 2")
echo "Assigned: $ID27"   # expected: 0027
```

```bash
scripts/adr edit "$ID27" <<'BODY'
# Orchestrator topology: N=3 fetch + 1 transcribe; mpsc payload (Claim, Vec<f32>, PathBuf); capacity 2

## Context and Problem Statement

Phase 2's orchestrator design must pick a worker topology and channel-payload shape. Three coupled questions: (a) how many fetch workers, (b) what crosses the fetch→transcribe channel, (c) channel capacity. The choices have empirical ground truth from Epic 1's bake numbers.

## Considered Options

* N=3 fetch + 1 transcribe; payload `(Claim, Vec<f32>, PathBuf)`; capacity 2
* N=2 fetch + 1 transcribe; payload `(Claim, PathBuf)`; capacity 4
* N=6 fetch + 1 transcribe; payload `(Claim, PathBuf)`; capacity 1
* N=1 fetch + 1 transcribe (degenerate; equivalent to serial loop)

## Decision Drivers

- Steady-state throughput = `min(N / avg_fetch, 1 / avg_transcribe)` — curve-flattening point
- Outlier handling (one stuck fetch shouldn't halve effective capacity)
- CPU envelope (each fetch spawns yt-dlp + ffmpeg postprocessor; ~6 subprocesses concurrent at N=3)
- Memory envelope (Vec<f32> is ~3 MB per clip; multiply by in-flight count)

## Decision Outcome

placeholder — `adr decide` fills this in

## Consequences

- **N=3 anchoring** (from Epic 1 bake on `news_orgs` fixture, n=8):
  - avg_fetch=5.5s, avg_transcribe=1s
  - N=1: 0.18 v/s fetch, fetch-bound, ~6.5s/video
  - N=2: 0.36 v/s fetch, fetch-bound, ~2.75s/video
  - **N=3: 0.55 v/s fetch, fetch-bound, ~1.83s/video** (curve flattens here)
  - N=4: 0.73 v/s fetch, fetch-bound, ~1.38s/video (marginal returns)
  - N=6+: transcribe-bound at ~1 v/s
- N=3 is the curve-flattening point: ~3.5× speedup vs serial. Outlier handling: with N=2 one stuck fetch halves capacity; with N=3 it drops by only a third. CPU envelope: N=3 spawns ~6 concurrent subprocesses (yt-dlp + ffmpeg per fetch), comfortable on 4–8 cores.
- **Payload `(Claim, Vec<f32>, PathBuf)`**: fetch workers do WAV decode in parallel (~50–100ms per clip via `hound`), keeping the transcribe path lean — transcribe worker receives ready-to-feed samples and just calls `engine.transcribe()`. `PathBuf` rides through for cleanup after `mark_succeeded` per 0008.
- **Capacity 2**: buffers transcribe's small variance (transcribe is stable at ~1s on `large-v3-turbo-q5_0`; never accumulates). Peak memory at N=3 + capacity 2 + 1 in-flight + 3 active fetches = ~6 items in flight × ~3 MB ≈ 18 MB. Negligible against A10's 24 GB.
- All defaults flag-tunable via `--download-workers` (default 3) and `--channel-capacity` (default 2).
- Phase 2's CLI gains two flags; defaults match this ADR.
- The transcribe worker is single-instance (0015 stays; whisper.cpp's `whisper_full_with_state` is not parallel-safe across instances of the same engine).
- Multi-state intra-GPU parallelism (`whisper_full_parallel`) stays deferred to Plan C; the orchestrator's public shape (1 transcribe worker) doesn't change when Plan C swaps in a `WhisperPool` of N engines.
- Channel-payload size (~3 MB per item) is the right unit; if Epic 4+ moves WAV decode into the engine, payload becomes `(Claim, PathBuf)` and the channel sizing reduces.
BODY
```

Decide:

```bash
scripts/adr decide "$ID27" 'N=3 fetch + 1 transcribe; payload `(Claim, Vec<f32>, PathBuf)`; capacity 2' 'N=2 leaves capacity on the table; N=6 is transcribe-bound so extra fetch workers idle; N=1 is the serial loop. N=3 is the empirical curve-flattening point per news_orgs bake (n=8).'
```

- [ ] **Step 4: Validate the ADR set**

```bash
scripts/adr validate
```

Expected: clean. All three ADRs `status: accepted`.

- [ ] **Step 5: Commit**

```bash
git add docs/decisions/0025-*.md docs/decisions/0026-*.md docs/decisions/0027-*.md
git commit -m "$(cat <<'EOF'
docs(decisions): land 0025–0027 (Phase 2 ADRs)

0025 — Bug-class supervision: tokio::JoinSet + tokio_util CancellationToken
+ tokio::select! at engine call. **Shutdown ORDER is load-bearing:**
token.cancel() → drop fetch→transcribe sender → join workers →
engine.shutdown() LAST. Without this order the engine worker parks on
blocking_recv until process exit (codex-advisor identified during
brainstorm; the ADR text encodes the order in Consequences so it
survives adr decide's placeholder overwrite of Decision Outcome).

0026 — No polling for Plan B: fetch workers exit on claim_next == None.
Plan B is batch-drain (ingest is a separate prior phase). Polling is
deferred to Plan C / daemon mode. Explicit deviation from
EPIC-2-SKETCH's polling proposal — captured for clarity.

0027 — Orchestrator topology defaults: N=3 fetch + 1 transcribe;
mpsc payload (Claim, Vec<f32>, PathBuf); capacity 2. Empirically derived
from Epic 1 bake's throughput math. Flag-tunable via --download-workers
and --channel-capacity.

Refs: 0012 (composes with cancellation), 0015 (single transcribe
worker stays), 0016 (engine API stable)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] All three ADRs `status: accepted`
- [ ] 0025's `## Consequences` explicitly lists the four-step shutdown order (it MUST live here, not in Decision Outcome — `adr decide` overwrites Decision Outcome with a one-liner)
- [ ] 0026 calls out the EPIC-2-SKETCH deviation
- [ ] 0027 numbers in the throughput-math table match the spec/bake
- [ ] `scripts/adr validate` clean

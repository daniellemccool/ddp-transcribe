# Task 6 — Populate `orchestration.md`

**Goal:** Replace every `(TBD)` in `docs/reference/architecture/orchestration.md` with real content covering topology, control loop, supervision, shutdown order, failure handling, and the batch-validation contract. Includes one ASCII topology diagram. Also: replace the `<sha>` placeholder in the in-flight stamp with the current short commit hash.

**ADRs referenced:** 0017, 0021 (cross-cuts data-input), 0025, 0027.

**Files:**
- Modify: `docs/reference/architecture/orchestration.md`
- Read: `src/pipeline.rs`, `src/process.rs`, any orchestration helpers under `src/state/` (claim flow), `Cargo.toml` (verify `tokio-util` is present for `CancellationToken`)

**Pre-reqs:** T01, T02 complete. Ideally T05 (state-machine.md) complete so cross-links from orchestration's claim flow into the state machine deepdive are concrete.

---

- [ ] **Step 1: Survey source files**

```bash
wc -l src/pipeline.rs src/process.rs
grep -n "^fn\|^pub fn\|^impl\|JoinSet\|CancellationToken\|mpsc\|spawn" src/pipeline.rs
grep -A2 "tokio-util" Cargo.toml
```

Note for later:
- The orchestrator entry points (likely `run_serial` and `run_pipelined`).
- The `JoinSet` and `CancellationToken` usage pattern.
- The mpsc channel construction (look for `mpsc::channel(2)` or similar) — verify capacity is 2 per ADR 0027.
- The fetch-worker and transcribe-worker function signatures.
- The shutdown sequence in `run_pipelined`.

Read further to ground claims:
- The control-loop shape — how `claim_next` is called from which task, how claims flow to fetch workers, how completed payloads flow to the transcribe worker.
- How errors propagate out of workers to the orchestrator.

- [ ] **Step 2: Replace the in-flight stamp's `<sha>` placeholder**

```bash
git rev-parse --short HEAD
```

Edit the in-flight stamp at the top of `orchestration.md` — replace `<sha>` with that short hash, matching the format used in `state-machine.md`.

- [ ] **Step 3: Write `## Topology` intro**

Replace the `## Topology` `(TBD)` with this content (verify the topology specifics against the code; if the orchestrator currently uses a different N or capacity, use the actual values and note the discrepancy with the spec):

```markdown
## Topology

The orchestrator runs an n=3-fetch + 1-transcribe topology over a bounded mpsc channel of capacity 2 (per [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md)). The choice is shaped by GPU saturation: at most one transcribe task can usefully run on the single A10 dev GPU at a time, but fetch is network-bound and benefits from concurrency. The channel capacity bounds backpressure — when the transcribe task is busy, fetch workers block on send rather than buffering work indefinitely.

A separate `run_serial` topology exists as a debugging / single-threaded baseline (no orchestrator, single worker, no mpsc); the docs below describe `run_pipelined` unless stated otherwise.
```

- [ ] **Step 4: Write `### Topology diagram`**

Add an ASCII topology diagram. Template (verify against code; rename arrows/boxes if the actual structure differs):

```markdown
### Topology diagram

```
                  +---------+
                  |  State  |
                  | machine |
                  +----+----+
                       ^
                       | claim_next  /  mark_*
                       |
                  +----+-----------------------------------+
                  |       Orchestrator (run_pipelined)     |
                  |                                        |
                  |   JoinSet<()>  +  CancellationToken    |
                  |                                        |
                  |  spawns 4 workers:                     |
                  +-+--------+--------+---------+----------+
                    |        |        |         |
                    v        v        v         v
                +------+ +------+ +------+  +---------------+
                | Fetch| | Fetch| | Fetch|  |  Transcribe   |
                |  #1  | |  #2  | |  #3  |  |   (single)    |
                +--+---+ +--+---+ +--+---+  +-------+-------+
                   |        |        |              ^
                   |        |        |              |
                   +--------+--------+              |
                            |                       |
                            v                       |
              mpsc::Sender<(Claim, Vec<f32>,        |
                            PathBuf)>  (cap = 2)----+
                            |
                  fetch workers send the
                  PCM-prepped audio + claim
                  to the transcribe worker
```
```

If the actual payload type is something other than `(Claim, Vec<f32>, PathBuf)`, use the real type — cite `src/pipeline.rs:N`.

- [ ] **Step 5: Write `## Control loop`**

Describe the control-loop shape:
- The orchestrator's "main task" pulls claims via `claim_next` and dispatches them to fetch workers (or fetch workers each pull claims themselves — verify which pattern is used).
- Each fetch worker, upon completing a download + audio extraction, sends the payload over mpsc.
- The transcribe worker reads from the mpsc receiver, transcribes, writes the artifact, and calls `mark_succeeded` (per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md)).
- The loop continues until `claim_next` returns `None` and the mpsc is drained.

Cite the specific code paths. If the actual structure differs (e.g., the dispatcher is itself the fetch workers competing on `claim_next`), describe what the code actually does.

- [ ] **Step 6: Write `## Supervision`**

Per [ADR 0025](../../decisions/0025-bug-class-supervision-joinset-cancellationtoken-shutdown-order-is-load-bearing.md), the orchestrator supervises workers with a `tokio::task::JoinSet` plus a shared `tokio_util::sync::CancellationToken`. Cover:
- The `JoinSet` holds the join handles of all spawned workers.
- The `CancellationToken` (typically `tokio_util::sync::CancellationToken`) is cloned and passed into each worker; workers observe it via `tokio::select!` or `cancelled()` checks.
- An error from any worker (or a Ctrl-C from the user) triggers `token.cancel()`, which cascades to all workers.
- The orchestrator awaits the `JoinSet` to drain after cancellation, so worker errors and panics are observable.

Redirect the *why* (and especially the *order* — see next section) to ADR 0025.

- [ ] **Step 7: Write `## Shutdown order`**

This is the load-bearing section per ADR 0025. The order is:

1. `token.cancel()` — signals all workers to wind down.
2. Drop the fetch-worker → transcribe-worker mpsc sender (or close it explicitly) — this unblocks the transcribe worker's receiver from waiting on the next message and lets it exit its loop.
3. Await the `JoinSet` — drain all worker join-handles. This is where worker errors surface.
4. `engine.shutdown()` (or equivalent) on the whisper context — *last*, because the transcribe worker may still be holding the whisper state mid-inference when cancellation hits, and the shutdown must follow the worker's exit.

Cite `src/pipeline.rs:N` for each step. Redirect to ADR 0025 for the rationale; note explicitly that the order is *load-bearing* — reversing or merging steps risks deadlock or use-after-free.

- [ ] **Step 8: Write `## Failure handling`**

Describe how the orchestrator turns worker failures into state-machine mutations:
- A retryable failure from a worker calls `Store::mark_retryable_failure(video_id, kind, message)`.
- A terminal failure calls `Store::mark_terminal_failure(video_id, reason, message)`.
- As of Plan B Epic 2 in-flight, the classifier is string-kind only (e.g., `"FetchOrTranscribe"`); rich classification with discrete failure kinds lands in Epic 3 — note this honestly.
- A panic in a worker surfaces via `JoinSet` join-error; the orchestrator logs and cancels the token (treating it as a fatal error rather than a per-row retryable failure).
- Subprocess output (yt-dlp's stdout/stderr) is bounded per [ADR 0021](../../decisions/0021-bounded-subprocess-output-capture-via-streaming-vecdeque-u8.md); the orchestrator does not need separate handling.

- [ ] **Step 9: Write `## Batch validation contract`**

Per [ADR 0017](../../decisions/0017-operational-done-contract-for-batch-validation.md), the orchestrator's "done" predicate for a batch is: every row in scope is in a terminal state (`succeeded`, `terminal_failure`) OR the stale-claim threshold has elapsed for every non-terminal row OR an operator cancellation was issued. Describe how the predicate is computed and where in `src/pipeline.rs` it's checked. If the contract is currently implemented in a different shape, describe what's actually there and link to 0017 for the target.

- [ ] **Step 10: Write ADRs section**

```markdown
## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0017 | Operational done contract | Batch validation predicate. |
| 0021 | Bounded subprocess output capture | Inherited from fetch workers (covered in `data-input.md`). |
| 0025 | JoinSet + CancellationToken shutdown order is load-bearing | Supervision and shutdown sequence. |
| 0027 | Orchestrator topology n=3 + 1, mpsc cap 2 | Topology and channel shape. |
```

- [ ] **Step 11: Verify, lint, commit**

```bash
grep -n "(TBD\|<sha>" docs/reference/architecture/orchestration.md
wc -l docs/reference/architecture/orchestration.md
```

Expected: no `(TBD)` and no `<sha>` remain; line count 160-230 (spec budget ~190).

```bash
grep -oP '\(\.\./\.\./decisions/\K[^)]+' docs/reference/architecture/orchestration.md | sort -u | while read f; do
  test -f "docs/decisions/$f" && echo "OK ADR: $f" || echo "MISSING ADR: $f"
done
```

Expected: every line `OK`.

```bash
git add docs/reference/architecture/orchestration.md
git commit -m "$(cat <<'EOF'
docs(reference): populate architecture/orchestration.md

Topology per 0027 (n=3 fetch + 1 transcribe, mpsc cap 2) + ASCII
diagram, control loop, JoinSet+CancellationToken supervision per 0025
with load-bearing shutdown order, failure handling, batch validation
contract per 0017.

In-flight stamp pinned to current commit; will revise at Epic 2 close.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

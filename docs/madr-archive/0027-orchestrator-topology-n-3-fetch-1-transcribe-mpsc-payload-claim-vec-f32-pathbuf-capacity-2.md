---
status: accepted
date: "2026-05-20"
comments:
    - author: Danielle McCool
      date: "2026-05-20 13:55:43"
      text: marked decision as decided
---

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

Chosen option: "N=3 fetch + 1 transcribe; payload `(Claim, Vec<f32>, PathBuf)`; capacity 2", because N=2 leaves capacity on the table; N=6 is transcribe-bound so extra fetch workers idle; N=1 is the serial loop. N=3 is the empirical curve-flattening point per news_orgs bake (n=8)..

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

## Comments

* **2026-05-20 13:55:43 — @Danielle McCool:** marked decision as decided

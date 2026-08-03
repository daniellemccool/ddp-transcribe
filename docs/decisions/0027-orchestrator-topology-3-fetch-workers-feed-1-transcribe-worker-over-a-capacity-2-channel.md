---
status: accepted
date: "2026-05-20"
category: Orchestration
applies_to:
    - src/pipeline/pipelined.rs
    - src/config.rs
priority: default
---

# Run 3 fetchers into 1 transcriber

## Decision

The pipelined orchestrator runs N=3 fetch workers and exactly one transcribe
worker over a bounded mpsc channel of capacity 2; fetch workers decode the
WAV and send ready-to-feed samples (the `FetchedItem`: claim, `Vec<f32>`
samples, WAV path) so the transcribe path stays lean. Defaults live in
`src/config.rs`, flag-tunable via `--download-workers` / `--channel-capacity`.

## Guidance

- Keep WAV decode in the **fetch** worker — the transcribe worker's loop is the GPU bottleneck and must only call `engine.transcribe()`; review rejects work migrating onto the transcribe side.
- The transcribe worker stays single-instance: one engine, states not parallel-safe across a shared engine. Scaling transcription is an engine-internals change (multi-state/pool behind the stable engine API), not a second worker.
- The bounded capacity is the backpressure mechanism — fetch workers block on `send` when transcribe is busy; don't grow it to "fix" a stall (peak channel memory is items × ~3 MB of samples).
- Retune N against measurement, not intuition: 3 is the empirical curve-flattening point (avg_fetch 5.5s / avg_transcribe 1s ⇒ fetch-bound through N=4, transcribe-bound by N=6), and each fetch costs ~2 subprocesses of CPU envelope.

## Why

Steady-state throughput is `min(N / avg_fetch, 1 / avg_transcribe)`; the bake
showed N=3 captures ~3.5× over serial while N=6+ just idles fetch workers
behind the GPU. N=3 also degrades gracefully — one stuck fetch drops
capacity by a third, not half.

## Context

Anchoring numbers from the Epic 1 bake (`news_orgs` fixture, n=8, model
`large-v3-turbo-q5_0`): N=1 ≈ 6.5 s/video; N=2 ≈ 2.75; N=3 ≈ 1.83; N=4 ≈
1.38 (marginal); N=6+ transcribe-bound at ~1 v/s. The WAV path rides the
payload for post-success cleanup. If WAV decode ever moves into the engine,
the payload shrinks to (claim, path) and channel sizing should be revisited.

## Alternatives

- **N=2 / capacity 4** — leaves measured throughput on the table.
- **N=6 / capacity 1** — transcribe-bound; extra workers idle, subprocess envelope doubles.
- **Payload as path only (decode in transcribe worker)** — serializes ~50–100ms of decode behind the GPU bottleneck.

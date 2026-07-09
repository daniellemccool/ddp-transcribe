---
status: accepted
date: "2026-05-12"
category: Whisper engine
applies_to:
    - src/transcribe.rs
priority: default
---

# Engine API stays stable across single- and multi-state internals

## Decision

`engine.transcribe(samples, cfg)` returns one result per call and its public
shape is independent of internal parallelism: today's internals are one
(context, state, worker thread); a production upgrade to multi-state or a
pool changes internals only, never the callers.

## Guidance

- New engine capability lands behind the existing call shape; review rejects caller-visible changes that leak internal parallelism (batch handles, state indices, per-state config at the call site).
- Only owned data crosses the worker boundary: `Vec<f32>` samples in, owned config in, owned output structs out. `WhisperContext`/`WhisperState` and any reference types never escape the worker thread.
- A closed reply oneshot at reply time means the caller cancelled — future-drop is a designed cancel path via the engine's drop guard — so the worker ignores the failed send quietly and moves on; the `let _ = req.reply.send(...)` shape is by design, not sloppiness.
- Config plumbing stays upgrade-ready (a `gpu_device` index today; widening to multiple devices/states is a config change, not an API change).

## Why

The dev grant runs a single A10; the researcher's production grant will want
multi-state or multi-GPU. A stable call shape makes that upgrade a swap-in
instead of a rewrite of every caller — while pooling from day one would have
frozen routing/fairness choices that can't be made well without a measured
production workload.

## Alternatives

- **WhisperPool from day one (N=1)** — premature; the pool's design choices (round-robin vs least-loaded vs work-stealing) need production measurements that don't exist.
- **Defer entirely, rewrite when needed** — forces every caller to change on upgrade day.

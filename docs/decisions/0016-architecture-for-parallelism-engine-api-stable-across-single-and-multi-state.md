---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-05-12 13:07:23"
      text: '(unrecoverable: legacy comment placeholder "1")'
    - author: Danielle McCool
      date: "2026-05-12 13:18:54"
      text: '2. (2026-05-12 13:18:54) Danielle McCool: Invariant qualifier (from codex code-quality review of T1): the closed-oneshot-is-Bug invariant applies during normal execution. During coordinated shutdown (process is exiting; callers intentionally drop their receivers), the worker observing a closed oneshot is expected, not Bug-class. T5''s worker loop should: (a) on a closed reply during normal execution, surface loudly (fatal log + structured Bug event); (b) on a closed reply after a shutdown signal has been observed, log shutdown-context once and exit cleanly. T5''s closed-oneshot Bug test exercises path (a). Path (b) lands when shutdown signaling is wired (likely Epic 2).'
legacy-outcome: true
---

# Architecture for parallelism Engine API stable across single and multi state

## Context and Problem Statement

Plan B targets a single A10 for dev grant cost. Downstream production (researcher's separate grant) will use multi-state and/or multi-GPU. How do we architect Epic 1's WhisperEngine so the production upgrade is a swap-in change, not a rewrite?

## Considered Options

* Stable public API; mutable internals. engine.transcribe(samples, cfg).await returns one result per call regardless of internal parallelism. Epic 1 ships single (context, state, worker thread). Plan C either (a) upgrades Engine internals to (context, Vec<state>, Vec<worker>, dispatcher) or (b) wraps WhisperPool of N Engines.
* Pool from day one — implement WhisperPool in Epic 1 with N=1 trivially routed
* Defer entirely — single-Engine for Plan B; rewrite when production needs multi

## Decision Drivers

Engine public API must be stable across single/multi-state internals. Configuration plumbing must anticipate multi-GPU. Worker-thread invariants must be documented so T5 (engine shell) can reference them.

## Decision Outcome
We decided for [Option 1](#option-1) because: Public API stays stable across single/multi-state internals; configuration plumbing anticipates with gpu_devices: Vec<i32> and states_per_gpu: usize (Epic 1 defaults gpu_devices=[0], states_per_gpu=1). Worker-thread invariants (relevant to upgrade path, referenced by T5): only owned data crosses the worker boundary (Vec<f32> samples, owned config, owned output structs); WhisperContext / WhisperState and any reference types stay inside the worker thread and never escape; a closed oneshot reply (caller dropped the receiver before the worker replied) is Bug-class — indicates a caller-side ordering error and must surface loudly, not silently. Rejected alternatives: Option 2 (pool from day one) — premature/YAGNI for Epic 1; a pool abstraction has routing/fairness/capacity cost that adds no value at N=1, and the design choices (round-robin vs. least-loaded vs. work-stealing) cannot be made well without measured production workload. Option 3 (defer entirely; rewrite later) — forces a rewrite of every caller when production needs arrive, violating the architect for parallelism guidance in the Plan B spec; cheaper to land the stable public-API shape now.

## Comments

* **2026-05-12 13:07:23 — @Danielle McCool:** (unrecoverable: legacy comment placeholder "1")
* **2026-05-12 13:18:54 — @Danielle McCool:** 2. (2026-05-12 13:18:54) Danielle McCool: Invariant qualifier (from codex code-quality review of T1): the closed-oneshot-is-Bug invariant applies during normal execution. During coordinated shutdown (process is exiting; callers intentionally drop their receivers), the worker observing a closed oneshot is expected, not Bug-class. T5's worker loop should: (a) on a closed reply during normal execution, surface loudly (fatal log + structured Bug event); (b) on a closed reply after a shutdown signal has been observed, log shutdown-context once and exit cleanly. T5's closed-oneshot Bug test exercises path (a). Path (b) lands when shutdown signaling is wired (likely Epic 2).

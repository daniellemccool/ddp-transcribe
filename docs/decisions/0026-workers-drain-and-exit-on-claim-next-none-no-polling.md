---
status: accepted
date: "2026-05-20"
category: Orchestration
applies_to:
    - src/pipeline/pipelined.rs
    - src/pipeline/serial.rs
priority: default
---

# Workers drain and exit on claim_next None — no polling

## Decision

Fetch workers exit on the first `claim_next() == None` (drain semantics).
`process` is batch-drain — the pending pool is frozen at startup by the prior
`ingest` phase — so there is no sleep-polling, no backoff loop, and no
"wait for more work" signal anywhere in the workers.

## Guidance

- Review rejects sleep/poll/backoff loops around `claim_next` and condvar-style work signals; a worker that sees `None` returns.
- The drain signal is structural: `JoinSet::join_next()` returning `None` after all workers exit IS batch completion — don't add a separate completion channel.
- A `process` run with zero pending rows exits immediately (claimed=0 → exit 3 via `CommandExit::NoClaims` in `src/commands.rs`).
- If a daemon mode (continuous ingest while draining) ever lands, polling policy is a new decision, not a tweak to this one — and claim-ordering starvation (see the in-pipeline retry record) must be revisited with it.

## Why

Polling burns CPU without producing work in a model where no new work can
arrive mid-run, and the machinery (backoff tuning, condvars) would be
redesigned anyway the day ingest becomes live. Drain semantics make batch
completion observable from task structure alone.

## Alternatives

- **Sleep-poll 100ms–2s with exponential backoff** — proposed in the original epic sketch, written before the batch-drain operational model was settled; pure waste once the pool is frozen.
- **Condition variable signaled by `claim_next`** — machinery a daemon-mode redesign would replace.

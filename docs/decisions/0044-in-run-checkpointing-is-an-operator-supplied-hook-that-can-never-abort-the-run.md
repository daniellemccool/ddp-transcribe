---
status: accepted
date: "2026-07-30"
category: Orchestration
applies_to:
    - src/pipeline/pipelined.rs
    - src/cli.rs
    - src/commands.rs
priority: invariant
companions:
    - tests/pipeline_fakes/pipelined_tests.rs
---

# Keep checkpoint hooks non-fatal

## Decision

In-run checkpointing is an operator-supplied hook: `process --checkpoint-cmd
<path> [--checkpoint-every <dur>]` runs that command on a supervised periodic
task through the bounded subprocess runner. The pipeline embeds no sync,
upload or snapshot logic of its own — what a checkpoint *does* is
deployment-specific and stays in the deploy repo.

## Guidance

- A hook failure — nonzero exit, timeout or spawn error — warns and bumps `checkpoints_failed`; review rejects any `?`, `return Err`, or other error escaping `checkpoint_task`, whose only `return` is `Ok(())` on cancellation. The supervision loop turns a task's first `Err` into `token.cancel()`, so a propagated hook error would kill the whole batch over a failed rsync.
- The hook runs through `process::run`'s bounded capture, never a raw spawn, and is invoked with no arguments. The task receives only the cancellation token, its config and the two counters — no `tx` clone, no store handle — so the load-bearing `drop(tx)` drain ordering is untouched and the hook can read no pipeline state.
- Timeout equals the interval and the loop sleeps before it runs: a hook slower than its own interval reports itself as a failure instead of stacking overlapping copies, and the first firing is one full interval after start, never at t=0.
- With the hook enabled the orchestrator cancels the token once every real worker has joined — otherwise the periodic task never joins and the run hangs at completion. Keep that cancel gated on the hook being enabled so unhooked runs stay byte-for-byte unchanged.
- `checkpoints_run` / `checkpoints_failed` ride `ProcessStats` into the census and `batch_runs`, and `checkpoint_cmd` / `checkpoint_every_secs` into `params_json` — whether a run was checkpointing, and whether it worked, must be answerable from the state DB alone.

## Why

An uncapped campaign run goes hours between exits, and the deployment's
run-boundary sync only fires when `process` exits — so the storage volume and
the operator's resume snapshot stale for the length of the run, with a human
ritual as the only defense. The *content* of a checkpoint stays outside the
binary because the binary ships by pinned release tag while sync paths,
credentials and transfer policy change on the deployment's own schedule. And
the run must survive its hook: hours of GPU work cannot be lost because an
rsync exited nonzero.

## Alternatives

- **Embed the sync (rsync + `.backup` snapshot) in the pipeline** — puts deployment-specific paths and credentials behind a tagged release; every sync tweak becomes a promotion.
- **Checkpoint every N videos rather than every N minutes** — couples the cadence to throughput, which swings with the failure mix; the operator's data-loss exposure is measured in wall clock.
- **Treat a hook failure as fatal** — a transient storage-mount hiccup would cost the batch; a counter plus a warn surfaces the same fact without spending the run.
- **An external cron/systemd timer on the VM** — no run-scoped record (neither `params_json` nor the census could say whether checkpointing was on), and it fires when no run is in progress.

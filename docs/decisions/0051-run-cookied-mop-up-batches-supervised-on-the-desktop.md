---
status: accepted
date: "2026-08-10"
category: Operations
applies_to:
    - docs/operations/cookied-runs.md
priority: invariant
---

# Run cookied mop-up batches supervised on the desktop

## Decision

Cookied fetch runs (the `SensitiveLoginGated` mop-up) happen on the
operator's desktop, supervised, in paced small batches — never on the VM
and never unattended. At campaign drain the writer role hands off: the VM
pauses, the final sync lands, and the desktop becomes the sole writer
against the authoritative DB.

## Guidance

- A cookied `process` invocation runs only with an operator watching the per-batch success rate: a dead or blocked jar fails *silently* by design (gated claims re-park, harmlessly), so supervision IS the detection mechanism. Review rejects any cookied invocation wired into the VM, cron, checkpoint hooks, or other unattended automation.
- Paced batch calculus (operator-confirmed 2026-08-10): `--max-videos 500` per session, one or two sessions per day, `--download-workers 1`, a fresh jar export plus one hand-validation `yt-dlp --cookies` fetch before every session. Stop on mid-batch success-rate sag (the jar died) or any account-level anomaly in the browser (captcha, forced re-login, verification prompt).
- This record revises the account-exposure calculus that cookie scoping was built on (~300 fetches → ~18,700 paced across many sessions); the gate itself is untouched — cookies still ride only requires-cookie retries via `cookie_opts_for`, and widening that stays under its own invariant.
- Sole-writer handoff is strict: no cookied batch against the authoritative DB before the VM has paused and the final sync landed — there is no DB merge. Rehearsals run only against disposable copies, never the sync target.
- Derive `--retries` from the live cohort's max `attempt_count` (strictly exceed it) before the first batch of each session block; the full procedure, arithmetic, and jar handling live in `docs/operations/cookied-runs.md`.

## Why

The only failure signal a dead jar emits is a sagging success rate a human
must notice — impractical on the VM — and keeping the session credential
off shared infrastructure caps the blast radius of a compromise; an
unsupervised or VM-hosted cookied run would burn the research account (and
donor-data recovery with it) without anyone noticing.

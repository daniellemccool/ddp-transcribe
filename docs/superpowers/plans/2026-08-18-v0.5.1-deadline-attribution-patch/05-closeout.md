# Task 05 — Close-out: FOLLOWUPS, runbook addendum, release notes, full gate

**Files:**
- Modify: `docs/FOLLOWUPS.md`, `docs/followups/production-run.md`,
  `docs/archive/followups-resolved.md`, `docs/operations/src-vm.md`
- Create: release-notes text (in the task report — NOT a repo file; the
  controller carries it into the PR description and the tag message)

**Interfaces:**
- Consumes: Task 01–04 commit SHAs (ask the controller if not provided in
  the dispatch; `git log --oneline` shows them on this branch).
- Produces: a merge-ready branch and the v0.5.1 tag text for the ADR-0043
  promotion (in-place tag checkout on the VM, per the 2026-08-13
  precedent recorded in the runbook).

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

- [ ] **Step 1: FOLLOWUPS resolution (ADR-0020 lifecycle)**

Resolve the entry titled **"yt-dlp's internal retry count is uncapped in
our argv (default 10)"** (`docs/followups/production-run.md`; its
scope-index line in `docs/FOLLOWUPS.md` begins "v0.5.0 batches 2026-08-13:
**yt-dlp internal retry count uncapped**"): move the entry body to
`docs/archive/followups-resolved.md` under a new section
`## Resolved by v0.5.1 — deadline-attribution patch (2026-08-18)`, appending
the resolution (Task 04's commit SHA; `--retries 3` in both argv builders;
config-file route explicitly not taken). Remove its scope-index line from
`docs/FOLLOWUPS.md`.

The deadline-attribution bug itself was never filed as a followup (it went
straight from incident to this plan) — it needs NO followup entry; the
archive section's header line plus the release notes are its record. State
this in the report so the reviewer doesn't hunt for a missing entry.

- [ ] **Step 2: Runbook addendum (`docs/operations/src-vm.md`)**

- **Version-state paragraph:** append: v0.5.1 exists (deadline-attribution
  patch); the VM update is the in-place tag-checkout sequence proven on
  2026-08-13 (fetch tags → checkout v0.5.1 → CUDA rebuild → `sudo cp` to
  `/usr/local/bin` → `-V` prints `0.5.1`); **no schema change — no
  `migrate` needed** (schema stays v7).
- **Known VM facts**, extend the terminal-semantics/incident block with one
  bullet: a per-item transcription that exceeds the 600 s deadline is a
  retryable `Timeout` since v0.5.1 (before: it killed the run — 2026-08-17
  incident); an aborted run now closes its `batch_runs` row with
  `"aborted": true` in the census JSON, so `finished_at IS NULL` no longer
  occurs and the crash marker is queryable.

- [ ] **Step 3: Release-notes draft (report-only)**

```
v0.5.1 — deadline-attribution patch

- A per-item transcription hitting its deadline (600 s) is now a retryable
  Timeout, not a run-terminating Cancelled (2026-08-17 incident: one
  over-deadline video killed the run and stranded 7 claims; the attempt-2
  tier would have re-triggered it at the census tail).
- A run that dies on a worker error now closes its batch_runs row with an
  "aborted" census (sweep counters + error string) instead of leaving
  finished_at NULL and losing the census.
- yt-dlp's internal retry loop is capped at 3 (was default 10 — up to
  ~3.5 min of stalled download worker per flaky connection).

Promotion per ADR-0043 steps 1-4 + in-place tag checkout on the VM (the
2026-08-13 precedent; no migrate needed — schema unchanged at v7).
```

- [ ] **Step 4: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1 && cargo build --release`
On the desktop additionally:
`PATH=/opt/cuda/bin:$PATH CUDAHOSTCXX=/usr/bin/g++-15 CUDAARCHS=75 cargo build --release --features cuda`
Run: `adg lean index --root .` (pre-commit re-runs it).
Expected: all green; paste the test-summary line into the report.

- [ ] **Step 5: Commit**

Commit: `git commit -am "docs: v0.5.1 close-out — retries followup resolved, runbook amended"`

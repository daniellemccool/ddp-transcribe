# FOLLOWUPS — production-run active entries

Active-scope items whose trigger is an operational milestone of the full
production run (2,982,471 unique videos, measured against the 144-donor inbox
on 2026-07-28) rather than a code epic. See `../FOLLOWUPS.md` for the scope
index across all groups; `../cosmetic-followups.md`, `../bake-findings.md`,
`../archive/followups-resolved.md` for sibling categories. The
unverified-hypothesis prefix rule (`**Hypothesis (unverified):**`) applies here
per 0020.

---

### Capacity estimate for the production batch

**Found in:** Epic 4c planning and close (the spec's scale measurement was the
epic's binding design constraint, but capacity planning was an explicit
non-goal).
**Disposition:** Required before the full run starts; not a code change.
**Trigger to revisit:** before the first non-pilot `process` batch is launched.

The 2026-07-28 inbox measures 144 donors / 4,847,408 watch entries /
**2,982,471 unique videos** — ~53× the 56,620-video pilot corpus. Nothing in
Plan B has been sized against that number. The estimate needs at least:

- **Fetch + transcribe throughput.** Pilot-derived per-video wall clock on the
  A10, times two GPU instances, against the unique-video count — does the run
  fit the workspace's available window at all, and by what margin?
- **Window narrowing.** Epic 4b's `--window-start`/`--window-end` is the lever
  that cuts the fetch set. The estimate should state what analysis window the
  PI actually needs and how many uniques survive it — the design deliberately
  does *not* assume the window helps, so this has to be measured, not hoped.
- **Disk.** `video_metadata_raw` at the descoped envelope size (~0.6–1 KB per
  video measured live; call it 1–2 KB with SQLite row overhead) is roughly
  **3–6 GB** across the full corpus, on the boot disk beside the state DB. The
  loader's page-at-a-time streaming keeps RAM flat, but the table is permanent
  until someone decides to prune it (see the entry below). Transcript
  artifacts and the transient per-video WAVs also need a hot-path number:
  WAVs are removed after each success, so the ceiling is roughly
  `concurrent fetch workers × largest WAV`, but that has never been checked
  against the boot disk's free space under two instances.
- **Rate-limit exposure.** ~3M fetches is a different regime from the pilot's
  56k; the failure mix under sustained load is unmeasured.
- **Coverage, not just size.** `video_metadata_raw` coverage will run meaningfully
  below attempted-fetch count — the ~15% dead-link cohort and all structural
  failures (timeouts/spawn) leave no row; only fetches whose extraction
  succeeded do.

**Note on the disk figure:** the Epic 4c plan quoted "6–12 GB" for
`video_metadata_raw`. That number predates the caption descope, which removed
the largest projected component of the envelope. The corrected estimate is the
3–6 GB above. ~~The `src/metadata_loader.rs` comment repeating the stale
figure is a cosmetic fix for whoever next edits that file.~~ **Done** — the
comment now reads 3–6 GB (v0.3.1 doc pass). The rest of this entry (throughput,
window narrowing, WAV headroom, rate-limit exposure, coverage) is still open.

**2026-07-29 triage — measured; HOLD:** throughput and window-narrowing were
measured in `docs/operations/capacity-estimate-2026-07-29.md` (commit
`e73e2f0`). Status: **HOLD** pending the 4-download-worker A/B and the PI's
window decision. Archive this entry once the PI summary ships.

---

### `video_metadata_raw` prune / VACUUM decision

**Found in:** Epic 4c close.
**Disposition:** Deliberately deferred — the right answer depends on what the
first real batch's blobs actually cost and on whether a re-parse is still
plausible at that point.
**Trigger to revisit:** after the first production `process` batch has run and
`load-metadata` has completed successfully over it.

Once `load-metadata` has parsed a batch's envelopes into the typed columns, the
raw blobs are redundant *for the current parse* but not for any future one —
that replayability is the entire point of ADR-0042. The operator then has a
choice nobody has made yet:

- **Keep them.** Any later field addition (`title`, `channel_id`, `duration`,
  `repost_count` are already captured but untyped) is a `load-metadata` re-run.
  Cost: the table stays on the hot path forever.
- **Prune for export.** Keep the working DB intact but strip
  `video_metadata_raw` from the copy that ships to the researcher, so the
  delivered artifact is lean. Needs a decision about who holds the unpruned
  copy and for how long.
- **Prune and `VACUUM` in place.** Reclaims the space but discards
  replayability permanently. `VACUUM` on a multi-GB DB needs free space equal
  to the DB size and a window where nothing else is running.

Decide with a real number in hand, not the estimate above. Whatever is chosen,
record it — a future reader finding an empty `video_metadata_raw` should be
able to tell "pruned deliberately" from "the capture never worked".

---


### Cookie-gated residue after the first `backfill-metadata` run

**Found in:** the 2026-07-29 `backfill-metadata` design review (codex-advisor
argv pass) and the branch's review loop.
**Disposition:** Nothing to do until there is a real number. The subcommand
ships deliberately cookie-free.
**Trigger to revisit:** the first full backfill run's stats line — specifically
the `capture-failed` count and what a hand-probe of a sample of those videos
says.

**Hypothesis (unverified):** some fraction of the ~10,235-video backfill cohort
has become login-gated since it was originally fetched (rc1 fetched and
transcribed these videos successfully, so they were reachable then). Those rows
would count `capture-failed` on every run and never drain — a permanent residue
rather than a transient failure. The size of that residue is unmeasured; it is
equally possible the residue is dominated by outright deletions, which no cookie
would recover.

If the residue turns out to be material *and* attributable to gating, extending
`backfill-metadata` to carry cookies would require an **explicit ADR-0035
revision**, not a quiet argv change: widening the cookie gating-class was
considered and deliberately rejected in the 2026-07-29 design review, on the
grounds that a metadata-only sweep is exactly the wrong place to put the
study's session credential (it touches the whole cohort, not the narrow
login-gated retry path 0035 scopes cookies to). Measure first; if the answer is
"yes, and it matters", write the ADR revision before the code.

**Two argv-hardening candidates rejected as out-of-scope for v0.3.1**, both
gated on the same trigger (the first full run's `capture-failed` stats):

- **`--ignore-no-formats-error`** — would let yt-dlp still print the info dict
  for videos whose *formats* have expired while the metadata is otherwise
  extractable. Plausibly a real slice of the rc1 cohort, since these videos are
  months old and format URLs age out faster than the entries themselves. Held
  back because it changes what "capture succeeded" means and nobody has
  measured how many videos it would recover; add it only with a
  before/after count from a real run.
- **A `--` separator before the URL positional** — purely defensive, against a
  `source_url` that begins with `-` and would otherwise be parsed as a flag.
  Today's cohort URLs are canonical TikTok watch URLs built by the ingest path,
  so the exposure is theoretical. This mirrors the standing Plan C entry for the
  same hardening on the fetch argv (`docs/followups/plan-c.md`, "yt-dlp argv
  `--` separator before `source_url`") — if it lands there, land it in
  `build_metadata_only_args` at the same time rather than separately.

---

### Concurrent-writer state updates lost, then idempotently re-done

**Found in:** first 2×A10 production shakedown (2026-07-28), the R11
two-writer scenario's first real test.
**Hypothesis (unverified):** one of two concurrent `process` runs (its own
summary: `claimed=13 succeeded=13`, `stale_after_*=0`) had its DB updates
absent minutes later — its videos read `pending` again while their transcript
artifacts existed on disk; a later uncapped sweep re-claimed and re-did them
(ADR 0008 idempotency absorbed the loss as duplicate GPU work). The
alternative — that the observing query itself misread — was not excluded:
both spot-checked IDs read `succeeded` after the sweep, so the direct
evidence window closed.
**Disposition:** verify/instrument the two-writer commit path before any code
change; do not fix blind.
**Trigger to revisit:** any observed increase in the `pending` count during
the campaign (operators watch the 5-minute tally; a bump = recurrence, note
the timestamp), or the next epic touching claiming/state commits.

---

### `process` claims more than `--max-videos`

**Found in:** the same shakedown — runs invoked with `--max-videos 5`
reported `claimed=13` and `claimed=10`.
**Disposition:** cap accounting appears per-worker rather than per-run.
Harmless for uncapped campaign use; misleading for smoke tests and capped
batches.
**Trigger to revisit:** next epic touching the claim loop / fetch workers.

**2026-07-29 triage correction:** the "per-worker cap accounting" diagnosis
above is **falsified**. The claim cap is a run-shared
`Arc<AtomicUsize>` checked, incremented, and claimed inside the same
`store.lock().await` guard (`src/pipeline/pipelined.rs:266-289`) — race-free
by construction across N concurrent fetch workers. That fix landed in
`9228c89` on 2026-05-21, *before* the 2026-07-28 shakedown observation, so
per-worker accounting cannot explain `claimed=13`/`claimed=10` under
`--max-videos 5`. The overshoot is therefore **unexplained** and belongs
with the concurrent-writer instrumentation work below — treat the two
entries as one two-writer anomaly cluster rather than two separate bugs.

---

### Periodic in-run checkpoint for uncapped campaign runs

**Found in:** campaign ops 2026-07-29 — the batch-end auto-sync (hop 1)
only fires when a `process` invocation exits, so an uncapped campaign run
staled the volume (and the Yoda-pushed resume snapshot) for hours until a
manual `sync-to-storage.sh`. Documented as an operator ritual in the
researchcloud repo (`yoda-operations.md`, "Campaign checkpoint ritual"), but
the pipeline could emit a periodic checkpoint (or invoke a configurable hook)
every N videos/minutes and remove the human dependency.
**Disposition:** ops-robustness feature, small.
**Trigger to revisit:** the ritual getting missed in practice, or the next
ops-focused epic.

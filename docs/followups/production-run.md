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

**Note on the disk figure:** the Epic 4c plan quoted "6–12 GB" for
`video_metadata_raw`, and a comment in `src/metadata_loader.rs` still does.
That number predates the caption descope, which removed the largest projected
component of the envelope. The corrected estimate is the 3–6 GB above; the
stale comment is a cosmetic fix for whoever next edits that file.

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

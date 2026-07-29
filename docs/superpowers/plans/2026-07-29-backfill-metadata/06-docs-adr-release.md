# Task 06: ADR revision, doc fixes, FOLLOWUPS lifecycle, release notes

**Files:**
- Modify: `docs/decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md` — **via `adg` / the `write-adr:write-lean-adr` skill ONLY, never hand-edited**
- Modify: `src/metadata_loader.rs` (one stale comment)
- Modify: `docs/operations/src-vm.md` (backfill runbook section)
- Modify: `docs/FOLLOWUPS.md`, `docs/followups/epic-5.md`, `docs/followups/production-run.md`, `docs/archive/followups-resolved.md` (ADR-0020 lifecycle)
- Create: `docs/superpowers/plans/2026-07-29-backfill-metadata/RELEASE-NOTES-v0.3.1.md` (draft for the annotated tag)

**Interfaces:**
- Consumes: the landed Tasks 01–05 (commit SHAs from `git log --oneline` on this branch — cite them in FOLLOWUPS resolutions), the shipped subcommand surface (`backfill-metadata [--limit N] [--dry-run]`, stats line `examined / captured / capture-failed / already-filled / insert-failed`).
- Produces: docs consistent with the code; release-notes draft the operator uses in step 3 of the overview's release checklist.

**Semantics (binding):**
- The executing subagent MUST load `write-adr:write-lean-adr` before touching `docs/decisions/` and author through `adg` (`adg lean new --from-stdin` / the skill's revision flow). The pre-commit hook runs `adg lean index --root .` + `adg lean check`; fix inconsistencies, never bypass.
- FOLLOWUPS lifecycle per ADR-0020: a resolved entry's body moves to `docs/archive/followups-resolved.md` with the resolving commit SHA; its one-line pointer leaves the `docs/FOLLOWUPS.md` scope index. New entries: body in the right `docs/followups/<group>.md` + one-line pointer in the index.

- [ ] **Step 1: Revise ADR-0042 (the backfill carve-out)**

Load `write-adr:write-lean-adr`; revise ADR-0042 through the skill's flow so that:

- `applies_to` gains `src/backfill.rs`.
- Guidance acquires one bullet reconciling the backfill with the existing "review rejects a separate enrichment invocation or any extra request per video" rule, in this spirit:
  - *The fetch path stays single-invocation; `backfill-metadata` is the one sanctioned separate metadata-only invocation, existing solely to recover cohorts that predate fetch-time capture (rc1). It writes through `insert_metadata_raw_if_missing` (never the fetch path's last-write-wins upsert), never touches video status/lifecycle, and never carries cookies (0035). A backfilled envelope is schema-identical to a fetch-time one; `load-metadata` cannot tell them apart.*
- The existing "metadata must never create a new failure mode" bullet's enforcement list gains the backfill loop (per-video failures count and continue).

Run `adg lean check` on the record; fix whatever it flags.

- [ ] **Step 2: Fix the stale size comment**

`src/metadata_loader.rs` line ~12: the comment citing **"6–12 GB"** at production scale is stale — the corrected estimate is **3–6 GB** post-caption-descope (`docs/followups/production-run.md` records this and names this exact comment as the pending cosmetic fix). Update the number in place; touch nothing else in the file.

- [ ] **Step 3: Runbook section**

In `docs/operations/src-vm.md`, add a `backfill-metadata` section adjacent to the existing `load-metadata` material, matching the file's voice and structure. It must cover:

1. What it recovers (the rc1-era cohort: succeeded videos with no envelope; ~10,235 at the 2026-07-29 snapshot) and what it never does (no media, no GPU, no status/lifecycle writes, no cookies).
2. Safe alongside a live `process` run (WAL + busy_timeout; serial loop is the rate limiter). Expected duration ~2–4 h for the full cohort.
3. The operator sequence: `backfill-metadata --dry-run` (cohort size sanity check) → `--limit 5` smoke → verify the 5 new envelopes parse via `load-metadata --dry-run` → full run → `load-metadata` at a convenient boundary.
4. Interpreting the stats line (`examined / captured / capture-failed / already-filled / insert-failed`); capture-failed is expected for dead/blocked videos — re-runs converge on whatever remains.
5. Timeouts/spawn failures lose stdout (same as fetch-path captures) — the video just counts as capture-failed and a re-run retries it.

- [ ] **Step 4: FOLLOWUPS lifecycle**

1. **Resolve** the `global = true` entry (`docs/followups/epic-5.md` §"`--whisper-model` global flag rejected…", index line `docs/FOLLOWUPS.md:~65`): move the body to `docs/archive/followups-resolved.md` with Task 05's commit SHA; note the final scope was **10 flags** (the body's "seven" table predated Epic 4a/4c growth); delete the index line.
2. **Update** the "Cargo package version must track release tags" entry (`docs/followups/production-run.md`, index line `docs/FOLLOWUPS.md:~84`): it resolves at the v0.3.1 **tag commit** (post-merge, release checklist step 2) — annotate the entry as "resolution in flight: v0.3.1 tag commit" now; the post-merge doc pass archives it with that SHA. Do not archive it early.
3. **Update** the production-run "capacity estimate" entry only if it names the stale `metadata_loader.rs` comment — mark that cosmetic sub-item done (Step 2).
4. **Add** one new entry (group: production-run): *cookie-gated metadata residue* — hypothesis (unverified per 0020): some backfill cohort videos may have become login-gated since fetch and will persistently count as capture-failed; if the post-run residue is material, a cookie-carrying extension would need an explicit ADR-0035 revision (gating-class widening was deliberately rejected in the 2026-07-29 design review). Trigger: first full backfill run's stats.

- [ ] **Step 5: Release-notes draft**

Create `docs/superpowers/plans/2026-07-29-backfill-metadata/RELEASE-NOTES-v0.3.1.md`, ready to paste into `git tag -a v0.3.1`:

```markdown
v0.3.1 — metadata backfill + CLI ergonomics

- NEW `backfill-metadata` subcommand: recovers video_metadata_raw
  envelopes for succeeded videos that predate fetch-time capture (the
  rc1-era cohort, ~10,235 videos). Metadata-only yt-dlp per video — no
  media, no GPU, never touches video status, never carries cookies.
  Best-effort and re-runnable; insert-if-missing (never overwrites a
  fetch-path envelope). `--dry-run` prints the cohort; `--limit N` for
  smoke runs. Run `load-metadata` afterwards to fill typed columns.
- All GlobalArgs flags now accept placement after the subcommand
  (`global = true` on all 10 remaining flags; SRC-bake + T11 followup).
- Cargo package version now tracks release tags (this tag's commit
  bumps 0.1.0 → 0.3.1; `-V` finally means something).
- ADR-0042 revised: backfill carve-out. New cohort queries +
  insert-if-missing mutator in the state layer.

Upgrade: in-place per docs/operations/src-vm.md (build + cp + -h
check); catalog pipeline_git_ref → v0.3.1.
```

Adjust wording to match what actually landed (check DEVIATIONS in earlier task reports).

- [ ] **Step 6: Full verification + hook gate**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green (Step 2 touched a comment only). Then `adg lean index --root . && adg lean check docs/decisions/0042-*.md` — clean.

- [ ] **Step 7: Commit**

```bash
git add docs/ src/metadata_loader.rs
git commit -m "docs: ADR-0042 backfill carve-out; runbook backfill section; FOLLOWUPS lifecycle; v0.3.1 release notes draft"
```

(The pre-commit hook re-runs the adg gate; if it fails, fix the `docs/decisions/` inconsistency — never bypass.)

# Task 13: matrix execution, FOLLOWUPS lifecycle, docs pass, release notes (Phase 3, last)

**Files:**
- Modify: `docs/FOLLOWUPS.md`, `docs/followups/epic-5.md`, `docs/followups/cross-epic.md`, `docs/archive/followups-resolved.md` (ADR-0020 lifecycle), `DISPOSITION-MATRIX.md` (final states)
- Modify: `README.md`, `docs/operations/src-vm.md`, `docs/reference/architecture/index.md` + affected deepdives (bin/lib shape, commands module, requeue-failures, attempt-dir lifecycle, CLI examples)
- Create: `docs/superpowers/plans/2026-07-30-epic-5b-plan-b-closeout/RELEASE-NOTES-v0.4.0.md`

**Interfaces:**
- Consumes: the landed Tasks 01–12 (cite SHAs from `git log --oneline` on the branch); the DISPOSITION-MATRIX with operator rulings; RELEASE-NOTES-v0.3.2.md as the format template.
- Produces: docs consistent with shipped code; the v0.4.0 tag-notes draft; Plan B's DoD proven.

**Semantics (binding):**
- **Archive-integrity checks (never re-archive):** verify code still matches the three archived resolutions — provenance (`VideoFetcher::name`/`Transcriber::name` + stamping), `From<RunError>` (@9974d69 behavior), `--whisper-model` global (@7dfa771 behavior). Record "verified <date>, matches" in the matrix; a mismatch becomes a NEW active FOLLOWUPS entry (and this task reports DONE_WITH_CONCERNS naming it).
- **FOLLOWUPS lifecycle per ADR-0020:** every entry whose matrix row says fix/archive moves to `docs/archive/followups-resolved.md` with its resolving Task SHA; index lines removed; re-routed entries move to the Plan C group with rationale; hypotheses keep their `**Hypothesis (unverified):**` prefixes. The Plan-B DoD check: after this task, the Epic 5 group and the Plan-B-scope cross-epic entries are empty or explicitly re-routed — state this in the commit body with the final counts.
- **Docs pass:** README (crate-shape/commands description, `requeue-failures` in the CLI section with the default-deny grammar and the `--retries > A` note, attempt-dir lifecycle in workflow text); runbook (requeue-failures operator section with the cap-exhausted-cookie-cohort example and the manual-SQL-is-unsupported note; `.work` sweep note if Task 07 didn't fully cover it); architecture doc set (index.md ADR map gains the epic's new records; §4/§6 attribution → Epic 5b; state-machine/orchestration/data-input statements that referenced the bin/lib split or the old dispatch shape). Grep-verify: `rg -n 'main.rs.*mod|walking skeleton|requeue-retryables|reset-stale-claims' README.md docs/` returns nothing stale.
- **Release notes:** same shape as RELEASE-NOTES-v0.3.2.md (preamble + fenced tag-message block + verification footnote with the branch's final gate totals incl. the 261-runnable census and the release/cuda builds). Content: restructure (user-invisible, binary behavior identical; census note), `requeue-failures` (grammar summary + arithmetic note), attempt-dir lifecycle + `.work` sweep, 0013 backend assertion, sync-IO policy, hygiene/test bundles; v0.4.0 supersedes v0.3.2 for future deployments; the campaign VM stays on v0.3.0 and the catalog is unchanged.
- No `Cargo.toml` bump on the branch (ADR-0043 — the post-merge tag commit owns it).

- [ ] **Step 1: Archive-integrity checks** (three named above) + record results in the matrix.
- [ ] **Step 2: FOLLOWUPS lifecycle** per the matrix; final counts into the commit body.
- [ ] **Step 3: Docs pass** + the grep-verify sweep.
- [ ] **Step 4: Release-notes draft.**
- [ ] **Step 5: Full verification** — complete gate (fmt, clippy, single-threaded suite, `cargo build --release`, and `cargo build --release --features cuda`) + `adg lean index --root .` clean.
- [ ] **Step 6: Commit**

```bash
git add docs/ README.md
git commit -m "docs: Epic 5b close-out — FOLLOWUPS lifecycle to empty/re-routed, archive-integrity verified, docs pass, v0.4.0 release notes draft"
```

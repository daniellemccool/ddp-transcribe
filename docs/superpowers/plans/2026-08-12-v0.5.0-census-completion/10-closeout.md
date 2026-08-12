# Task 10 — Close-out: FOLLOWUPS, runbook, release notes, full gate

**Files:**
- Modify: `docs/FOLLOWUPS.md`, `docs/followups/production-run.md`,
  `docs/archive/followups-resolved.md`, `docs/operations/src-vm.md`
- Conditional: `docs/operations/incident-2026-08-10-tiktok-waf-impersonation-block.md`
  (only if present on this branch — it lives on its own docs branch)
- Create: release-notes text (in the PR description / tag message draft —
  not a repo file)

**Interfaces:**
- Consumes: everything Tasks 01–09 landed (ADR numbers now known — use the
  real ones, they were adg-assigned in Tasks 01/04/07).
- Produces: a merge-ready branch and the v0.5.0 tag text for the ADR-0043
  promotion.

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

- [ ] **Step 1: FOLLOWUPS resolutions (per ADR-0020 lifecycle)**

- **Resolve** "Mass-instant-failure circuit breaker for the fetch path"
  (`docs/followups/production-run.md`): move the body to
  `docs/archive/followups-resolved.md` with the Task 08 commit SHA; remove
  its scope-index line from `docs/FOLLOWUPS.md`.
- **Annotate** "`--impersonate` belongs in `build_yt_dlp_args`"
  (`docs/followups/production-run.md`): add a dated note that v0.5.0
  shipped the URL-derivation half (the fetch-URL ADR — cite its real
  number) and the env echo (Task 09), so what remains open is strictly the
  `--impersonate` flag machinery, still gated on evidence that
  impersonation is needed again. Update its scope-index line if the title
  reads stale.
- **Leave** the yt-dlp version-drift entry untouched (its trigger hasn't
  fired).

- [ ] **Step 2: Runbook amendment (`docs/operations/src-vm.md`)**

- Version-state paragraph: v0.5.0 exists; the workspace upgrade jumps
  0.3.0 → 0.5.0 in one promotion; after relaunch `-V` must print `0.5.0`,
  `process -h` must show `--breaker-threshold`, and the DB needs one
  `migrate` (v6 → v7, idempotent, refuses on non-19-digit canonical ids).
- Exit-code table: add **4 = circuit breaker tripped** (consecutive
  no-success streak ≥ threshold; census carries `breaker_tripped`; the
  correct operator response is probe-matrix first, restart second).
- Resume sequence: cite spec D6 (validation batch `--max-videos 50
  --retries 2` → 10k rate measurement → capped batches; hourly dead-man
  census check is deploy-side).

- [ ] **Step 3: Incident-2 supersession note (conditional)**

If `docs/operations/incident-2026-08-10-tiktok-waf-impersonation-block.md`
exists on this branch, edit its "Remaining steps" section: mark the SQL
URL-rewrite **SUPERSEDED by the fetch-URL ADR (v0.5.0 derives at claim
time; the UPDATE must not run)** and leave the rest of the section intact
as history. If the file is not on this branch, put the same sentence in
the PR description instead, addressed to whoever merges the incident
branch.

- [ ] **Step 4: Release notes draft (for the ADR-0043 tag commit)**

Draft in the PR description:

```
v0.5.0 — census-completion release

- Claim order: newest-published first within attempt tiers (<claim-order ADR>);
  schema v7 (idx_videos_pending_v4 + 19-digit width guard; run `migrate`).
- Fetch transport: canonical fetch URL derived from video_id at claim time
  for canonical rows (<fetch-URL ADR>); videos.source_url is provenance and
  is never rewritten. backfill-metadata uses the same derivation.
- Circuit breaker: --breaker-threshold (default 50, 0 disables); trips
  cancel-and-drain, census-visible, exit code 4 (<breaker ADR>).
- Observability: params_json carries fetch_url_form, breaker_threshold,
  ytdlp_version, ytdlp_impersonation_available.

Promotion per ADR-0043: merge → tag (this text, plus Cargo.toml bump to
0.5.0 in the tag commit) → push tag → bump SRC pipeline_git_ref → relaunch.
Operator validation per spec D6 before uncapping.
```
(Substitute the real ADR numbers.)

- [ ] **Step 5: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1 && cargo build --release`
On the desktop additionally:
`PATH=/opt/cuda/bin:$PATH CUDAHOSTCXX=/usr/bin/g++-15 CUDAARCHS=75 cargo build --release --features cuda`
(laptop cannot run this — CLAUDE.md "CUDA is per-machine").
Run: `adg lean index --root .` (pre-commit re-runs it).
Expected: all green; paste the test-summary line into the report.

- [ ] **Step 6: Commit and hand to review**

Commit: `git commit -am "docs: v0.5.0 close-out — followups resolved, runbook amended"`
Then `superpowers:requesting-code-review` / the 0018 three-tier protocol
for the branch, and `superpowers:finishing-a-development-branch` for the
merge/PR decision. The ADR-0043 promotion itself (tag + bump +
`pipeline_git_ref`) is the operator's move after merge, followed by spec
D6's staged validation on the VM.

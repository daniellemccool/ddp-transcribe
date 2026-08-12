# v0.5.0 census-completion — draft PR description

Draft only — not the tag message. Cut the tag from the release-notes
section below (substituted with real ADR numbers already) per ADR-0043 at
promotion time. The other three sections are handoff notes addressed to
whoever next merges the two unmerged incident-docs branches
(`docs/incident-2026-08-06-tiktok-tls-403`,
`docs/incident-2026-08-10-waf-impersonation-block`) into this history —
keep this file until both have merged and been reconciled, then it can be
deleted.

---

## (a) Release notes (Step 4)

```
v0.5.0 — census-completion release

- Claim order: newest-published first within attempt tiers (ADR-0048);
  schema v7 (idx_videos_pending_v4 + 19-digit width guard; run `migrate`).
- Fetch transport: canonical fetch URL derived from video_id at claim time
  for canonical rows (ADR-0049); videos.source_url is provenance and
  is never rewritten. backfill-metadata uses the same derivation.
- Circuit breaker: --breaker-threshold (default 50, 0 disables); trips
  cancel-and-drain, census-visible, exit code 4 (ADR-0050).
- Observability: params_json carries fetch_url_form, breaker_threshold,
  ytdlp_version, ytdlp_impersonation_available.

Promotion per ADR-0043: merge -> tag (this text, plus Cargo.toml bump to
0.5.0 in the tag commit) -> push tag -> bump SRC pipeline_git_ref -> relaunch.
Operator validation per spec D6 before uncapping (see docs/operations/src-vm.md,
"Resume sequence after the v0.5.0 promotion").
```

---

## (b) Incident-2 supersession note (Step 3)

`docs/operations/incident-2026-08-10-tiktok-waf-impersonation-block.md` is
not present on this branch (it lives on the unmerged
`docs/incident-2026-08-10-waf-impersonation-block` branch), so this note
could not be applied to the file directly. **Addressed to whoever merges
that branch:**

> The "Remaining steps" section's SQL `UPDATE` rewriting `videos.source_url`
> to the canonical form is **SUPERSEDED by ADR-0049** ("Derive the fetch URL,
> never rewrite provenance") — v0.5.0 derives the canonical fetch URL at
> claim time in code (`canonical::derived_fetch_url`), so that UPDATE must
> never run. Leave the rest of that section intact as history; mark this one
> step superseded, don't delete it.

---

## (c) Impersonate FOLLOWUPS annotation (Task 10 Step 1, deferred)

The active FOLLOWUPS entry this annotates (`--impersonate` belongs in
`build_yt_dlp_args`, or incident-2's rewrite of it, "fetch-URL construction
belongs in `build_yt_dlp_args`") does not exist on this branch — it is
filed only on the two unmerged incident-docs branches. **Addressed to
whoever merges either branch: apply this dated note to that entry before
leaving it active:**

> **2026-08-12 (v0.5.0):** v0.5.0 shipped the URL-derivation half (ADR-0049
> — canonical URL derived from `video_id` at claim time; `source_url` never
> rewritten) and the yt-dlp environment echo (Task 09 —
> `ytdlp_version` / `ytdlp_impersonation_available` in
> `batch_runs.params_json`). What remains open after v0.5.0 is strictly
> explicit `--impersonate` flag machinery in `build_yt_dlp_args` — still
> gated on evidence that impersonation is needed again. (It currently is
> not: the working remedy proven in the 2026-08-10 incident is
> *unimpersonated* canonical fetch — impersonation is what caused that
> incident, not its fix.) Update the entry's title/scope-index line if it
> still reads as though impersonation were the goal.

---

## (d) Breaker followup — merge-ordering note (mechanical, do not skip)

**This is the enforcing instruction, not just prose.** The sibling incident
branch(es) still carry an **active** FOLLOWUPS entry for "Mass-instant-failure
circuit breaker for the fetch path." This branch has already resolved and
**archived** that finding:

- Resolution: ADR-0050 ("Trip the breaker, never burn the pool"),
  implemented in `04a457f`.
- Archive location: `docs/archive/followups-resolved.md`, section
  "Resolved by v0.5.0 — census-completion release (2026-08-12)".

**Whoever merges `docs/incident-2026-08-06-tiktok-tls-403` and/or
`docs/incident-2026-08-10-waf-impersonation-block` into a branch that
already has this v0.5.0 close-out merged:**

1. **Do not let the merge silently reintroduce the breaker entry as
   active.** Git will happily merge the incoming branch's new
   `docs/FOLLOWUPS.md` scope-index line and `docs/followups/production-run.md`
   body for it — check for that specifically and remove/archive it as part
   of the same merge, pointing at the existing archive entry above instead
   of duplicating it.
2. Apply the impersonate annotation from section (c) above to that same
   incoming entry (or its incident-2 rewrite) before leaving it active.
3. Apply the incident-2 supersession note from section (b) above if that
   file lands with the "remaining steps" SQL rewrite still unmarked.

Until this reconciliation happens, `docs/FOLLOWUPS.md` on this branch
carries neither an active breaker nor an active impersonate entry — by
design, not oversight (see the Task 10 report for the full reasoning).

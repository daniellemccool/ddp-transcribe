# Followups — active-scope entries

Active-scope FOLLOWUPS entries scheduled for an upcoming Plan B epic (or
explicitly routed to Plan C). Each entry names the task or context where
the finding arose, the disposition, and the trigger that should re-surface
it. When an entry is resolved, move it (with the resolving commit SHA) to
`docs/archive/followups-resolved.md`; do not just delete.

Sibling files (off the orchestrator's planning-time reading path):

- `docs/cosmetic-followups.md` — items deferred indefinitely; touch when
  the surrounding file gets edited for unrelated reasons.
- `docs/bake-findings.md` — operational findings from bake runs; not
  code-quality FOLLOWUPS.
- `docs/archive/followups-resolved.md` — append-only history of resolved
  entries.

Per-epic entry bodies live in sibling files under `docs/followups/` and
are loaded only when an epic's task expansion needs them — see the
"Full entries" pointers in each scope-index group below.

**Discipline:** entries that record unverified hypotheses must prefix the
hypothesis with `**Hypothesis (unverified):**` so the next operator knows
to verify before acting (per 0020).

## Maintenance

- **Add an entry:** append the full body to the appropriate
  `docs/followups/<group>.md` file; add a one-line scope-index entry below
  pointing at it.
- **Modify:** edit the body in the sub-file. Update the scope-index line
  if the title or disposition changed.
- **Re-target** (e.g., Epic 3 → Epic 4): move the body between sub-files
  and update its scope-index line.
- **Resolve:** move the body to `docs/archive/followups-resolved.md` with
  the resolving commit SHA; remove its scope-index line.

---

## Scope index

Grouped by target epic; format `T<n>: <short title> → Epic <N> <task hint>`.
Routing is authoritative per `docs/superpowers/plans/2026-05-12-plan-b/EPIC-5-SKETCH.md`
lines 120-148.

**Epic 2 (concurrent fetch + state-machine)**
- (no active entries — T17 archived @964e9c2, T5-Epic1 re-routed to Epic 5)
- Full Epic 2 entries: [followups/epic-2.md](followups/epic-2.md)

**Epic 3 (failure classification taxonomy)** — closed 2026-07-07. All ten entries resolved
(archived with resolving SHAs in [archive/followups-resolved.md](archive/followups-resolved.md),
section "Resolved by Plan B Epic 3") or split-and-re-filed: `YtDlpFetcher::acquire`
finding 3 → Epic 5, finding 4 → Plan C (see those groups below).

**Epic 5 (Plan A → Plan B cleanup sweep)** — closed 2026-07-30. All 21 entries
resolved: archived with resolving SHAs in
[archive/followups-resolved.md](archive/followups-resolved.md), section
"Resolved by Plan B Epic 5b — close-out slice / v0.4.0". Three are archived as
**accepted** under operator rulings rather than fixed (tmp-sweep TOCTOU window;
ingest file-ledger items 1–2), each keeping its evidence-gated re-open
condition in the archive.
- (no active entries)
- Full Epic 5 entries: [followups/epic-5.md](followups/epic-5.md)

**Production run (operational milestones, not code epics)**
- Epic 4c close: capacity estimate for the production batch — 2,982,471 uniques, throughput / window narrowing / disk (`video_metadata_raw` ~3–6 GB, transient WAVs) → measured 2026-07-29, 4-worker A/B included (`docs/operations/capacity-estimate-2026-07-29.md`, commits 9e43211 + a839ac7); HOLD pending only the PI window decision
- Epic 4c close: `video_metadata_raw` prune / VACUUM decision — keep for re-parse, prune for export, or reclaim in place → after the first production batch's `load-metadata` completes
- v0.3.1 backfill: cookie-gated metadata residue — hypothesis (unverified) that part of the cohort is now login-gated; carries two rejected argv-hardening candidates (`--ignore-no-formats-error`, `--` separator) → after the first full `backfill-metadata` run's stats
- Campaign shakedown 2026-07-28: **Hypothesis (unverified):** concurrent-writer lost updates (one run's 13 successes reverted to pending, idempotently re-done) → instrumented in v0.3.2 (`31c18df`); adjudicate at the next `pending` bump via `swept_stale` events, then graduate to a fix task
- Campaign shakedown 2026-07-28: `process` claims beyond `--max-videos` → same two-writer cluster, same v0.3.2 instrumentation; per-worker cap accounting diagnosis falsified 2026-07-29 (fix landed 9228c89, predates the observation)
- Epic 5a T03 review: `swept_stale` event-set/recovered-set invariant is enforced only by `debug_assert_eq!` (compiled out of release) → next-campaign hardening; operator ruling: prefer a DB-visible signal over a log warn
- ~~403-incident 2026-08-06: `--impersonate` belongs in `build_yt_dlp_args`~~ **SUPERSEDED by the WAF incident** — impersonation is now the *failure*, not the fix; see the entry below
- WAF-incident 2026-08-10: **deploy-repo curl_cffi reinstatement trap** + evidence-gated `--impersonate` machinery — URL-derivation half RESOLVED by v0.5.0 (ADR-0049, claim-time derivation; DB rewrite superseded); what's left: fix the deploy repo's `ytdlp` role **before the v0.5.0 promotion** (delete-and-relaunch reinstates curl_cffi → 100% fetch failure), and `--impersonate` argv machinery stays evidence-gated
- WAF-incident 2026-08-10: classification patterns for `Unexpected response from webpage request` and `Unsupported URL` (both retryable — WAF refusals, not video death); evidence is the dumped block page → with the next classification-table edit; operator-editable TOML, no release needed
- WAF-incident 2026-08-10: **two-form `source_url` corpus** — new artifacts carry `@x` URLs, the existing 880,387 carry share URLs (ADR-0010 provenance field); lossless and reconstructible from the PK, but researcher-visible → PI corpus note at analysis handoff (normalisation retired 2026-08-12: ADR-0049 forbids provenance rewrites and fetch no longer reads the stored form)
- 403-incident 2026-08-06: yt-dlp version drift vs 0033 pattern pins (fleet runs 2026.06.09/2026.07.04 against a 2026.03.17 pin; no observed mismatch) → any yt-dlp upgrade/reinstall, or unexplained `YtDlpOther` growth. **2026-08-11: 2026.07.04 is the latest stable (`pip install -U` is a no-op), and upstream reports 2026.03.17 unaffected by the WAF block — a downgrade would both fix and de-drift, untested**
- v0.5.0 batches 2026-08-13: **yt-dlp internal retry count uncapped** (default 10 → ~3.5-min worker stalls on connect timeouts; 3/1,458 claims) → next release touching fetcher argv: `--retries <small N>` in `build_yt_dlp_args`, never via config file
- v0.5.0 validation 2026-08-13: **silent pre-batch startup phases** — `.work` temp sweep unbounded against mass-failure residue (verified: ~25 min enumerating ~1.5–2M incident-era dirs, zero log output) + suspected unindexed startup table scan (hypothesis, verify with EXPLAIN) → next startup-touching epic; phase log lines are the cheap half
- WAF-incident 2026-08-10: **hourly census check** (zero successes, or nonzero `YtDlpOther`, in the last hour) — the alarm was pre-registered after incident 1 and never added; the breaker half of the detection gap landed in v0.5.0 (ADR-0050, archived), this check is the remaining tell-a-human half → before the next unattended stretch
- Full production-run entries: [followups/production-run.md](followups/production-run.md)

**Plan C (short-link resolution, multi-engine, storage scale)**
- T5: `SHORT_LINK_RE` query parameters → Plan C (short-link resolution lands)
- T8: `output::shard` ASCII-only byte slice → Plan C (when `VideoId` newtype lands)
- T1-Epic1: Promote 0010's pass-through rule to a meta-process ADR → Plan C (if recurring pressure)
- T3-Epic1: `decode_wav` trusts float-format WAV sample values → Plan C (if alternate fetcher introduces float WAVs)
- T10-Epic1: Per-token text field doubles raw_signals payload → Plan C (compact JSON landed in perf-tweaks decdf6f; drop-text still deferred pending 0010 amendment)
- T11 (split at Epic 3 close): yt-dlp argv `--` separator before `source_url` → Plan C (when resolved URLs reach the fetcher)
- Epic 3 final review: `scrub_cookie_path` canonicalized/relative path-variant hardening → Plan C (multi-engine work)
- Transcript-storage assessment: DB-at-runtime transcript storage (schema v4 + export subcommand + sync redesign; own epic) only if the ADR-0004 ~1M-small-files ceiling approaches or SQL-queryable transcripts become a research need → Plan C (storage scale)
- T1-Epic1: codex ADR-refinement bullets gated on multi-engine / CUDA-fallback work (0009 fallback Engine API, 0016 multi-engine GPU memory, error-variant enumeration) → Plan C (re-routed from cross-epic 2026-07-30; the entry's other three bullets are archived)
- v0.5.0 Task 03 (parked): `claim_next`'s doc comment states the 19-digit id guarantee unconditionally, but the v7 migration guard enforces it only for `canonical = 1` rows (latent — all pending rows are canonical today) → Plan C (short-link resolution lands non-canonical rows)
- Full Plan C entries: [followups/plan-c.md](followups/plan-c.md)

**Cross-epic / ADR maintenance / verify-then-archive**
- Epic 5b T09: `cleanup_after_success` removes the attempt dir while the store mutex is held — bounded and best-effort (ADR-0047 class b), but needless lock-hold time; moving it touches ADR-0008's locked half, so it needs its own change → unscoped (next epic touching artifact-write/mark ordering, or contention evidence)
- T9-Epic1: integration test only exercises empty-segment path on silence fixture → unscoped (when spoken-English fixture lands)
- T7-Epic1: Revisit `SamplingStrategy::Greedy { best_of }` after T13 bake → unscoped tuning followup (see also `bake-findings.md`)
- T8-Epic1: Diagnostic log when `lang_detect`'s top id disagrees with primary inference → unscoped diagnostic (see also `bake-findings.md`)
- T08-arch-docs: architecture doc-set drift detection → standing maintenance (revise matching deepdive + index.md §4 at each epic's planning time if it touches a covered surface)
- Full cross-epic entries: [followups/cross-epic.md](followups/cross-epic.md)

# Census-completion strategy — design (2026-08-12)

**Status: DRAFT for operator review.** Product of the 2026-08-12 holistic
options session following the two WAF incidents. Companion evidence:
`docs/operations/incident-2026-08-06-tiktok-tls-403.md` and
`docs/operations/incident-2026-08-10-tiktok-waf-impersonation-block.md`
(the latter on branch `docs/incident-2026-08-10-waf-impersonation-block`
at time of writing), upstream
[yt-dlp#17403](https://github.com/yt-dlp/yt-dlp/issues/17403).

## 1. Problem

Two TikTok/Akamai WAF rule deployments in five days each stopped the
campaign's fetching cold (Aug 6: python TLS ClientHello 403'd on the share
host; Aug 10: curl_cffi impersonation fingerprints served a block page
globally). The yt-dlp user base is a fingerprint monoculture, so every
community workaround self-extinguishes as the herd adopts it. ~1.93M of
2.98M unique videos remain unfetched; the VM is paused mid-remediation
(backup taken, impersonation config retired, curl_cffi uninstalled, **the
incident-2 SQL URL-rewrite NOT applied**). The question this design
answers: how hard to push for census completion, with what ordering, what
transport, and what engineering investment.

## 2. Evidence base (measured 2026-08-12 against the 2026-08-11 snapshot)

**Corpus state:** pending 1,908,746 (57,352 @ attempt 0 + 1,851,394 @ 1),
succeeded 880,387, failed_terminal 173,404, failed_retryable 19,920.
`max(attempt_count) = 1` corpus-wide — nothing near the cap.

**Removal dynamics** (creation time decoded from the video_id snowflake —
validated corpus-wide: 0 of 4.58M watch events precede their video's
decoded creation):

- Cross-section: removed-rate ~2–3% for 2020–2024 cohorts, 9.8% for 2025,
  16.7% for 2026 (peak ~18.5% at ~5 months of age). Old videos in these
  histories are survivors; new videos carry a front-loaded hazard.
- Watched-fresh subset (first watch ≤7 days after creation, n≈1.79M):
  removal rises monotonically ~14.9% (1 month old) → ~19.8% (5–10 months),
  then plateaus. **~15% of removal happens in the first month; ~20% by six
  months; near zero after.** The observed age-gradient also runs opposite
  to what DDP-export exclusion of removed videos would predict, so the
  export evidently includes pre-donation removals.
- Marginal cost of delay for the pending pool: ~0.7–0.9 pp/month on young
  cohorts. Real, linear, not a cliff.
- **Census yield forecast: ~1.61M fetchable of the 1.93M remaining
  (83.4%)**, weighting stratum pending counts by observed alive-rates.

**Base rate (shallow keyword study, US panel, EN+ES tiers):** 27,425 of
880,392 transcripts (3.1%) hit the strict tier; eyeball precision ~70–75%
(FPs dominated by "shooting" as filming/basketball) → true rate ≈ 2.2–2.5%,
a floor (overlay/music-only content is invisible to transcripts).
Policing-discourse terms (ACAB, blue lives, …) add only 19 videos beyond
the strict tier. Hit rate by creation stratum rises ~2.7% (oldest) →
~3.3–3.6% (newest) — consistent with crime/policing content being removed
at a ~20–30% relatively higher rate, observationally confounded with a
secular content trend (methods note must carry both readings).

**Per-participant (the power-relevant number):** at today's 40% mean
coverage, median participant has **178** strict-tier hits (p25 = 18,
p75 = 579, max 3,296); 25 of 125 participants under 10. Census (~83%
achievable coverage) roughly doubles these. **Open check:** `watch_history`
has 125 distinct respondents vs. 144 ingested donors — confirm whether 19
donations carried empty/invalid histories before per-participant analyses.

**Strategic conclusion the numbers force:** the study is not data-starved —
the WAF arms race threatens precision and the low-exposure tail, not
viability. Push steadily on the cheap path; no heroics (browser-fetcher,
proxy pools) unless the cheap path dies; pre-test fallbacks instead of
inventing them mid-outage.

## 3. Decisions

### D1 — Publication-recency claim order

`claim_next` orders `attempt_count ASC` (retry fairness, unchanged), then
**`video_id` numeric DESC** (snowflake ⇒ newest-created first), replacing
`first_seen_at` (ingest fairness has no remaining purpose). Rationale,
operator-ratified 2026-08-12:

- **Truncation semantics:** an interrupted census is a *complete corpus of
  all videos created after a cutoff date* — statable in one sentence,
  defensible across the broadest range of future analyses, and it converts
  the never-made PI window decision into an empirical outcome.
- **Attrition-minimizing:** the newest cohort is the perishable stock
  (§2); oldest videos are survivors with a spent hazard.
- **Bias posture:** concentrates observation where removal-censoring is
  provably smallest; the residual recency confound (news cycles, content
  trends) is a documented frame, not a hidden distortion.

Rejected alternatives: participant-complete order (cluster-sample
semantics; power dies at small participant counts; soft frequency bias via
shared videos), frequency-first (size-biased against exactly the
tail-heavy content of interest), uniform random (safest generic choice but
gives up the window-census property and the attrition win).

### D2 — Claim-time canonical fetch-URL derivation (no DB rewrite)

The fetcher derives the URL it gives yt-dlp from the primary key at fetch
time — `https://www.tiktok.com/@x/video/<video_id>/` for `canonical = 1`
rows — instead of fetching the stored `source_url`. **The incident-2
handoff's 1.93M-row `UPDATE` is superseded and must not run.** `source_url`
remains pristine DDP provenance in the DB and in every artifact (ADR-0010 /
ADR-0042 posture); the URL *form* becomes a versioned transport concern,
visible in `params_json`, reversible by release. `backfill-metadata`'s
`build_metadata_only_args` uses the same derivation (it reads succeeded
rows whose stored share-URLs 403 under the current gate). Non-canonical
rows (Plan C short-links) keep current behavior. The `@x` placeholder is
the incident-2-proven form (fetches identically to any username;
non-empty segment keeps clear of `CANONICAL_RE`'s empty-user gap).

### D3 — Ship it as a release; promote per ADR-0043

The VM (pinned v0.3.0) is upgraded by cutting **v0.5.0** from main
(v0.4.0 + this design's changes) and following the tag-and-relaunch
promotion. No hand-built binaries, no config-file side effects. Schema
compatibility: v0.3.0 and v0.4.0 share schema v6; D1's index change bumps
to v7 through the ADR-0022 migration ladder. The upgrade also delivers the
v0.3.2–v0.4.0 operational stack the campaign has run without (checkpoint
hook — retiring the cron stopgap; `swept_stale` forensics; per-attempt dir
cleanup + startup sweep; real hostnames in `worker_id`; ADR-0013 assertion
firing on CUDA builds).

### D4 — Minimal mass-failure circuit breaker

Run-global consecutive-no-success counter across claim outcomes, reset on
any success. At threshold (default **50**, operator-ratified 2026-08-12;
operator flag `--breaker-threshold`, `0` disables): cancel the ADR-0025
token → workers finish their current row's state write and drain → census
written with the breaker verdict → distinct exit code (**4**). Properties:

- False-trip probability at the campaign's ~77% success rate is
  0.23⁵⁰ ≈ 10⁻³²; even a pure dead-video run cannot plausibly trip it
  (0.2⁵⁰ ≈ 10⁻³⁵). A WAF wave at observed rates trips in seconds, ~50
  attempts burned instead of incident 1's 1.8M.
- DB-visible per the standing operator ruling: the trip lands in
  `batch_runs` census (`breaker_tripped`, plus the failing-label census
  the run already keeps) — adjudicable from the state DB alone, not from
  scrollback.
- Interactions: cancellation is the existing supervised token (0025);
  ADR-0026's drain-on-None is untouched; the 0044 checkpoint task's
  join-count gating is unchanged (the breaker cancels the same token the
  clean-drain path does).

### D5 — Transport observability (kill the invisible states)

Incident 2 left correct behavior depending on a package absence, a data
column's URL form, and a deleted config file — none visible anywhere.
v0.5.0 records in `params_json`: the fetch-URL derivation form
(`fetch_url_form: "canonical-v1"`), and a startup environment echo — yt-dlp
version and impersonation-target availability, captured once via the
bounded subprocess runner (ADR-0021) at process start. "Am I
impersonating?" and "which URL form am I fetching?" become answerable from
`batch_runs`, the same reasoning as ADR-0013's GPU assertion.

### D6 — Operating discipline for the remainder

- **Resume sequence:** promote v0.5.0 → validation batch
  `--max-videos 50 --retries 2` (expect ≥ ~70% success, zero `HttpError` /
  `YtDlpOther`, envelopes captured) → `--max-videos 10000` rate
  measurement → capped batches as routine (bounded unattended blast
  radius), `--retries 2` floor throughout.
- **Detection:** hourly census check (zero successes in the last hour, or
  any `YtDlpOther` growth — the pre-registered alarm incident 2 validated).
  Implementation is deploy-repo/VM-side (cron + notify against the state
  DB); this repo's contribution is that the census is queryable, which it
  already is.
- **Fallback ladder, pre-tested not improvised** (upstream-sourced, each
  verified by the incident-2 probe-matrix procedure before it's needed):
  (1) UA-override to a current Chrome string; (2) yt-dlp downgrade to
  2026.03.17 — which is the version ADR-0033's patterns are pinned to, so
  it *closes* the version-drift followup while active; (3) only then the
  heavy options (real-browser fetcher, egress diversity) with their own
  design round.
- **Budget frame:** ~1.61M expected fetchable at the measured ~44k/day ≈
  **37 fetch-days plus WAF weather**. Timeline slip costs ~0.7–0.9 pp/month
  of the youngest cohorts — linear, priced, not panic-grade.

## 4. Component design notes

- **Fetch-URL derivation** lives beside the existing per-claim policy
  composition (`cookie_opts_for` / `format_policy_for`,
  `src/pipeline/mod.rs`) so all per-claim fetch decisions stay at one
  altitude; the fetcher receives the derived URL as its `source_url`
  argument today (no `YtDlpFetcher` signature change). ADR-0038's
  download-first selector already carries the canonical-URL path (proven
  in the incident-2 smoke: `download` format offered unimpersonated).
- **Claim order**: new partial index
  `(status, attempt_count, video_id DESC) WHERE status='pending'`
  replacing `idx_videos_pending_v3`; `claim_next`'s ORDER BY matches. The
  19-digit uniformity assumption (lexicographic = numeric) is asserted by
  the v7 migration (`MIN(LENGTH(video_id)) = MAX(LENGTH(video_id)) = 19`
  over canonical rows) — fail the migration, not the claim, if violated.
- **Breaker counter** is an `Arc`-shared atomic beside the existing
  `max_videos` cap atomic (same claim-guard locking altitude); the count
  of "claims resolved without success" increments in the failure dispatch
  paths and resets in `mark_after_artifacts`' caller. Trip = same
  `token.cancel()` the supervision loop owns; exit-code mapping in
  `commands::dispatch`.
- **ADRs to author during implementation** (via `write-lean-adr`, per
  repo governance): the fetch-URL-derivation decision (transport is code,
  provenance is data — supersedes the incident-2 handoff SQL), the claim-
  order decision (D1's rationale is an architecture-grade commitment), and
  the breaker (thresholds, semantics, why it never writes video state).
  Candidate amendments: 0026 (breaker coexists with drain-on-None), the
  incident-2 doc's "remaining steps" section (mark superseded by D2).

## 5. Testing

Per ADR-0003 (batch test-first for plan-prescribed code): derivation unit
tests (canonical/non-canonical/backfill paths, artifact `source_url`
untouched), claim-order integration test (mixed-attempt, mixed-age fixture
pool claims in `attempt ASC, video_id DESC`), migration test (v6→v7 ladder
+ length assertion), breaker integration tests on the pipeline fakes
(trips at threshold, resets on success, clean drain + census + exit 4,
disabled at 0). Full gate before promotion:
`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
--features test-helpers -- --test-threads=1 && cargo build --release`,
plus the staged validation batches on the VM (D6).

## 6. Methodology record (carry into the PI write-up)

- Ordering statement: "fetching proceeded newest-publication-first;
  the achieved set is a complete corpus of watched videos created after
  <cutoff>" + the D1 bias discussion.
- Removal hazard: front-loaded (~15% month one, ~20% by six months,
  plateau); donation-to-fetch latency is a design parameter for future
  campaigns (fetch recency-first from day one).
- Censoring gradient: strict-tier hit rate 2.7% → 3.3–3.6% (old → new
  strata); both readings (differential removal vs. secular trend).
- The 125-vs-144 respondent discrepancy (open check, §2).
- Transcript-based exposure is a floor (speech-only; overlays/music
  invisible).

## 7. Out of scope (deliberately)

- `--impersonate` flag machinery (not impersonating; the transport ADR
  defines the extension point when evidence demands it).
- Browser-based fetcher, proxy/egress diversity (contingency tier 3 —
  own design round if ever triggered).
- The cookied mop-up (ADR-0048 / `docs/operations/cookied-runs.md` —
  unchanged by this design; gated cohort untouched at attempt 1, its
  §2 retry arithmetic re-derives from the final DB as written).
- `video_metadata_raw` prune, PI window decision, Plan C items.

## 8. Open questions for review

1. ~~Breaker default threshold~~ — resolved 2026-08-12: operator set the
   default to **50** (trip in seconds at wave rates; false-trip probability
   ≈ 10⁻³² at the campaign's success rate).
2. Version: v0.5.0 proposed (feature release from v0.4.0 main) — accept?
3. D1 keeps `attempt_count ASC` precedence, so the 57k attempt-0 rows
   (scattered ages) claim before the 1.85M attempt-1 wave rows on the
   first post-upgrade run — acceptable one-time deviation from pure
   recency (~1.3 days at measured rate), or should the migration reset the
   wave rows' attempt counts (touches ADR-0046 territory — default-deny)?
   Proposal: accept the deviation; do not touch attempt counts.
4. Hourly alarm delivery mechanism (VM cron + mail? push?) — deploy-repo
   handoff detail, operator's choice.

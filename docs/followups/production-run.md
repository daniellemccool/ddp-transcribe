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
`e73e2f0`). The 4-download-worker A/B has since been **measured** too
(commit `9e43211`): scaling is linear at 1.32× observed vs 1.33× predicted,
with no throttle signature — failure mixes statistically identical across
instances, and ~4,500 claims/h from one IP drew no visible pushback. An
operator disclosure that both instances shared one A10 during the A/B window
(commit `a839ac7`) leaves the A/B itself standing — same GPU and same IP on
both sides makes worker count the only variable — but corrected the ceiling
math, which had assumed a GPU each; the action is to pin one instance per GPU
at the next restart. The PI write-up is unblocked. Status: **HOLD** pending
only the PI's window decision. Archive this entry once the PI summary ships.

**2026-08-13 clarification:** the 1.32× is download-worker scaling only —
**dual-GPU throughput has never been measured** (the A/B's instances shared
one A10). The first two-GPU round of the v0.5.0 campaign is the dual
measurement: sum both censuses over wall clock (runbook resume-sequence
step 2 note).

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

**Instrumented in v0.3.2 (`31c18df`):** every row the stale-claim sweep
recovers now writes a `swept_stale` `video_events` row carrying the stale
claim's provenance (`was_claimed_by`, `claimed_at`, `threshold_secs`), and
instances report their real hostname in `worker_id` instead of the literal
`host`, so the two GPU runs are distinguishable without pid archaeology.
**Next occurrence of a pending-count bump:** pull the snapshot and check for
`swept_stale` events matching the affected rows and timestamps — present = the
sweep did it (expected behavior, ADR-0024's blind revert); absent = evidence of
writer loss, at which point this entry graduates from hypothesis to a fix task.
The instrumentation changed no predicate and no status semantics, so it neither
confirms nor refutes the hypothesis on its own.

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
with the concurrent-writer instrumentation work above — treat the two
entries as one two-writer anomaly cluster rather than two separate bugs.

**Instrumented in v0.3.2 (`31c18df`)** along with the concurrent-writer entry:
the same `swept_stale` events and real hostnames apply here, since a capped run
whose rows were swept back to `pending` and re-claimed is one of the candidate
explanations for an inflated `claimed` count. Same adjudication rule — matching
events = sweep, none = writer loss.

---

### `--impersonate` belongs in `build_yt_dlp_args` (with its own ADR)

**Found in:** the 2026-08-06 TLS-fingerprint 403 incident
(`docs/operations/incident-2026-08-06-tiktok-tls-403.md`) — TikTok's edge
began rejecting non-browser TLS fingerprints on `www.tiktokv.com`, killing
every fetch's unimpersonated first hop; 1.81M rows burned one attempt over
60 unattended hours.
**Disposition:** The live mitigation is deliberately ops-level: a yt-dlp
user config (`~/.config/yt-dlp/config` → `--impersonate chrome`) on the
campaign VM, installed by the deploy repo's `ytdlp` role, working because
the pipeline never passes `--ignore-config`. That keeps the pinned v0.3.0
binary untouched mid-campaign (ADR-0043), but it is config-by-side-effect:
invisible in `batch_runs.params_json`, invisible in the argv the subprocess
layer logs, and silently absent on any host where the file is missing.
**Trigger to revisit:** the next pipeline release that touches the fetcher
(or Plan C multi-engine work, whichever first).

The proper fix is `--impersonate <target>` as an explicit member of
`build_yt_dlp_args` (`src/fetcher/ytdlp.rs`), with an ADR settling: whether
it is unconditional or flag-gated, what happens on hosts without curl_cffi
(yt-dlp hard-errors on `--impersonate` with no target available — the
desktop currently has none installed), and whether the target string is
operator-configurable. Note the interaction with the deploy role's
provisioning check ("Verify yt-dlp impersonation targets are available"),
which currently verifies a capability nothing in the pipeline explicitly
uses. When the argv change lands, retire the config file in the same deploy
change — two sources of the same flag is drift waiting to happen.

---

### yt-dlp version drift vs the classification-pattern pins

**Found in:** the same incident's probe work. ADR-0033 pins the table's
patterns to yt-dlp 2026.03.17 stderr; the deploy role installs yt-dlp
unpinned at provisioning time (`pipx install`, `creates:` guard, never
upgraded). Live fleet observed 2026-08-09: campaign VM 2026.07.04, old VM
2026.06.09 — three versions in play including the dev pin, none chosen on
purpose.
**Disposition:** No observed mismatch — the campaign classified correctly
throughout, and the incident's `HTTP Error` messages matched the table on
both VM versions. This is drift *exposure*, not drift damage.
**Trigger to revisit:** any yt-dlp upgrade or reinstall on a fetch host, or
the first unexplained growth of `YtDlpOther` (the fallback catching what a
drifted message no longer matches — cheap census query, worth adding to the
operator's periodic checks).

Options when triggered, in ascending effort: re-verify the table against
the new version's stderr corpus (0033's own procedure); pin the version in
the deploy role so drift becomes a deliberate act; both.

**2026-08-19 — the trigger FIRED:** the campaign VM was upgraded to
nightly 2026.08.18.122307 (the 2026-08-18 header-fingerprint incident's
remedy — see `../operations/incident-2026-08-18-tiktok-header-fingerprint-block.md`),
a ~13-month jump past the 2026.03.17 pattern corpus. First-order
verification passed the same day: `IpBlockedMessage` stderr text is
byte-identical on the nightly (probe), and the 50-video validation census
classified 8/8 terminals correctly with zero `YtDlpOther`. Standing watch:
`YtDlpOther` share in the full-cap censuses. The deploy-role pin option is
now mandatory rather than optional — the role must install ≥ nightly
2026.08.18 (stable 2026.07.04 cannot fetch TikTok at all post-rollout);
tracked with the curl_cffi removal in the deploy repo.

---

### `swept_stale` event/recovered-set invariant is enforced only in debug builds

**Found in:** Epic 5a Task 03 review (2026-07-30), parked by operator ruling
the same day. Filed here rather than under an epic because its trigger is the
next campaign, not a code sweep.
**Disposition:** Deferred to the next campaign (~2 months out). The sweep
gathers its forensic rows with a SELECT that repeats the UPDATE's predicate
verbatim inside one IMMEDIATE transaction, and a `debug_assert_eq!` pins
"event set == recovered set". `debug_assert` compiles out of `--release`, which
is the only build the campaign machine ever runs — so in production the
invariant has **no runtime enforcement**. If a future edit de-syncs the two
predicates, the sweep would keep reporting a recovered count while silently
emitting a different set of events, and the forensic trail would lie precisely
when it is being trusted to adjudicate a two-writer anomaly.
**Operator ruling (2026-07-29):** do not harden mid-campaign; any fix should
prefer a **DB-visible signal** (e.g. recording the mismatch where a query can
find it) over a `tracing::warn!` — the operator runs in tmux and cannot scroll
back, so log warnings are operationally invisible.
**Trigger to revisit:** next-campaign hardening pass, or any edit to
`sweep_stale_claims`' SELECT/UPDATE predicate pair (ADR-0024's Guidance already
rejects adding a condition to either side).

---

### Fetch-URL construction belongs in `build_yt_dlp_args` (with its own ADR)

**Supersedes** the incident-1 entry "`--impersonate` belongs in
`build_yt_dlp_args`". That entry assumed impersonation was the fix; the
2026-08-10 WAF incident
(`docs/operations/incident-2026-08-10-tiktok-waf-impersonation-block.md`)
established it is the *failure*. Both decisions are now one ADR.

**Found in:** the 2026-08-10 Akamai WAF block. TikTok serves a 537-byte
"Site Maintenance" page with HTTP 200 to clients presenting curl_cffi's Chrome
TLS fingerprint. The remedy is the canonical
`www.tiktok.com/@<user>/video/<id>/` URL fetched **unimpersonated** — proven
end-to-end through the pinned v0.3.0 binary (3 claimed / 2 succeeded with valid
16 kHz mono WAVs and metadata envelopes / 1 correctly-classed
`IpBlockedMessage`).

**Disposition:** applied as **two ops-level changes**, both invisible to the
code and to `batch_runs.params_json`:

1. curl_cffi uninstalled from the pipx yt-dlp venv — the only lever, because
   `impersonate=True` is hardcoded in yt-dlp's TikTok extractor
   (`_extract_web_data_and_status`). No CLI flag disables it.
2. `videos.source_url` rewritten to `https://www.tiktok.com/@x/video/<id>/`
   for `pending`/`failed_retryable`/`in_progress` rows (1,928,670 rows;
   rehearsed on a snapshot copy: 6m49s, `integrity_check ok`, +737 KB,
   succeeded/terminal rows untouched).

The proper fix builds the URL from `video_id` inside `build_yt_dlp_args`
(`src/fetcher/ytdlp.rs:97`), leaving `source_url` as pure provenance, and
retires the DB rewrite in the same change. The ADR must settle: whether the
canonical form is unconditional or policy-selected; what username segment to
emit (must be non-empty — `@/video/<id>` fetches but fails `CANONICAL_RE` in
`src/canonical.rs:25`, classifying `Invalid`); whether impersonation is
explicitly *disabled* in argv rather than by dependency absence; and what
run-visible witness proves which mode is active (ADR-0013's precedent: an
unverifiable claim about the backend is worthless).

**Trigger to revisit:** **before the next ADR-0043 promotion.** 0043's step 5
is delete-and-relaunch, which reinstates curl_cffi from the deploy repo's
`ytdlp` role and returns the machine to 100% fetch failure — so the promotion
itself becomes the outage unless the deploy repo is fixed first. Also blocks
`backfill-metadata` (it reads `source_url` on `succeeded` rows, which were
deliberately left on the 403-gated share form).

> **2026-08-12 (v0.5.0): URL-derivation half RESOLVED.** ADR-0049 ("Derive
> the fetch URL, never rewrite provenance") landed the pipeline-side fix —
> the canonical `@x` URL is derived from `video_id` at claim time
> (`canonical::derived_fetch_url`, used by both pipeline paths and
> `backfill-metadata`), `videos.source_url` is immutable provenance, and the
> DB-rewrite remedy is superseded (must never run again). The ADR also
> settled the witness question: `params_json` now records `fetch_url_form`
> and `ytdlp_impersonation_available` (Task 09 env echo). The
> `backfill-metadata` blocker above is resolved the same way — it derives
> too, so share-form `succeeded` rows no longer matter.
>
> **What remains open in THIS entry, after v0.5.0:**
> 1. **The deploy-repo trap (the live trigger — unchanged and urgent):** the
>    `ytdlp` role still installs curl_cffi, and yt-dlp's TikTok extractor
>    hardcodes `impersonate=True` when it is present — so a 0043
>    delete-and-relaunch still re-breaks fetching regardless of the v0.5.0
>    code. Fix `~/src/d3i-infra/researchcloud-ddp-transcribe` **before** the
>    v0.5.0 promotion. Post-relaunch witness: `ytdlp_impersonation_available`
>    must be `false` in the validation batch's `params_json`.
> 2. **Explicit `--impersonate` flag machinery in `build_yt_dlp_args`:**
>    still open and still **evidence-gated** — impersonation is currently the
>    failure, not the fix (this incident), so the machinery waits for
>    evidence it is ever needed again.

---

### Classification patterns for the two WAF-refusal messages

**Found in:** the 2026-08-10 WAF incident. 1,360 rows in ~21 minutes fell to
the `YtDlpOther` catch-all: 1,351 × `Unsupported URL:
https://www.tiktok.com/share/video/<id>/` (the *generic* extractor's message
when the redirect chain leaves it holding the middle hop) and 9 ×
`Unexpected response from webpage request` (the *TikTok* extractor's message
when handed the block page directly). Same event, two extractors.

**Disposition:** the catch-all's `retryable` disposition was **correct** —
these are refusals, not video death, and the whole wave is recoverable. So
this is an observability fix, not a behavioral one: labelling them means a
recurrence shows up in `status` instead of hiding in `YtDlpOther`. Evidence
meets ADR-0033's bar (the dumped block page is in the incident record), and
per ADR-0037 the table is operator-editable TOML — a `--classification` file
needs no release.

Keep `YtDlpOther` monitored regardless: the reason both incidents were
survivable is that an unrecognised message defaults to retryable, and the
reason both went unnoticed is that the same default is silent.

**Trigger to revisit:** next classification-table edit, or the first
recurrence.

---

### Two-form `source_url` corpus (researcher-visible provenance)

**Found in:** the 2026-08-10 remedy. `source_url` is a provenance field
written into every transcript JSON artifact (ADR-0010,
`src/output/artifacts.rs:50`). After the rewrite, new artifacts carry
`https://www.tiktok.com/@x/video/<id>/` while the existing 880,387 carry
`https://www.tiktokv.com/share/video/<id>/`.

**Disposition:** accepted deliberately, not stumbled into. It is lossless —
the 2026-08-11 snapshot census confirmed all 2,982,461 rows are
`canonical = 1`, all form-1, zero query strings, every URL containing its own
`video_id`, so either form is exactly reconstructible from the primary key,
and the DDP originals remain in Yoda. The rewrite was scoped to unfetched rows
specifically to leave completed provenance untouched.

**Trigger to revisit:** analysis handoff (flag it to the PI as a corpus note).
~~or normalise when the pipeline-side fix lands~~ **2026-08-12: the
normalisation option is retired** — ADR-0049 makes `source_url` immutable
provenance (bulk rewrites are exactly what it forbids), and since v0.5.0
derives the fetch URL from `video_id`, the stored form no longer affects
fetching. The two-form corpus is permanent; the only remaining action is the
PI corpus note.

---

### v0.5.0 startup: silent pre-batch phases (unbounded `.work` sweep + suspected unindexed scan)

**Found in:** the 2026-08-13 v0.5.0 VM validation — the first `process` start
after the in-place tag promotion sat **silent for ~25 minutes** after
`config resolved`, with no way to tell working from wedged except `/proc`.

**Verified half:** the kernel stack showed `getdents64`/`ext4_readdir`
against `~/ddp-work/transcripts/.work` — the startup temp sweep enumerating
incident-era residue: one `ytdlp-<id>` work dir per failed attempt from the
Aug 6–11 outages (>65k subdirs by ext4's nlink=1 signal; 146 MB directory
inode ≈ 1.5–2M entries) on a ~35 MB/s disk. Remedy applied live: `mv .work`
aside (instant rename), delete offline; the restart reached the GPU banner
in 4 s. The sweep cost recurs after any future mass-failure stretch — that
is exactly when a restart happens.

**Hypothesis (unverified):** the ~3.4 GB of sequential reads *before* the
directory walk was a full-table scan of the state DB during startup —
suspect a `status = 'in_progress'` predicate (stale-claim sweep) that no
index covers (the pending partial index is `WHERE status='pending'`).
Verify with `EXPLAIN QUERY PLAN` before acting; cold-cache disk speed may
be the whole story.

**Fix candidates, cheapest first:** log lines bracketing each startup phase
(the silence, not the time, was the operator cost — the existing
`start-of-batch sweep complete` line shows the idiom); progress-log every N
entries in the temp sweep; if the scan hypothesis verifies, weigh an index
against accepting the cold-cache cost.

**Trigger to revisit:** next epic touching worker/startup code, or before
the next long unattended campaign stretch.

---

### Hourly census check (the alarm that was filed and never installed)

**Found in:** the 2026-08-10 WAF incident — and, verbatim, in incident 1's own
followups five days earlier, which named "the first unexplained growth of
`YtDlpOther`" as a trigger and called it "a cheap census query, worth adding
to the operator's periodic checks."

**Disposition:** it was never added, and incident 2 ran undetected until the
operator happened to look. That it cost ~7 minutes rather than incident 1's
60 hours was luck, not instrumentation. The check is two queries against the
state DB: successes in the last hour (zero = stop), and `YtDlpOther` in the
last hour (nonzero = investigate). Per the standing operator ruling, prefer a
DB-visible signal over a log warning.

This is the cheap half of the detection gap; the mass-instant-failure circuit
breaker is the durable half. Neither substitutes for the other — the breaker
stops the burn, the check tells a human. **2026-08-12: the breaker half
landed in v0.5.0** (ADR-0050, `--breaker-threshold` default 50, exit code 4,
`breaker_tripped` in the census — archived in
`../archive/followups-resolved.md`). This check is now the only missing half.

**Trigger to revisit:** before the next unattended stretch. Until it exists,
the operator is the circuit breaker for anything the streak counter can't see
(e.g. a partial-failure mix below the trip threshold).

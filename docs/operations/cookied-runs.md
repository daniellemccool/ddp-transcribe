# Cookied runs — desktop mop-up runbook (`SensitiveLoginGated`)

**Stage: ACTIVE** (written 2026-08-04 during the campaign; refreshed
2026-08-30 at campaign close). The §6 trigger has FIRED: the fetch queue
drained 2026-08-30 (2,446,943/2,982,461 transcribed), the final sync landed
on Yoda, and the VM is paused. The final snapshot was pulled to the desktop
and every DB-derived number below was re-verified read-only against it
(2026-08-30, schema v7, 5.4 GB). Remaining gates before the first batch:
transport-stack install + rebuilt rehearsal (§4) and the operator's
batch-calculus re-ruling (§5). Read with ADR-0035 (cookie scoping —
invariant), ADR-0036
(retry authority), ADR-0046 (requeue-failures), ADR-0033 (evidence-derived
classification), and the README's `requeue-failures` arithmetic. The
desktop ops-model revision (writer role moving from the VM to the desktop
at campaign end) is recorded as the "Run cookied mop-up batches supervised
on the desktop" invariant (`docs/decisions/0051-*`, accepted 2026-08-10;
re-authored 2026-08-30 under a fresh number after colliding with the
v0.5.0 claim-order record's 0048).

Operator decision (2026-08-03): cookied runs happen on the **desktop,
supervised, in small batches — never on the VM**. A dead jar fails silently
by design (0035: gated claims just re-park), so the only detection mechanism
is a human watching the per-batch success rate; that is natural locally and
impractical on the VM. Side benefit: the session credential never touches
shared infrastructure.

## 1. Cohort facts (2026-08-03 Yoda snapshot, verified read-only 2026-08-04)

- `SensitiveLoginGated`, `failed_retryable`: **10,206 rows, every one at
  `attempt_count = 1`** (33× the ~300 that 0035's exposure calculus assumed).
- Other parked retryables: `Fetch` × 239, also all at `attempt_count = 1`.
  **These are gated-cohort members with a legacy placeholder kind** (verified
  2026-08-10): their stored messages carry the "This post may not be
  comfortable…" gate text, and the sweep classifies from the *message*, not
  the stored kind — so they adjudicate requires-cookie, and a cookied sweep
  requeues them with the kind normalized to `SensitiveLoginGated`
  (`batch.rs` preserve-kind-on-fallback logic). Effective gated pool =
  `SensitiveLoginGated` + `Fetch` counts.
- Terminal residue: **none that is gating-shaped.** The only terminal
  reasons are the three probe-confirmed dead/region-locked classes
  (IpBlockedMessage 63,642 / VideoNotAvailable10240 35,486 /
  VideoNotAvailable10231 6,630 — 2026-08-03 second-egress probe). No
  `requeue-failures --include-terminal` pass is warranted.
- Schema: v0.3.0 and v0.4.0 share schema v6 — Yoda snapshots open in the
  local binary with no `migrate`. *(HISTORICAL — the final DB is schema v7;
  see the 2026-08-30 addendum.)*

**2026-08-09 addendum (post-403-incident refresh —
`incident-2026-08-06-tiktok-tls-403.md`):** the gated cohort has grown to
**18,461 rows, still every one at `attempt_count = 1`**, so §2's
`--retries 2` floor still holds — but re-derive from the final snapshot as
§2 already requires. The incident did not touch the parked pool: the
2026-08-09 recovery sweep (cookie-less, `--retries 2`) examined 18,700 and
parked all of them (`requeued_for_retry 0`, `kept_capped 0`). Two
observations against this doc's earlier claims: (a) the `Fetch` rows
(239) **parked rather than requeueing** in that cookie-less sweep,
contrary to §1's expectation — *resolved 2026-08-10*: the sweep classifies
from the stored message, and all 239 messages carry the gate text; they ARE
gated cohort (18,461 + 239 = the 18,700 examined). §1 is corrected; the
sweep behaved correctly on both versions;
(b) the 1.85M wave rows sit in `pending` at attempt 1, ahead of nothing
(claim order `attempt_count ASC` puts the remaining ~134k attempt-0 rows
first), so the trimmed-rehearsal starvation logic in §4 is unchanged.

**2026-08-30 campaign-close addendum** (verified read-only against the
final snapshot on the desktop, 2026-08-30; narrative context in
`~/projects/crime-and-policing/methodology/campaign-close-2026-08-30.md`):

- Final state table at drain (`pending = 0`): succeeded **2,446,943**
  (82.1% of the 2,982,461 corpus), `failed_terminal` 468,450,
  `failed_retryable` 67,063, `in_progress` 5 (stale claims from the final
  runs' last moments — harmless bookkeeping residue).
- The gated pool is **53,051 rows** (52,812 `SensitiveLoginGated` + 239
  legacy-kind `Fetch`, still all at attempt 1 — ~2.8× the 18,700 above)
  and attempt counts are **heterogeneous**: `SensitiveLoginGated` sits at
  attempt 1 × 20,264 / attempt 2 × 32,436 / attempt 3 × 112. Max
  `attempt_count` = 3 → **`--retries 4` is the floor** (§2).
- The rest of the parked pool is NOT cookied cohort: **14,012 rows** at
  attempts 3–4 — a MIXED pool (`NoVideoFormats` 7,206, with live
  audio-less photo posts confirmed 4/4 among its exhaustees;
  `NoPermission` 5,776; `YtDlpOther` 513; `FfprobePostprocess` 334;
  `HttpError` 176; `ToolTimeout` 7 — including the §2 deadline-killer).
  Note `--retries 4` makes this residue sweep-reachable too; in
  `attempt_count ASC` claim order it drains behind the gated tiers.
- Terminal counts (exactly three classes, still none gating-shaped):
  `IpBlockedMessage` 330,772 / `VideoNotAvailable10240` 111,639 /
  `VideoNotAvailable10231` 26,039. Semantics refresh (browser-verified
  spot checks 2026-08-13/26/29/30): `IpBlockedMessage` reads
  **removed-OR-private** (~4/5 of late checks private, not IP blocking);
  `10240` is **format-mixed** (photo-mode posts confirmed among it,
  photo/video split unresolved); `10231` stays region-gated. None of this
  changes the gated cohort — the do-not-relitigate stance below stands,
  on refreshed evidence.
- Schema: the final DB is **v7** (verified) — the desktop binary must be
  v0.5.x lineage. Use the **v0.5.1** tag (cut 2026-08-30; see §6 for why
  v0.5.0 is not acceptable).

## 2. `--retries` arithmetic

Sweep-requeue fires iff `attempt_count < retries + 1`; in-pipeline re-park
after a failed cookied fetch needs the *bumped* count under the same cap.
With the whole cohort at `attempt_count = 1`:

- `--retries 1` requeues the pool, but a failed cookied fetch lands at
  attempt 2 = cap → the row **cap-exhausts** instead of re-parking. One bad
  jar day would strand every claimed row behind `requeue-failures`.
- **`--retries 2` is the floor**: sweep requeues (1 < 3), a failed cookied
  fetch re-parks (2 < 3) and stays sweep-reachable for one more session,
  then caps at attempt 3.

2026-08-30, verified against the final snapshot: **`--retries 2` is STALE
— the floor is now `--retries 4`.** The worked example above only holds
while the cohort's max `attempt_count` is 1; the final gated cohort tops
out at attempt 3 (§1 addendum). The rule: the floor is the smallest
`--retries` that *strictly exceeds* the cohort's max `attempt_count` —
`--retries 4` requeues the whole pool (3 < 5), and a failed cookied fetch
on the attempt-3 rows re-parks (4 < 5) before capping at attempt 5.
Re-derive before each session block: the max climbs as batches run.

Raising `--retries` also un-parks the 14,012-row non-gated residue (§1
addendum), including video `7645028780246895894` — the 2026-08-17
deadline-killer, verified still parked at attempt 3 (`ToolTimeout`, 300 s).
Under v0.5.0 its deadline elapse is misattributed as `Cancelled` and
terminates the whole run; **v0.5.1 (the fix) is therefore mandatory for
every cookied invocation**, rehearsal included (§6).

## 3. Project account + jar procedure

- Create the dedicated NL research TikTok account **early** so it ages
  before the mop-up. Adult birthdate at signup. Let it exist and browse a
  little — a brand-new account fetching exclusively gated content via
  yt-dlp is the anti-bot worst case.
- ⚠ 2026-08-30: the drafted "adult birthdate covers the 18+ tier" premise
  is DEAD in the EEA. TikTok's 2026 age-assurance rollout (DSA) grants the
  18+ tier by *predicted or confirmed* age, not the registered birthdate;
  prediction runs on profile + on-platform behavior, and the confirmation
  flows (Incode/Yoti selfie, government ID, credit card) are offered
  reactively — there is no self-service toggle. Browser-verified
  2026-08-30: the research account does NOT clear the 18+ gate. The
  sensitivity interstitial (the cohort's parking gate text) is a separate
  gate and unaffected. Being pursued operator-side: predicted-age readout
  via an ad's "About this ad" panel, app-side behavior of gated URLs, and
  the privacy-webform objection route. Until resolved, expect the true-18+
  subset of the cohort to fail cookied fetches and re-park (harmless);
  sample-tally its share before investing in verification.
- Verify in a browser that the account clears real gated URLs from the
  parked pool: `~/data/d3i/uu-tiktok/cookied-rehearsal/sample-gated-urls.txt`
  holds 20 randomly sampled cohort URLs. The sensitivity interstitial and
  true age restriction are different gates — confirm both clear.
- Jar export, fresh before every run: log in in a private window, export
  Netscape cookies.txt, close the window **without logging out** (stops
  session rotation under a live browser). `chmod 600`, outside any repo.
- Validate the jar with ONE hand fetch before spending pipeline attempts:

  ```sh
  yt-dlp --cookies <jar> --skip-download <known-gated-url-from-the-sample>
  ```

## 4. Rehearsal (disposable DB copy — NEVER the sync target)

A trimmed rehearsal workspace is prepared at
`~/data/d3i/uu-tiktok/cookied-rehearsal/`: `state.sqlite` is a 2026-08-09
snapshot backup reduced to **exactly the 18,700-row gated cohort**
(18,461 `SensitiveLoginGated` + 239 legacy-kind `Fetch`, per §1; pending,
succeeded, and terminal rows deleted; vacuumed to 26 MB, rebuilt
2026-08-10).
The trim is mandatory. Post-drain (`pending = 0`) the reasons are: (a) it
makes the sweep-census check exact — a trimmed DB must report
`requeued_for_retry` = the cohort count, while a full copy would requeue
the entire parked pool, non-gated residue included; (b) it keeps
rehearsal attempts structurally away from the ~14k non-cookie residue;
(c) a small throwaway file beside a 5.4 GB authoritative one is hard to
mis-target. (The original pre-drain rationale — millions of pending
attempt-0 rows starving the gated tiers under `attempt_count ASC` claim
order — no longer applies to a drained snapshot.) The trim deletes rows
only; `attempt_count` is never reset — the rehearsal must exercise the
real §2 arithmetic, and attempt history is forensic (0046 discipline).
Never rehearse against the sync target — the sweep requeues the ENTIRE
parked pool even under `--max-videos`, and there is no DB merge.

⚠ 2026-08-30 — the prepared workspace is STALE and the rehearsal is
UNPROVEN under the current transport; both must be redone before anything
authoritative:

- The trimmed `state.sqlite` above is built from the 2026-08-09 snapshot
  (18,700 rows); the final cohort is 53,051 rows with heterogeneous
  attempt counts (§1 addendum). Rebuild the workspace from the final
  snapshot, now pulled to the desktop (backup, trim to the gated cohort,
  vacuum).
- Transport: four WAF generations passed during the campaign. The
  REQUIRED fetch stack (VM-proven 2026-08-26) is **yt-dlp ≥ nightly
  2026.08.25 + curl_cffi (`pipx inject yt-dlp curl_cffi`) + Deno**;
  the impersonation posture REVERSED to *required* on 2026-08-25 — see
  `incident-2026-08-25-tiktok-challenge-requires-impersonation.md` and
  the dated posture-history bullet in `src-vm.md` (on the PR #35 branch
  until it merges). Install the same stack on the desktop before any
  cookied invocation.
- **Cookies + impersonation together are UNTESTED** — every jar-handling
  draft in this runbook predates the posture reversal. The rebuilt
  rehearsal must pass under the current stack before the first
  authoritative batch (§6).

```sh
/home/dmccool/src/ddp-transcribe/target/release/ddp-transcribe process \
  --state-db /home/dmccool/data/d3i/uu-tiktok/cookied-rehearsal/state.sqlite \
  --inbox /home/dmccool/data/d3i/uu-tiktok/cookied-rehearsal/inbox \
  --transcripts /home/dmccool/data/d3i/uu-tiktok/cookied-rehearsal/transcripts \
  --whisper-model /home/dmccool/src/ddp-transcribe/models/ggml-large-v3-turbo-q5_0.bin \
  --cookies-file <jar> --retries 4 --max-videos 10 --download-workers 1
```

One download worker deliberately: every cookied fetch ties to the account;
protecting it is the point of 0035's scoping. Verify:

- sweep census: `requeued_for_retry` = the rebuilt workspace's full row
  count (53,051 for a final-snapshot trim; 18,700 in the old 2026-08-09
  build), `parked_for_cookies 0`, `kept_capped 0` — a nonzero
  `kept_capped` means `--retries` is below the §2 floor;
- `cookies=true` on the gated fetch lines, jar path redacted everywhere;
- `backend="GPU" device="CUDA0"` banner (0013 — absence means the sandbox
  hid the GPU; see §7);
- the success rate. Expect some genuine deaths — no cookie recovers a
  deleted video.

The whole directory is throwaway; rebuild it from the latest snapshot
(backup, trim, vacuum) if it goes stale.

## 5. Batch calculus (operator-confirmed 2026-08-10 — ⚠ re-ruling PENDING)

⚠ 2026-08-30: this calculus was ruled on an 18,700-row cohort; the final
pool is 53,051 (verified) — roughly **52 supervised hours ≈ 106 sessions
of 500** at the drafted pace. An operator re-ruling on batch size and
cadence is pending; until it lands, the 2026-08-10 parameters below are
the only ratified ones (recorded in the desktop-mop-up invariant,
`docs/decisions/0051-*`).

Single-worker fetch runs ~3.5 s/video → the 18,700-row cohort is roughly
18 hours of supervised fetch time:

- **Batch size**: `--max-videos 500` (~30 min supervised per session).
- **Cadence**: one or two batches per day, not back-to-back — paced,
  multi-session; ~37 sessions ≈ 3–5 weeks to drain at that pace.
- **Jar refresh**: fresh export before every session (§3), plus the one-shot
  hand validation.
- **Stop conditions**: success rate sagging well below the running baseline
  mid-batch → the jar died; stop, re-validate by hand, re-export. Any
  account-level anomaly in the browser (captcha wall, forced re-login,
  verification prompt) → stop for the day.

## 6. The real mop-up (trigger FIRED 2026-08-30: campaign drained)

VM finishes the bulk corpus and pauses → final Yoda sync → desktop becomes
the **sole writer** against the authoritative DB → supervised cookied
batches per §5 → transcripts + DB flow back per the deploy repo's
procedures (`~/src/d3i/d3i-infra/researchcloud-ddp-transcribe`). The drain
happened 2026-08-30 (queue drained, final sync landed, VM paused). Before
the first authoritative batch:

- ~~pull the final snapshot to the desktop and re-verify this runbook's
  DB-derived numbers against it~~ — DONE 2026-08-30 (operator-run sync;
  read-only verification of §1's addendum and §2's floor);
- build the desktop binary from the **v0.5.1 tag** (cut 2026-08-30 —
  0043's promotion discipline applies to the mop-up host too, even though
  it is not SRC-provisioned). v0.5.0 is NOT acceptable: raising
  `--retries` un-parks residue that trips its deadline/`Cancelled`
  run-killer (§2), and the final DB is schema v7 either way;
- install and smoke the required fetch stack, then re-run the rebuilt
  rehearsal under it (§4) — cookies + impersonation have never run
  together;
- re-derive the `--retries` floor from the live DB (§2) — **4** as of the
  2026-08-30 snapshot; the max climbs as batches run, so re-derive before
  each session block;
- re-check the sweep census against the final counts before letting the
  batch proceed past its first few claims.

`requeue-failures` (0046) stays out of the normal flow: the sweep reaches
everything under the cap; the override is only for cap-exhausted rows (e.g.
after a dead-jar batch under too-low `--retries`), is default-deny, and
never resets `attempt_count`.

## 7. Desktop invocation notes

- **Sandboxed agent sessions**: pipeline invocations run unsandboxed only
  via `excludedCommands` prefix match — the command must START with the
  absolute binary path (`/home/dmccool/src/ddp-transcribe/target/release/…`).
  A leading `cd`, variable assignment, or compound command breaks the match
  and CUDA sees no GPU inside the sandbox.
- `process` exits **3** when it claimed zero videos — "nothing to do," not
  a failure.
- CUDA build line and prerequisites: CLAUDE.md ("CUDA is per-machine") and
  the README "GPU (CUDA) build" section.

## Deliberately out of scope (do not relitigate here)

- Widening the cookie gate beyond `SensitiveLoginGated` — invariant-level
  scrutiny per 0035; no evidence supports it.
- Cookies on `backfill-metadata` — rejected 2026-07-29; reopening requires a
  0035-revision ADR first.
- Requeueing the IpBlockedMessage / 10240 / 10231 cohorts — terminal on
  refreshed evidence (2026-08-13/26/29/30 spot checks): removed-or-private,
  format-mixed dead posts, and region-gated respectively (§1 addendum).
  None is login-gated; no cookie recovers any of them. The photo/video
  split of 10240 is a content-analysis question (FOLLOWUPS), not a fetch
  question.

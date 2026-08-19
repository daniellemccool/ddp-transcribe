# Incident: TikTok header-fingerprint block, 2026-08-18 — first live breaker trip

## Summary

At **2026-08-18T23:00:01Z**, on the hour, TikTok's WAF began serving block
responses to the pipeline's unimpersonated canonical-URL fetches — the
transport that had cleared the WAF since the v0.5.0 promotion (2026-08-13)
at ~76–82% success across ~450k claims. Both GPU instances hit an
instant-failure wave (~4 failures/second, every fetch, no metadata
envelope); both **circuit breakers (ADR-0050) tripped at exactly
streak = 50**, cancelled their runs, drained cleanly, and closed their
censuses with `breaker_tripped: true` and exit code 4. Total burn: **~100
attempts across both instances**, zero terminal misclassifications, both
batch-end syncs ran.

Root cause (upstream-established): TikTok's anti-bot challenge rollout —
progressing across client classes since ~2026-08-10 (the same rollout arc
as the 2026-08-10 impersonation-block incident) — reached the plain-python
HTTP-header fingerprint class. yt-dlp upstream merged the countermeasure
**the same day** (PR #17452, 2026-08-18): impersonation *removed* from
TikTok webpage requests (converging on this campaign's incident-2 posture)
plus **randomized HTTP header fingerprints**. Remedy here was ops-level:
upgrade the VM's yt-dlp to nightly 2026.08.18.122307. Validation batch
2026-08-19: 41/50, all terminals correctly classed, zero `YtDlpOther`,
breaker quiet. No pipeline code change required.

## Timeline (UTC)

- **~2026-08-10** — TikTok challenge rollout begins for impersonated
  fingerprints (our incident 2; upstream issue #17403 opened the same day).
- **2026-08-13 → 08-18** — campaign runs healthy on unimpersonated
  canonical fetches (v0.5.0 transport), ~450k claims, 76–82% success.
- **2026-08-18 ~12:23** — upstream merges PR #17452 (header-fingerprint
  randomization; impersonation removed from webpage requests); nightly
  2026.08.18.122307 published.
- **2026-08-18 23:00:01–:02** — rule reaches this traffic class: both
  instances' fetches fail instantly (`Unexpected response from webpage
  request`, one `HTTP Error 403` on video data; `no metadata envelope
  captured` on every fetch). GPU-1 log: healthy `succeeded` at 22:59:xx,
  then consecutive `YtDlpOther` retryable failures, then
  `circuit breaker tripped: consecutive claims without success —
  cancelling run streak=50 threshold=50` at 23:00:02.207. Clean drain;
  census closed (`claimed=37311 succeeded=28584`, `breaker_tripped true`);
  exit 4; sync ran. GPU-0 identical in parallel.
- **2026-08-19 ~08:30** — operator reattaches, finds both runs stopped.
  Detection gap ≈ 9.5 h of idle GPUs (the breaker stopped the burn in
  seconds; nothing told a human — see Costs).
- **2026-08-19 ~08:35** — diagnosis from the DB in one query
  (`video_events.detail_json` messages = the incident-2 block signature on
  the *unimpersonated* path) + upstream issue sweep (#17403/#17407/#17414
  → PR #17452 merged hours before onset).
- **2026-08-19 ~10:30** — remedy applied: `pipx install --force
  --pip-args=--pre yt-dlp` → nightly 2026.08.18.122307 (deliberately no
  `[curl-cffi]` extra). Probe: live canonical URL extracts and
  format-selects unimpersonated, `JS runtimes: none` (the header fix
  sidesteps the challenge; no Deno needed).
- **2026-08-19 ~11:00** — 50-video validation batch: 41/50 succeeded, 8
  `IpBlockedMessage` + 1 cookie-park, zero `YtDlpOther`,
  `breaker_tripped false`. Campaign resumed at full caps.

## DB evidence

Failure messages (query:
`SELECT json_extract(detail_json,'$.message') FROM video_events WHERE
event_type='retry_requeued' AND at >= strftime('%s','2026-08-18 22:59:30')`):

- `ERROR: [TikTok] <id>: Unexpected response from webpage request; please
  report this issue on https://github.com/yt-dlp/yt-dlp/issues …` — the
  TikTok extractor handed a challenge/block response (the incident-2
  signature, now on the unimpersonated path).
- `ERROR: unable to download video data: HTTP Error 403: Forbidden` — one
  occurrence; the media CDN refusing after extraction succeeded.

All wave failures fell to the `YtDlpOther` catch-all → retryable — the
third incident in a row where the default-retryable fallback (ADR-0033/0036
posture) was exactly right: **zero terminal writes during the wave; the
whole cohort re-enters via the retry tier.** Both `batch_runs` rows closed
with censuses (contrast: the 2026-08-17 deadline-kill left rowid 20
unclosed — fixed in v0.5.1, merged 2026-08-19, tag pending).

## Mechanism

Established by upstream (yt-dlp #17403, high-priority/impersonation/site-bug;
fixed by PR #17452, commit `b375e1d`): TikTok's WAF now keys on the
**HTTP header fingerprint** of non-browser clients — not TLS (incident 1's
axis) and not curl_cffi impersonation signatures (incident 2's axis). The
fix randomizes header fingerprints per request and removes impersonation
from webpage requests entirely. Consistent local observations: failures
were instant (refusal, not throttle/timeout); both instances at the same
second (global rule deployment, not IP-scoped reaction — matching
incidents 1–2, which reproduced from residential egress); the same egress
+ same TLS stack works immediately with only headers changed (the nightly,
unimpersonated).

## Remedy and proof

Ops-level only — **no pipeline release required** (the pinned v0.5.0
binary invokes whatever `yt-dlp` is on PATH):

1. `pipx install --force --pip-args=--pre yt-dlp` → nightly
   **2026.08.18.122307** (contains `b375e1d`). No `[curl-cffi]` extra:
   impersonation stays uninstalled, per the incident-2 ruling — which
   upstream has now adopted as default behavior for TikTok webpage
   requests.
2. Probe (removed video): prints `Your IP address is blocked from
   accessing this post` — **the `IpBlockedMessage` stderr text is
   byte-identical on the nightly**, so that ADR-0033 pattern survives the
   version jump (first drift check, incidental).
3. Probe (live video): full extraction + format selection
   (`bytevc1_720p_484720-1`), unimpersonated, no JS runtime.
4. `--list-impersonate-targets` still lists targets `(unavailable)` with
   `curl_cffi` in each Source cell — the v0.5.0 `params_json` parser's
   recognition rule and the `ytdlp_impersonation_available: false` witness
   both keep working; `ytdlp_version` in `params_json` now records the
   nightly string automatically.
5. Validation batch (50): 41 succeeded / 8 `IpBlockedMessage` / 1
   cookie-park / **zero `YtDlpOther`** / breaker quiet — classification
   verified against the new extractor at first order.

## What this validates, and what it costs

| | Incident 1 (08-06) | Incident 2 (08-10) | This (08-18) |
|---|---|---|---|
| Burn before stop | 1,806,618 attempts / 60 h | ~1,360 attempts / 21 min | **~100 attempts / ~10 s** |
| Stopped by | operator luck | operator watching | **breaker, automatically** |
| Census of the dying run | written at manual stop | written at manual stop | **closed automatically, breaker-visible** |
| Terminal misclassifications | 0 | 0 | 0 |
| Time to diagnosis | days (probe matrix) | hours | **minutes (one DB query + upstream sweep)** |
| Fix latency | ops config, days of probing | ops change same day | **upstream fix pre-existed onset; applied next morning** |

Every safety mechanism built from incidents 1–2 fired as designed on its
first live test: the breaker bounded the burn, the census carried the
verdict, `params_json` carried the environment, the classification
fallback kept the wave retryable, and the runbook's exit-4 row scripted
the response. The remaining gap is **detection latency**: 9.5 idle GPU
hours because the hourly dead-man census check (filed after incident 1,
re-filed after incident 2) is *still* not installed. The breaker is the
"stop the burn" half; the cron is the "tell a human" half; this incident
is the third consecutive demonstration that the second half is missing.

## Durability — same conclusion as incident 2, one rung higher

This is an actively tuned WAF ratchet: TLS fingerprints (incident 1), then
impersonation fingerprints (incident 2), now plain header fingerprints
(this). The strategic posture is unchanged and validated: fetch the
canonical URL as an honest client, ride yt-dlp's ecosystem (everyone broke
at 23:00; the fix was merged before we even noticed), keep the breaker
armed for the next rung. Standing consequences:

- **The deploy repo's `ytdlp` role is now doubly wrong for a rebuild**: it
  installs stable 2026.07.04 (cannot fetch TikTok at all post-rollout) and
  would reinstate curl_cffi. It must pin nightly ≥ 2026.08.18 (or the next
  stable containing `b375e1d`) with curl_cffi excluded — until then, any
  involuntary rebuild is a fetch outage.
- **ADR-0033 drift exposure is live**: the pinned pattern set now runs
  against a nightly a year newer than its corpus. First-order check passed
  (above); watch `YtDlpOther` share in the full-cap censuses.
- The `--retries 3` argv cap (v0.5.1, merged) reaches the VM at the next
  tag promotion; until then the nightly's internal retries still default
  to 10 on the deployed binary's invocations.

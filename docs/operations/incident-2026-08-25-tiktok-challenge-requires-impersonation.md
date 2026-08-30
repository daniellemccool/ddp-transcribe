# Incident: TikTok challenge requires impersonation, 2026-08-25 — the posture reversal

## Summary

At **2026-08-25T18:52Z** (cliff onset mid-minute; ~75–95 successes/min
until 18:51, then instant block pages) TikTok's WAF closed the
header-randomization path that had carried the campaign since 2026-08-19.
Both instances' breakers tripped at streak = 50 (~100 attempts total,
censuses closed, rowids 26/27), the fourth WAF rung in twenty days and the
second clean breaker save. Diagnosis and remedy established a **reversal
of the standing DO-NOT-IMPERSONATE posture**: TikTok's current challenge
flow requires a browser-grade (impersonated) client to receive a solvable
response, and yt-dlp upstream reintroduced impersonation for TikTok after
having removed it on 2026-08-18. The working stack on the VM since
2026-08-26 is: **yt-dlp nightly 2026.08.25.233329 + curl_cffi (pipx
inject) + Deno 2.9.5** (JS runtime for the challenge solver; necessity
today unverified — kept as upstream-direction insurance). Validation
batch: 33/50 succeeded, zero `YtDlpOther`, zero retryables, breaker
quiet, normal pace (no visible per-fetch challenge tax).

**Witness semantics flipped:** `ytdlp_impersonation_available` in
`params_json` now reads `true` when HEALTHY. From 2026-08-10 to
2026-08-25 the healthy reading was `false`. Any monitoring or operator
habit built on the old reading must be updated — this is the single most
misinterpretable consequence of the reversal.

## Evidence chain for the reversal

Probes on the VM, 2026-08-25/26, live test video via the pipeline's
`/video/` + `@x` URL form:

| Stack | Result |
|---|---|
| nightly 08-18, no Deno, no curl_cffi | challenge fail (`_solve_challenge_and_set_cookies` → "Unexpected response") |
| nightly 08-25 (same day's build), no Deno, no curl_cffi | same failure |
| + Deno 2.9.5 | same failure, plus explicit `WARNING: extractor is attempting impersonation, but no impersonate target is available` |
| + curl_cffi (`pipx inject yt-dlp curl_cffi`) | **extracts** (`Impersonation target: chrome-150:macos-26`) |

The warning line is the extractor's own testimony that the nightly
*expects* impersonation again. The 2026-08-10 rule that motivated the ban
(curl_cffi's then-Chrome fingerprint served a block page) is two WAF
generations stale; the current target passes. `pipx uninject yt-dlp
curl_cffi` reverts the stack if the ratchet turns again.

## The arms-race ledger (four rungs, twenty days)

| Rung | Date | WAF key | Working posture after |
|---|---|---|---|
| 1 | 08-06 | non-browser TLS on the share host | impersonate (ops config) |
| 2 | 08-10 | curl_cffi impersonation fingerprint | UNimpersonated canonical URL |
| 3 | 08-18 | plain-client HTTP header fingerprint | nightly's randomized headers, unimpersonated |
| 4 | 08-25 | challenge flow requiring browser-grade client | **impersonated (curl_cffi) + challenge solver (Deno available)** |

Positions 2 and 4 are opposites. The durable lesson is not any posture
but the meta-posture: ride yt-dlp's ecosystem (every rung broke the whole
user base; upstream's fix latency has been 0–2 days), keep the breaker
armed (rungs 3 and 4 cost ~100 attempts each vs rung 1's 1.81M), and
treat transport configuration as evidence-driven and reversible.

## Deploy-repo consequences (now the critical path)

A rebuild under the current `ytdlp` role produces a machine that cannot
fetch: it installs stable 2026.07.04 (pre-challenge extractor) WITHOUT
curl_cffi (per the now-reversed incident-2 posture) and without Deno. The
role must install: yt-dlp ≥ nightly 2026.08.25 (or the next stable
containing the challenge/impersonation flow), `curl_cffi` injected, and
Deno — and its provisioning check ("verify impersonation targets
available") is *correct again*. Until the role lands, an involuntary
rebuild is a total fetch outage with a misleading `false` witness.

## Also discovered during validation (separate note)

The batch's elevated terminal share led to browser verification that
exposed the `VideoNotAvailable10240` class as **format-mixed**: TikTok
photo-mode posts are confirmed among it (5/6 sampled redirect to
`/photo/`), one sampled id does not redirect, and the sample (one
~hour-wide creation window) cannot estimate the cohort's composition —
**inconclusive**, resolving instruments filed in FOLLOWUPS. The firm
corollary: live photo posts extract successfully through the pipeline
(soundtrack audio), so the transcript corpus already contains
photo-post soundtracks — a content-analysis note, not a pipeline defect.
Terminal-semantics guidance in the runbook updated accordingly (the
2026-08-13 "deletion flavor" reading of 10240 was overconfident).

## Detection

Third consecutive incident where both GPUs sat idle undetected (18:52 →
~08:30 next morning) because the hourly dead-man census check remains
uninstalled. The breaker owns the burn; nothing yet tells a human.

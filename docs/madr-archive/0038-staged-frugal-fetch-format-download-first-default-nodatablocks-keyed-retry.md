---
status: accepted
date: "2026-07-08"
links:
    pattern-precedent:
        - "0035"
comments:
    - author: Danielle McCool
      date: "2026-07-08 14:43:54"
      text: marked decision as decided
    - author: Danielle McCool
      date: "2026-07-08 14:44:51"
      text: marked decision as decided
    - author: Danielle McCool
      date: "2026-07-08 14:45:04"
      text: marked decision as decided
    - author: Danielle McCool
      date: "2026-07-08 16:46:14"
      text: marked decision as decided
---


# Staged frugal fetch format: download-first default, NoDataBlocks-keyed retry

## Context and Problem Statement

A 2026-07-08 live probe of 20 fresh videos (random sample from a new donor
inbox) plus the pilot-DB failure/success classes found that yt-dlp's
`download` format — TikTok's pre-rendered, watermarked share-link MP4, the
prior unconditional first choice — is consistently ~3x larger than the
smallest audio-tagged ABR variant (116.1 MB vs 39.9 MB across the 14 probe
videos where both landed, ~66% waste), for content we discard down to a 16
kHz mono WAV. Worse, `download`'s advertised-but-unservable failure mode
(selection succeeds, transfer dies with "Did not get any data blocks")
*is* the pilot's `NoDataBlocks` class — 2,318 rows, 2,311 still alive at
census — where a selection-time fallback chain cannot recover mid-transfer
once `download` is picked. The smallest advertised audio-bearing format
served 17/17 probe videos with a real audio stream (ffprobe-verified),
including TikTok's occasional audio-only `audio` format (509 KB).

The retained caveat, and the reason a pure frugal-only policy is not safe
for every retry: yt-dlp issues #15891 / #16622 document that ABR variants
intermittently serve h265 video-only files despite being tagged
`acodec=aac` by the extractor (`yt_dlp/extractor/tiktok.py` stamps the
claim in `COMMON_FORMAT_INFO` regardless of what TikTok's CDN actually
muxes). Such a fetch fails at wav extraction and classifies as
`FfprobePostprocess` — a class `download`'s pre-muxed asset is not subject
to, since it comes from a different TikTok pipeline than the ABR variants.

**Operator reversal (2026-07-08, evidence review).** This ADR originally
adopted frugal-first as the unconditional default. On review, that default
was judged not yet justified: the frugal probe is n=17 fresh videos against
a pilot-scale record (tens of thousands of rows) for the `download`-first
selector; yt-dlp issue #16622 is *open* against exactly the ABR formats the
frugal selector prefers, so the liar-metadata class is a live, unresolved
risk rather than a bounded one-retry cost; and there is no evidence yet
about whether the two format populations see differential rate-limiting
from TikTok's CDN. The feature is rescoped from an unconditional default
flip to a staged experiment: keep the pilot-proven `download`-first
selector as the default, and run the frugal selector only where it is
already known to help — retries of the `NoDataBlocks` class, which
`download`-first cannot recover by construction (see below).

How should format selection change, given the footprint goal (minimize
downloaded bytes for content that is immediately transcoded to audio-only)
without regressing recoverability for the liar-metadata class, and without
committing to an under-evidenced default flip?


## Considered Options

* Keep download-first as the unconditional default, with no frugal path at all (status quo; forgoes the footprint learnings entirely)
* Frugal-first as the unconditional default, with a deterministic-audio override (download first, then h264 fallbacks) applied only to retries whose prior failure classified `FfprobePostprocess` (this ADR's original decision)
* Frugal-first as the unconditional default, with no override for any retry kind
* Staged experiment: keep `download`-first (`DeterministicAudio`) as the default for fresh claims and every retry kind except `NoDataBlocks`; key the frugal selector only to `NoDataBlocks`-classified retries, where `download`-first is a proven-unrecoverable dead end; record the fetch policy in every fetch-failure event so the parked `NoDataBlocks` backlog's retry batch doubles as an at-scale frugal read, with an explicit census-gated trigger for revisiting the default


## Decision Outcome

Chosen option: "Staged experiment: keep `download`-first (`DeterministicAudio`) as the default for fresh claims and every retry kind except `NoDataBlocks`; key the frugal selector only to `NoDataBlocks`-classified retries, where `download`-first is a proven-unrecoverable dead end; record the fetch policy in every fetch-failure event so the parked `NoDataBlocks` backlog's retry batch doubles as an at-scale frugal read, with an explicit census-gated trigger for revisiting the default", because the frugal probe (n=17) is not yet strong enough evidence to flip an unconditional default against a pilot-scale download-first record with an open yt-dlp issue against exactly the ABR formats frugal prefers; staging the frugal selector to NoDataBlocks-keyed retries only captures the one proven win (the unrecoverable backlog class) while the instrumented backlog retry batch becomes the at-scale evidence a future default flip would need.

### Consequences

* Good, because the parked `NoDataBlocks` pilot backlog (~2,318 rows)
  becomes recoverable post-deploy via an operator-issued retry budget
  (e.g. `--retries 2`) without further code change — the backlog IS a
  `download`-selection casualty (selection succeeded, the transfer died
  mid-stream), and a selection-time fallback chain cannot recover
  mid-transfer once `download` is picked, so the retry must not re-pick
  it. The frugal selector, verified against probe fixtures to serve a real
  audio stream 17/17 times, is what resolves it.
* Good, because `Store::record_fetch_failure` now writes the fetch
  policy's tag (`"deterministic-audio"` / `"frugal"`) into every
  `retry_requeued` / `cookie_parked` / `failed_retryable` event's detail
  JSON (additive extension of the Epic 4a uniform-shape contract). The
  backlog retry batch running under this instrumentation IS the at-scale
  frugal experiment the n=17 fresh probe could not be: outcomes are
  attributable per policy from the event stream, not just inferred from
  aggregate success/failure counts.
* Good, because the experiment's success signal is reconstructible
  directly from the event chain without new bookkeeping: a video whose
  event history shows `failed_retryable` (detail `kind=NoDataBlocks`) →
  a later `claimed` → `succeeded` is a frugal-selector success for the
  `NoDataBlocks` class. Aggregating this pattern across the backlog after
  the retry batch completes gives the frugal success rate for that class.
* Neutral, because the fresh-claim and every-other-retry-kind path is
  unchanged from the pilot: `download`-first, then h264, then any best —
  byte-identical argv to what the pilot ran, so this ADR introduces zero
  regression risk to the already-proven majority path while the frugal
  path is validated at scale.
* Bad, because the ~66% footprint reduction the frugal probe demonstrated
  is deferred, not realized, for fresh fetches — the default keeps
  downloading `download`'s pre-muxed asset at its full size until the
  decision trigger below is met and a future ADR flips the default.
* Neutral, because the format-policy gate keys on the pinned (reserved)
  literal label `NoDataBlocks` (unlike the cookie gate, it has no
  dedicated disposition to resolve through the active classification
  table — same structural caveat this ADR's original decision recorded
  against `FfprobePostprocess`) — an operator's custom classification
  table that renames that label silently disables the frugal retry, so
  the label must be kept verbatim in custom tables.

**Decision trigger for a future frugal-default flip.** Revisit this ADR
after the `NoDataBlocks` backlog retry batch has run to completion and a
census is taken of the resulting event stream. A future ADR may flip the
default to frugal-first only if BOTH hold: (1) `FfprobePostprocess` fallout
from the frugal-retried backlog is approximately zero (the liar-metadata
class is not, in practice, hitting the frugal selector's `/b` fallback at
a meaningful rate), and (2) the overall failure mix for the backlog is no
worse under frugal than the historical `download`-first failure mix for
comparable videos. Absent both conditions, the staged design in this ADR
remains current.

Pattern precedent: this ADR's per-claim, kind-keyed opts function
(`format_policy_for`, beside `cookie_opts_for`) mirrors ADR 0035's cookie
gate exactly — same composition point, same `last_retryable_kind` read,
disjoint keyed labels so the two gates never both apply to one claim.

## Comments

* **2026-07-08 14:43:54 — @Danielle McCool:** marked decision as decided
* **2026-07-08 14:44:51 — @Danielle McCool:** marked decision as decided
* **2026-07-08 14:45:04 — @Danielle McCool:** marked decision as decided
* **2026-07-08 16:46:14 — @Danielle McCool:** marked decision as decided

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
---

# Frugal-default fetch format with kind-keyed deterministic retry

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

How should format selection change, given the footprint goal (minimize
downloaded bytes for content that is immediately transcoded to audio-only)
without regressing recoverability for the liar-metadata class?

## Considered Options

* Keep download-first as the unconditional default (status quo)
* Frugal-first (smallest audio-tagged combined format) as the unconditional default, with a deterministic-audio override (download first, then h264 fallbacks) applied only to retries whose prior failure classified FfprobePostprocess
* Frugal-first as the unconditional default, with no override for any retry kind

## Decision Outcome

Chosen option: "Frugal-first (smallest audio-tagged combined format) as the unconditional default, with a deterministic-audio override (download first, then h264 fallbacks) applied only to retries whose prior failure classified FfprobePostprocess", because it captures the ~66% footprint reduction and eliminates download's advertised-but-unservable failure mode from fresh fetches, while giving the yt-dlp #15891/#16622 liar-metadata class a deterministic pre-muxed fallback on its one retry.

### Consequences

* Good, because the ~3% `download`-unservable/poisoned class
  (`NoDataBlocks`) disappears from fresh fetches — frugal selection never
  picks `download`, so the mid-transfer failure mode it causes cannot occur
  on a first attempt.
* Good, because average per-video downloaded bytes drop roughly 3x for the
  large majority of fetches that don't hit the liar-metadata class.
* Neutral, because an intermittent no-audio ABR liar (yt-dlp
  #15891/#16622) still costs one retry before landing on `download` via
  `DeterministicAudio` — recoverable, not free, but bounded to a single
  extra attempt.
* Good, because the parked `NoDataBlocks` pilot backlog becomes recoverable
  post-deploy via an operator-issued retry budget (e.g. `--retries 2`)
  without a code change — it was never a `download`-selection problem, so
  the frugal default resolves it directly.

## Comments

* **2026-07-08 14:43:54 — @Danielle McCool:** marked decision as decided
* **2026-07-08 14:44:51 — @Danielle McCool:** marked decision as decided
* **2026-07-08 14:45:04 — @Danielle McCool:** marked decision as decided

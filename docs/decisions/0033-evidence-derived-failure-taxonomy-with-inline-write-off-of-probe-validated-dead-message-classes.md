---
status: proposed
date: "2026-07-07"
---

# Evidence-derived failure taxonomy with inline write-off of probe-validated dead message classes

## Context and Problem Statement

Epic 2 records failures with placeholder string kinds ("Fetch", "Transcribe",
"FetchOrTranscribe"). The completed 65k production run (2026-06-16 → 2026-07-03,
87.5% success, 7,087 failed_retryable) plus ground-truthing via TikTok's oEmbed
endpoint (n=36, perfect separation) showed the two dominant yt-dlp error
messages are INVERTED relative to their surface meaning: "Your IP address is
blocked" marks deleted videos (10/10 dead), while "Did not get any data blocks"
marks live, re-fetchable videos (10/10 alive; 10/10 re-fetch OK from the same
egress). How should the pipeline classify failures at failure time?

## Considered Options

* Evidence-derived enums (Retryable/Unavailable/Bug arms; variants only for observed corpus classes; inline terminal write-off of "IP blocked" + "status code 10231"; default-cautious catch-alls)
* Full speculative taxonomy from the Plan B spec (11 RetryableKind + 7 UnavailableReason variants, stderr classification tables as primary signal)
* No pipeline classification; record raw strings and defer all verdicts to the operator triage pass

## Decision Outcome

(placeholder — set via adr decide at epic close)

## Consequences

* Write-off patterns (substring match on yt-dlp stderr): "Your IP address is
  blocked" → UnavailableReason::IpBlockedMessage; "status code 10231" →
  UnavailableReason::VideoNotAvailable10231. Both route to
  mark_terminal_failure at failure time — its first caller. Residual
  false-terminal risk accepted by operator ruling 2026-07-07 (probe evidence
  15/15 dead; the classes are ~55% of all failures).
* Default-cautious: unmatched fetch stderr → RetryableKind::YtDlpOther;
  unmatched transcribe errors → RetryableKind::TranscribeOther. Bug requires an
  explicit match (tool missing, internal invariant); never a fallback.
* Classifier refuses to treat any error with exit_code == Some(0) as success.
* Version pinning: patterns validated against yt-dlp 2026.03.17 stderr from the
  65k run and TikTok oEmbed behavior observed 2026-07-06/07. Re-verify on
  yt-dlp upgrade or oEmbed drift.
* Enums serialize into the existing v2 string columns via tag()/message()
  per 0023 — no schema change.

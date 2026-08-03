---
status: accepted
date: "2026-07-08"
category: Failure classification
applies_to:
    - src/failure.rs
    - src/classification.rs
priority: invariant
---

# Classify on evidence, never message text

## Decision

Failure classification is evidence-derived: a message class exists only for
patterns observed in the corpus, a class is written off as terminal only with
probe/census evidence, and unmatched errors fall to cautious retryable
catch-alls (`YtDlpOther`, `TranscribeOther`). Bug-class requires an explicit
match — it is never a fallback.

## Guidance

- **Message text lies about causes — classify on evidence, never on what the message says.** The canonical trap, relitigated repeatedly (May–July 2026): `IpBlockedMessage` ("Your IP address is blocked") means **VIDEO REMOVED** (probe-validated 10/10 dead), while "Did not get any data blocks" rows re-fetched 10/10 OK from the same egress, affirmatively clearing the IP. The guard comment lives on the IpBlockedMessage rule in the compiled default table (`src/classification.rs`); keep it.
- Adding or flipping a class's disposition requires probe/census evidence in the record or the classification-table provenance; review rejects dispositions argued from message wording.
- The classifier never treats an error with `exit_code == Some(0)` as success, and structural errors (timeout, spawn failure, missing output) stay code-mapped in `src/failure.rs` — only tool-output interpretation is table-driven.
- Patterns are pinned to observed tool versions (yt-dlp 2026.03.17 stderr; oEmbed behavior of 2026-07-06/07); re-verify on yt-dlp upgrade or oEmbed drift.

## Why

The 65k pilot run plus oEmbed ground-truthing (n=36, perfect separation)
proved the two dominant yt-dlp messages are INVERTED relative to their
surface meaning — a speculative taxonomy built from message text would have
written off ~2,300 recoverable videos and retried thousands of dead ones.
Cautious catch-alls cost only wasted retries; a wrong terminal write-off
silently discards donor data.

## Context

Terminal write-off classes at Epic 4a (in the classification table's
compiled default): IpBlockedMessage, VideoNotAvailable10231,
VideoNotAvailable10240 (606/606 probe-dead at the 2026-07-07 census).
NoPermission stays retryable — the census found it impure (25/452 alive).
Residual false-terminal risk on the write-off classes was accepted by
operator ruling 2026-07-07 (probe evidence 15/15 dead; the classes are ~55%
of all failures).

## Alternatives

- **The Plan B spec's speculative taxonomy (11 retryable + 7 unavailable variants)** — variants for classes never observed; message-text tables as primary signal, which the evidence showed is inverted.
- **No pipeline classification; operator adjudicates raw strings** — pushes every verdict to a manual pass; the pilot showed the dominant classes are settled enough to automate.

---
status: accepted
date: "2026-07-07"
comments:
    - author: Danielle McCool
      date: "2026-07-07 04:37:05"
      text: marked decision as decided
---

# Cookies scoped to SensitiveLoginGated retries only, with argv redaction

## Context and Problem Statement

301 videos (4.2% of failures) are alive but login-gated ("This post may not be
comfortable for some audiences"); yt-dlp needs cookies to fetch them. The
study's research question concerns videos related to crime and policing, which
have a higher-than-average chance of being flagged sensitive — excluding this
class would bias the sample against precisely the content under study. How
should cookie support be scoped?

## Considered Options

* Cookies passed only on retries of claims whose last_retryable_kind is SensitiveLoginGated (~300-fetch account exposure)
* Global --cookies flag applied to every yt-dlp invocation (~50k-fetch account exposure)
* No cookie support; write the class off as terminal

## Decision Outcome

Chosen option: "Cookies passed only on retries of claims whose last_retryable_kind is SensitiveLoginGated (~300-fetch account exposure)", because research-validity rationale (crime/policing content skews sensitive) justifies cookie support; retry-only scope caps account exposure at ~300 fetches.

## Consequences

* First attempts never send cookies; only requeued sensitive-class claims do.
  A mid-run account block therefore cannot degrade the bulk pipeline.
* The cookie file path is redacted from the structured subprocess log via
  CommandSpec.redact_arg_indices and scrubbed from stderr excerpts before they
  reach error messages or the state DB.
* The operator supplies --cookies-file at `process` time; absent the flag,
  sensitive-class claims are fetched without cookies (and will re-fail into
  failed_retryable — harmless, capped by 0034's attempt cap).

## Comments

* **2026-07-07 04:37:05 — @Danielle McCool:** marked decision as decided

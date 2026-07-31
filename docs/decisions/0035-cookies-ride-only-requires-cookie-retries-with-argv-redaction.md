---
status: accepted
date: "2026-07-08"
category: Fetcher
applies_to:
    - src/pipeline/mod.rs
    - src/fetcher/ytdlp.rs
    - src/classification.rs
priority: invariant
companions:
    - src/process.rs
---

# Cookies ride only requires-cookie retries, with argv redaction

## Decision

The operator's cookie file rides a yt-dlp invocation only when the claim's
`last_retryable_kind` resolves through the active classification table to
disposition `requires-cookie` — retries of login-gated rows, nothing else.
First attempts never send cookies, and the cookie path is redacted from
structured subprocess logs and scrubbed from stderr excerpts before they
reach error messages or the state DB.

## Guidance

- `cookie_opts_for` (`src/pipeline/mod.rs`) is the single decision point: cookies ride iff `table.disposition_of(last_retryable_kind)` is `requires-cookie` AND the operator supplied `--cookies-file`; review rejects cookie args added at other call sites. The gate widens through the table, not code — the shipped default grants `requires-cookie` only to `SensitiveLoginGated`, and granting it to another label in any table (including an operator `--classification` table) widens credential exposure and gets the same invariant-level scrutiny.
- Any new path that can persist or print argv/stderr must respect `CommandSpec.redact_arg_indices` and the stderr scrub — the cookie file path is a credential and must not land in logs, error strings, or the DB.
- Absent `--cookies-file`, sensitive-class claims fetch cookie-less and re-fail into the parked pool (harmless, retry-cap-bounded); don't "fix" that by sending cookies from elsewhere.

## Why

Retry-only scoping caps research-account exposure at roughly the size of the
sensitive class (~300 fetches) instead of the whole corpus (~50k) — a mid-run
account block therefore cannot degrade the bulk pipeline — and redaction
keeps the credential out of durable artifacts. The class cannot simply be
written off: the study concerns crime/policing content, which skews
sensitive, so excluding it would bias the sample against precisely the
content under study.

## Context

The 2026-07 census found 301 videos (4.2% of failures) alive but login-gated
("This post may not be comfortable for some audiences"); yt-dlp needs account
cookies to fetch them. Requeued sensitive-class claims carry the kind
snapshot that gates the cookie decision at claim-fetch time.

## Alternatives

- **Global `--cookies` on every invocation** — ~50k-fetch account exposure; one block kills the batch.
- **No cookie support; write the class off as terminal** — biases the corpus against the research question.

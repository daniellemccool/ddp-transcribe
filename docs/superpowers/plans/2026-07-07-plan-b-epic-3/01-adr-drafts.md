# Task 01: Draft the three Epic 3 ADRs (proposed status)

**Files:**
- Create (via tool): `docs/decisions/0033-*.md`, `docs/decisions/0034-*.md`, `docs/decisions/0035-*.md`

**Interfaces:**
- Consumes: `scripts/adr` wrapper (`adr new <title>` prints the assigned ID; `adr edit <id> < body` takes the MADR body on stdin).
- Produces: three `proposed` ADRs later tasks cite as 0033/0034/0035 and Task 11 flips to `accepted`. **If `adr new` prints different IDs, record the mapping in the commit message and use the printed IDs in all later tasks.**

ADRs are captured at plan time, not retroactively (Plan B meta-process discipline). Bodies must contain the MADR required sections: `## Context and Problem Statement`, `## Considered Options` (bulleted with `*`), `## Decision Outcome` (placeholder — `adr decide` overwrites it in Task 11). Durable content (patterns, evidence tables) goes in `## Consequences`, which `adr decide` does not touch.

- [ ] **Step 1: Draft ADR 0033 — evidence-derived failure taxonomy + inline write-off**

```bash
cd /home/dmm/src/uu-tiktok
scripts/adr new "Evidence-derived failure taxonomy with inline write-off of probe-validated dead message classes"
# note the printed ID (expected 0033)
scripts/adr edit 0033 <<'EOF'
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
EOF
```

- [ ] **Step 2: Draft ADR 0034 — operator triage subcommand as the retry executor**

```bash
scripts/adr new "Operator triage subcommand: oEmbed oracle via curl subprocess, message-class fast path, attempt-capped requeue"
scripts/adr edit 0034 <<'EOF'
## Context and Problem Statement

failed_retryable is a sink on current main: claim_next selects only pending and
nothing resets failed rows. The 65k run left 7,087 rows there, of which ~2,400
are recoverable (probe-alive) and ~4,400 are dead. The architecture docs
promise "Epic 3 adds retry policy". Where does retry execution live, and how
are dead rows distinguished from recoverable ones at scale?

## Considered Options

* Single operator-driven `triage` subcommand: message-class fast path for write-off classes, oEmbed probe (curl via bounded process::run) for the rest; dead → failed_terminal, alive → pending under attempt cap; operator re-runs `process`
* Automatic in-pipeline retry with per-kind backoff
* Probe inside the pipeline classifier at failure time (network call on the hot path)

## Decision Outcome

(placeholder — set via adr decide at epic close)

## Consequences

* The pipeline hot path stays network-pure; TikTok endpoint availability can
  never stall `process`.
* Probe transport is the system `curl` binary through process::run (argv
  direct, bounded capture per 0021, explicit timeout) — no new HTTP-client
  dependency. `curl` on PATH becomes a runtime requirement for `triage` only.
* Probe oracle: GET https://www.tiktok.com/oembed?url=https://www.tiktok.com/@x/video/<id>;
  HTTP 200 → alive, 400/404 → dead, anything else → unreachable (row untouched).
  Validated 2026-07-06/07 (n=36, perfect separation). External-endpoint drift
  risk: re-validate on a sample if verdict distributions shift.
* Requeue is capped at attempt_count < 3 by default (--max-attempts). Requeue
  re-classifies the stored message and writes the normalized kind back, so
  historical placeholder-"Fetch" rows acquire taxonomy kinds without a wasted
  refetch.
* Both triage transitions write video_events rows (triaged_terminal, requeued)
  — operator actions are auditable, unlike the 0024 sweep.
* The per-kind census output is the study's attrition documentation.
EOF
```

- [ ] **Step 3: Draft ADR 0035 — cookie policy for the sensitive class**

```bash
scripts/adr new "Cookies scoped to SensitiveLoginGated retries only, with argv redaction"
scripts/adr edit 0035 <<'EOF'
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

(placeholder — set via adr decide at epic close)

## Consequences

* First attempts never send cookies; only requeued sensitive-class claims do.
  A mid-run account block therefore cannot degrade the bulk pipeline.
* The cookie file path is redacted from the structured subprocess log via
  CommandSpec.redact_arg_indices and scrubbed from stderr excerpts before they
  reach error messages or the state DB.
* The operator supplies --cookies-file at `process` time; absent the flag,
  sensitive-class claims are fetched without cookies (and will re-fail into
  failed_retryable — harmless, capped by 0034's attempt cap).
EOF
```

- [ ] **Step 4: Validate and commit**

```bash
scripts/adr validate
git add docs/decisions/
git commit -m "docs(adr): draft Epic 3 ADR slate (taxonomy+write-off, triage, cookie policy) as proposed"
```

Expected: `adr validate` passes (the pre-commit hook re-runs it). If `adr new` assigned IDs other than 0033/0034/0035, disclose the mapping in this commit message.

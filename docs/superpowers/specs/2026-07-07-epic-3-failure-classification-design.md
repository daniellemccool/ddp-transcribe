# Plan B Epic 3 — failure classification, triage, and cookie-scoped retry (design)

**Date:** 2026-07-07
**Status:** Approved by operator (brainstorm session 2026-07-07); supersedes the
scope sketched in `docs/superpowers/plans/2026-05-12-plan-b/EPIC-3-SKETCH.md`.
**Companion:** `docs/superpowers/plans/PLAN-B-EPIC-3-KICKOFF-PROMPT.md` carries the
full evidence session (census queries, probe method, retry test); this spec states
the resulting design.

## Context and evidence (summary)

The completed 65k production run (2026-06-16 → 2026-07-03) ended at 49,533/56,620
succeeded (87.5%), 7,087 `failed_retryable` (12.5%, all `attempt_count = 1` — the
sink behaved as designed), zero pending. Ground-truthing with TikTok's oEmbed
endpoint (`https://www.tiktok.com/oembed?url=…`, n=36, perfect separation) and a
10/10-successful re-fetch test from the workspace's own egress established:

| Message class | Count | Verdict |
|---|---|---|
| "Your IP address is blocked" | 3,241 (45.7%) | Dead (deleted content; message misleading) |
| "Did not get any data blocks" | 2,318 (32.7%) | Alive; transient — re-fetch succeeds from same egress, both URL forms |
| "Video not available, status code 10231" | 674 (9.5%) | Dead |
| "You do not have permission" | 452 (6.4%) | Dead (private/removed) |
| "This post may not be comfortable…" | 301 (4.2%) | Alive, login-gated (yt-dlp needs cookies) |
| ffprobe / no-formats / network / HTTP tails | ~101 (1.4%) | Mixed; mostly transient |

Error-message text is *inverted* relative to surface meaning on the two dominant
classes. Classification therefore keys on (a) a small set of probe-validated message
classes for inline verdicts and (b) the oEmbed probe as the authoritative oracle for
everything else. The share-link canonicalization hypothesis is refuted (share-form
URLs succeed at 87.5% and re-fetch fine); URL-form work stays out of scope.

**Research-validity rationale for cookie support:** the study's research question
concerns videos related to crime and policing, which have a higher-than-average
chance of being flagged sensitive. Excluding the login-gated class would bias the
sample against precisely the content under study. This rationale goes in the cookie
ADR verbatim.

## Operator decisions (settled in brainstorm)

1. **Single `triage` subcommand** owns probe + verdict + requeue. Pipeline hot path
   stays network-pure (no oEmbed calls during `process`).
2. **Inline write-off:** "IP blocked" and "status code 10231" message classes are
   marked `failed_terminal` at failure time in the pipeline. Probe-validated 15/15
   dead; the residual false-terminal risk is accepted as not worth recovering.
   Everything else stays `failed_retryable` with an evidence-derived kind tag.
3. **Cookie support in scope**, passed **only** on retries of kind
   `SensitiveLoginGated` — never on first attempts. Account exposure ≈ 300 fetches.

## Design

### 1. Taxonomy types

New module (`src/failure.rs`; `errors.rs` keeps the tool-level error enums):

```rust
pub enum ClassifiedFailure {
    Retryable   { kind: RetryableKind, ctx: FailureContext },
    Unavailable { reason: UnavailableReason, ctx: FailureContext }, // → mark_terminal_failure
    Bug         { ctx: FailureContext },                            // → worker Err (abort)
}

pub enum RetryableKind {
    NoDataBlocks, NoPermission, SensitiveLoginGated, NoVideoFormats,
    FfprobePostprocess, NetworkTransient, HttpError, ToolTimeout,
    YtDlpOther,      // default-cautious catch-all: unmatched fetch stderr
    TranscribeOther, // default-cautious catch-all: unmatched transcribe errors
}

pub enum UnavailableReason {
    IpBlockedMessage,        // probe-validated 10/10 dead, 2026-07-06
    VideoNotAvailable10231,  // probe-validated 5/5 dead; TikTok's own status code
}

pub struct FailureContext {
    pub tool: &'static str,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,          // new: ExitStatusExt capture, cfg(unix)
    pub stderr_excerpt: String,
    pub classification_reason: &'static str, // which rule matched (audit trail)
}
```

- Variants are **evidence-derived from the 65k corpus only**. The sketch's
  speculative variants (OOM, BadAudio, EmptyTranscript, RateLimited, …) are not
  implemented; `YtDlpOther`/`TranscribeOther` absorb the unknown, and triage
  adjudicates. Default-cautious posture: unmatched → `Retryable`, never `Bug`;
  `Bug` requires an explicit match (tool-not-found, path-bookkeeping violation,
  internal invariant).
- Enums serialize into the existing v2 string columns via `tag()` / `message()`
  projections per ADR-0023. **No schema change; no migration.**
- Classifiers are free functions at the pipeline boundary:
  `classify_fetch_error(&FetchError) -> ClassifiedFailure`,
  `classify_transcribe_error(&TranscribeError) -> ClassifiedFailure`. The fetcher
  remains a tool adapter and emits no classification (`Acquisition` unchanged;
  the sketch's `Acquisition::Unavailable` idea is dropped — classification is
  policy over stderr, not a transport fact).

### 2. Error-type refinements (absorbed FOLLOWUPS)

- `From<RunError> for FetchError`: split the Spawn/Io collapse. Missing binary /
  fork failure and pipe I/O become distinct variants (e.g. `ToolNotFound`,
  `SystemIo`) classified as `Bug`/`Retryable` respectively — not `NetworkError`.
- `CommandOutcome` gains `signal: Option<i32>` via
  `std::os::unix::process::ExitStatusExt` (cfg-gated) so OOM-kill / SIGSEGV /
  SIGINT are distinguishable.
- `From<AudioDecodeError> for TranscribeError` stops mapping to `Bug`; audio-decode
  failures get a dedicated variant classified `Retryable(TranscribeOther)` pending
  observed evidence to refine further.
- Classifier refuses to treat any error carrying `exit_code == Some(0)` as success
  (pattern guard per the sketch's still-valid instinct).
- `YtDlpFetcher::acquire` internal mappings corrected alongside (FOLLOWUPS T11
  findings 1–2): `create_dir_all` failure stops masquerading as `NetworkError`
  (filesystem → `Bug`-class config/system variant); post-success missing-WAV
  stops masquerading as `ParseError` (tool-contract postcondition violation).
  Finding 3 (WAV-filename glob hardening) stays deferred; finding 4 (`--`
  separator) remains Plan C.
- `Store::claim_next` / `mark_succeeded` inner statements gain `with_context`
  (FOLLOWUPS T10; bundled here per its trigger since this epic touches the
  mutator surface anyway).

### 3. Pipeline dispatch

The two existing failure call sites (`fetch_worker`, `transcribe_worker` in
`src/pipeline/pipelined.rs`; mirrored in `serial.rs`) match on the classifier:

- `Unavailable { reason, .. }` → `mark_terminal_failure(id, worker, reason.tag(), msg)`
  — **first caller** of the Epic 2 mutator; its `in_progress AND claimed_by`
  predicate fits this path exactly.
- `Retryable { kind, .. }` → `mark_retryable_failure(id, worker, kind.tag(), msg)`
  — replaces the placeholder `"Fetch"` / `"Transcribe"` literals.
- `Bug` → worker returns `Err` (unchanged orchestrator-abort semantics).

Stale-claim `Ok(0)` handling is unchanged for all arms.

Since this epic touches `fetch_worker`, the FOLLOWUPS T16 cancellation-latency
fix rides along (its trigger names exactly this condition): wrap
`fetcher.acquire()` in `tokio::select!` against `token.cancelled()`, mirroring
the transcribe-side wrap from `a66d38b`, so cancellation drops the future and
`kill_on_drop` reaps the yt-dlp subprocess instead of waiting out a
300-second timeout.

### 4. `triage` subcommand

`ddp-transcribe triage [--dry-run] [--rate <probes-per-sec>] [--max-attempts <N>]`

- Iterates `failed_retryable` rows. For each, probes
  `https://www.tiktok.com/oembed?url=https://www.tiktok.com/@x/video/<video_id>`
  by shelling out to `curl` through the existing bounded `process::run` infra
  (argv-direct, no shell; explicit timeout; bounded stdout capture per ADR-0021;
  HTTP status extracted via `-w`; body available for future status-code parsing).
  No new HTTP-client dependency. Default rate 1 probe/s.
- Verdict routing, via **two new `Store` mutators** (0006/0023 conventions:
  `Result<usize>`, WHERE-predicated on `status = 'failed_retryable'`):
  - HTTP 400 (dead) → `failed_retryable → failed_terminal`,
    `terminal_reason = "probe_dead"`; `last_retryable_*` preserved for audit.
  - HTTP 200 (alive) → `failed_retryable → pending` **iff
    `attempt_count < max_attempts`** (default 3). On requeue, triage re-runs the
    message classifier over the stored `last_retryable_message` and writes the
    resulting kind tag back to `last_retryable_kind`. This normalizes historical
    rows carrying the Epic 2 placeholder kind (`"Fetch"`) to taxonomy kinds — 
    load-bearing for cookie routing: without it, the ~301 sensitive rows in the
    production DB would need one wasted cookie-less refetch each just to get
    re-tagged by the new pipeline classifier.
  - Probe failure / unexpected status → row untouched (default-cautious).
- Both transitions append `video_events` rows (`triaged_terminal`, `requeued`) —
  unlike the stale sweep, triage is an operator action and must be auditable.
- Output: per-kind census table (0007-style stats struct, input-side counters,
  verb-named fields). This table doubles as the study's **attrition
  documentation** — deleted/private content cannot be recovered and its per-class
  counts are reportable in the paper.
- `--dry-run`: probe and report; no mutations.
- Probing is abstracted behind a small trait (e.g. `ProbeOracle`) so tests inject
  a fake; no network in the test suite.

After triage, the operator re-runs `ddp-transcribe process` to consume the
requeued rows. No automatic in-pipeline retry or backoff exists; triage **is** the
retry executor (this resolves the pre-kickoff review's flag #1: the architecture
docs' "Epic 3 adds retry policy" promise is satisfied by the operator-driven path).

### 5. Cookie-scoped retry for the sensitive class

- `Claim` gains `last_retryable_kind: Option<String>` (extend `claim_next`'s
  SELECT; no schema change — the column exists).
- `process` gains `--cookies-file <path>` (optional).
- `VideoFetcher::acquire` gains a per-request options parameter (e.g.
  `FetchOpts { cookies_file: Option<PathBuf> }`); `FakeFetcher` records received
  opts for assertions.
- The fetch worker sets `cookies_file` **only when** the claim's
  `last_retryable_kind == SensitiveLoginGated.tag()` and the operator provided the
  flag. First attempts (kind `None`) never send cookies.
- Redaction: the cookie file path and cookie-bearing argv never appear in logs,
  error messages, or stderr excerpts.

### 6. Documentation and record corrections

- `state-machine.md`: diagram gains the triage edges (`failed_retryable →
  failed_terminal`, `failed_retryable → pending`); "sink" language updated;
  mutator table extended. `orchestration.md` + `index.md` §4 per the T08
  standing drift rule.
- FOLLOWUPS corrections: "stderr is dropped" claim (stale — ADR-0021 shipped);
  share-link hypothesis marked refuted with the 2026-07-07 evidence; resolved
  Epic 3 entries move to archive at epic close per 0020.

### 7. ADRs (numbers 0033+, verify with `adg list` at drafting time)

1. **Failure taxonomy + inline write-off policy** — evidence-derived variants,
   default-cautious posture, the two write-off classes with their probe
   validation, version pinning (yt-dlp version, probe validation date), the
   `exit_code == 0` guard.
2. **Triage design** — oEmbed as oracle, curl-via-`process::run` transport, rate
   discipline, attempt cap, audit events, external-endpoint drift risk (oEmbed
   behavior re-validated if TikTok changes it).
3. **Cookie policy** — sensitive-retries-only scope, redaction rules, the
   crime/policing research-validity rationale.

### 8. Testing

- **Classifier:** table-driven tests over real stderr messages extracted from
  `ddp-run-export.sqlite` into committed fixtures under
  `tests/fixtures/yt_dlp_stderr/` (one file per message class; provenance noted).
  The hand-labeled fixture IDs from the evidence sessions are preserved in the
  kickoff prompt and FOLLOWUPS.
- **Triage:** integration tests with a fake `ProbeOracle` covering dead/alive/
  unreachable routing, the attempt cap, dry-run non-mutation, kind
  re-classification on requeue (placeholder `"Fetch"` → taxonomy kind), and
  `video_events` writes.
- **Dispatch + cookies:** `pipeline_fakes` suite extended for the three-arm match
  and cookie routing (FakeFetcher asserts opts). The routed `pipeline_fakes.rs`
  refactor (split into `tests/pipeline_fakes/{fakes,serial_tests,fetch_worker_tests,transcribe_worker_tests,pipelined_tests}.rs`)
  and the worker-level-vs-`run_pipelined` test audit execute as part of this
  epic, per their FOLLOWUPS routing.
- Verification command unchanged:
  `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`.

## Out of scope

- Automatic in-pipeline retry / backoff (triage is the retry executor).
- Short-link / URL-form canonicalization (hypothesis refuted).
- Whisper-side taxonomy expansion beyond observed failures (0 transcribe failures
  in 65k videos).
- Cookie support outside the sensitive-retry path; cookie acquisition/rotation
  operational docs beyond the minimum the ADR requires.
- Parsing TikTok API status codes beyond `10231` (the probe supersedes the need;
  revisit only if oEmbed drifts).

## Planning-process discipline (adopted from FOLLOWUPS at kickoff)

Per the "plan-brief library-API drift" cross-epic entry (three independent catches
in Epic 2), plan expansion for this epic adopts the verify-at-write-time
checklist: every library-API claim in a task brief is checked against the
actually-installed crate (`Cargo.lock` resolved version + registry source), and
suggested test designs are hand-traced against production-code semantics before
publication. Applies alongside ADR-0003's deviation-honesty norm.

## Sizing

~9–11 tasks (taxonomy + classifiers; error-type refinements; dispatch rewiring +
T16 cancellation wrap; triage mutators + `with_context` hygiene; triage
subcommand + oracle; cookie plumbing; fixtures + classifier tests;
pipeline_fakes split + audit; docs + ADRs + FOLLOWUPS corrections).

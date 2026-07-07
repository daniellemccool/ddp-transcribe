# ddp-transcribe — Plan B Epic 3: Failure Classification, Triage, Cookie-Scoped Retry

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-adr-drafts.md` … `11-docs-and-close.md`). Open only the task you're working on. Task files are self-contained; do NOT load the spec or other task files into a subagent's context.

**Goal:** Replace Epic 2's placeholder failure kinds with an evidence-derived typed taxonomy, give `mark_terminal_failure` its first caller (inline write-off of two probe-validated dead message classes), add an operator `triage` subcommand that drains dead rows to `failed_terminal` and requeues recoverable ones via the oEmbed probe, and add cookie-scoped retry for the sensitive/login-gated class.

**Architecture:** Classification is policy at the pipeline boundary: free functions `classify_fetch_error` / `classify_transcribe_error` in a new `src/failure.rs` map tool errors to a three-arm `ClassifiedFailure` (`Retryable` / `Unavailable` / `Bug`); the two worker error arms dispatch on it. The fetcher stays a dumb tool adapter. Retry execution is operator-driven: `triage` classifies stored messages (write-off classes → terminal without probing), probes the rest against TikTok's oEmbed endpoint via `curl` through the existing bounded `process::run`, marks dead → `failed_terminal`, requeues alive → `pending` under an attempt cap, then the operator re-runs `process`. Cookies ride only on claims whose `last_retryable_kind` is `SensitiveLoginGated`.

**Tech Stack:** Rust 2021, tokio, rusqlite, clap 4 (all existing). **No new Cargo dependencies.** New *runtime* dependency: `curl` binary on PATH for `triage` only (document in ADR; `process` does not need it). **No schema change; no migration** — v2 columns absorb everything.

**Reference:** Full design in `docs/superpowers/specs/2026-07-07-epic-3-failure-classification-design.md`. Evidence provenance (census, probe n=36, 10/10 server-side re-fetch) in `docs/superpowers/plans/PLAN-B-EPIC-3-KICKOFF-PROMPT.md`. Subagents should not need either — task files are self-contained.

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`. `--test-threads=1` is mandatory on this workstation (thermal); never drop it.
- **Clippy gates:** `unwrap_used` / `expect_used` are **denied** in production code (allowed in tests via existing crate-root `cfg_attr` + per-test-file `allow` headers — new test files must copy the header from an existing `tests/*.rs`).
- **Mutators return `Result<usize>`** (row-change count) per ADR-0006/0023.
- **Stats structs**: input-side counters, verb-named fields, per ADR-0007.
- **Artifact-before-`mark_succeeded`** ordering (ADR-0008) is untouched by this epic; do not reorder.
- **New integration tests** in `Cargo.toml` need `[[test]] required-features = ["test-helpers"]` per ADR-0005.
- **Dead-code hygiene** per ADR-0002: every task states which `#[allow(dead_code)]` it adds or lifts.
- **Commit disclosure** per ADR-0003: any deviation from the task brief is disclosed prominently in the commit message.
- **ADR IDs:** tasks reference the Epic 3 ADRs as **0033 (taxonomy + write-off), 0034 (triage), 0035 (cookie policy)** — the IDs `scripts/adr new` prints in Task 01 are authoritative; if they differ, use the printed IDs everywhere.
- **Write-off string patterns (load-bearing, shared by Tasks 03/07/10):**
  - `"Your IP address is blocked"` → `UnavailableReason::IpBlockedMessage`
  - `"status code 10231"` → `UnavailableReason::VideoNotAvailable10231`

## Task index

| # | File | Deliverable |
|---|------|-------------|
| 01 | `01-adr-drafts.md` | Three ADRs drafted as `proposed` (0033/0034/0035) |
| 02 | `02-error-refinements.md` | `FetchError` variant split, `CommandOutcome.signal`, `AudioDecode` variant |
| 03 | `03-taxonomy-and-classifiers.md` | `src/failure.rs`: enums + classifiers + corpus-seeded table tests |
| 04 | `04-claim-kind-and-context.md` | `Claim.last_retryable_kind` + `with_context` hygiene |
| 05 | `05-triage-mutators.md` | `triage_mark_terminal`, `requeue_retryable`, `list_failed_retryable` |
| 06 | `06-pipeline-fakes-split.md` | `tests/pipeline_fakes/` module split + narration strip + audit |
| 07 | `07-dispatch-rewiring.md` | Three-arm dispatch in both workers + serial; T16 cancellation wrap |
| 08 | `08-cookie-plumbing.md` | `FetchOpts`, `--cookies-file`, kind-gated cookie routing, redaction |
| 09 | `09-probe-oracle.md` | `ProbeOracle` trait + `CurlProber` via `process::run` |
| 10 | `10-triage-subcommand.md` | `triage` CLI command: classify → probe → mutate → census |
| 11 | `11-docs-and-close.md` | Accept ADRs, architecture-doc updates, FOLLOWUPS moves, EPIC-3-CLOSE |

Dependency chain: 01 → (02 → 03) → 04 → 05; 06 independent after 03; 07 needs 03+04+06; 08 needs 04+06+07; 09 needs 02 only; 10 needs 03+05+09; 11 last.

## Spec refinements (verified at plan-writing time; disclose per 0003 if further deviation needed)

1. **Typed error boundary for dispatch.** The spec's dispatch section assumes the worker sees `FetchError`, but `fetch_and_decode` returns `anyhow::Error`, erasing the type the classifier needs. Task 07 introduces `FetchPhaseError { Fetch(FetchError), Decode(AudioDecodeError) }` as its return type. Verified against `src/pipeline/mod.rs` on `main` (`fetch_and_decode` at ~line 195 returns `Result<(Vec<f32>, PathBuf)>` via anyhow today).
2. **Triage message-class fast path.** The spec has triage probe every `failed_retryable` row. Per the operator's write-off ruling, rows whose *stored* `last_retryable_message` matches a write-off pattern go straight to `failed_terminal` without a probe. On the production DB this saves ~3,915 of 7,087 probes. The oEmbed probe covers everything else.
3. **Redaction reuses existing infra.** `CommandSpec.redact_arg_indices` already exists in `src/process.rs` (landed with 0021) — Task 08 uses it rather than building new log-redaction machinery.
4. **`--rate` flag** is `f64` probes-per-second (default `1.0`), implemented as an inter-probe `tokio::time::sleep(Duration::from_secs_f64(1.0 / rate))`.

## Cross-cutting context subagents may need

- **Placeholder kinds being replaced:** `"Fetch"` (`src/pipeline/pipelined.rs:204`), `"Transcribe"` (`src/pipeline/pipelined.rs:426`), `"FetchOrTranscribe"` (`src/pipeline/serial.rs:73`).
- **`mark_terminal_failure`** exists on `Store` (`src/state/mod.rs:~460`), `#[allow(dead_code)]`, predicate `status='in_progress' AND claimed_by=?`. Task 07 lifts the suppression by adding the first caller.
- **The production evidence** (for doc-comment citations): "IP blocked" 10/10 dead, "10231" 5/5 dead, "no permission" 5/5 dead, "no data blocks" 10/10 alive + 10/10 re-fetch OK from workspace egress, sensitive 5/5 alive; validated 2026-07-06/07 against live TikTok oEmbed.
- **Historical-DB shape** triage must handle: rows with placeholder kind `"Fetch"` and full yt-dlp stderr in `last_retryable_message`; all `attempt_count = 1`.

## What this epic deliberately omits

- Automatic in-pipeline retry/backoff (triage is the retry executor).
- URL canonicalization / short-link resolution (hypothesis refuted; Plan C).
- Whisper-side taxonomy beyond observed failures (0 transcribe failures in 65k).
- Cookie use outside `SensitiveLoginGated` retries; cookie acquisition ops docs beyond the ADR minimum.
- TikTok API status-code parsing beyond the `10231` message match.
- `run_serial` retirement decision (Epic 5, per 0002 note in `src/pipeline/mod.rs`).

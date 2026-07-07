# Plan B Epic 3 — Epic Close

**Branch:** `feat/epic-3-failure-triage`
**Status:** all 11 tasks complete. Failure classification (ADR 0033), operator triage (ADR 0034), and cookie-scoped retry (ADR 0035) shipped; ADRs accepted at close.

## What landed

| Task | Commit(s) | Subject |
|---|---|---|
| 01 | `17ee077` | ADR slate drafted as proposed (0033 taxonomy+write-off, 0034 triage, 0035 cookie policy) |
| 02 | `9974d69` + `c7cb3f9` | Error refinements: split `RunError` mapping (`ToolNotFound`/`SystemIo`), `signal` capture, `AudioDecode` + `WorkDirCreate`/`MissingOutput` variants |
| 03 | `8000167` | `src/failure.rs`: evidence-derived taxonomy + classifiers, corpus-seeded table tests |
| 04 | `cc7782f` + `2098b60` | `Claim.last_retryable_kind` snapshot (cookie-routing input); `with_context` hygiene |
| 05 | `f760fa7` | Triage mutators: `triage_mark_terminal`, `requeue_retryable` (capped, kind write-back), `list_failed_retryable`; `triaged_terminal`/`requeued` events |
| 06 | `0dd9707` | `tests/pipeline_fakes/` split into per-concern modules; narration strip; worker-test audit |
| 07 | `50a1db0` + `50c4386` | Three-arm classifier dispatch in both workers + serial; first `mark_terminal_failure` caller; T16 fetch cancellation wrap |
| 08 | `73de244` | Cookie plumbing: `--cookies-file`, kind-gated `cookie_opts_for`, argv+stderr redaction |
| 09 | `0dfad55` | `ProbeOracle` + `CurlProber`: oEmbed liveness oracle via bounded curl subprocess |
| 10 | `b39ff5b` + `5c837bb` | `triage` subcommand: message-class write-off, probe, capped requeue, attrition census |
| 11 | this commit | ADRs 0033/0034/0035 accepted; architecture docs revised; FOLLOWUPS archived with SHAs (one entry split → Epic 5 / Plan C); this close doc |

Verification at close: `cargo fmt` clean, `clippy --all-targets -D warnings` clean, `cargo test --features test-helpers -- --test-threads=1` green, `adg validate` clean.

## Operator runbook — production DB (7,087 failed_retryable from the 65k run)

1. `ddp-transcribe triage --dry-run` — probe + census, zero mutations. Expect ~3,915 message-class write-offs, remainder probed at `--rate` (default 1/s, so budget ~1 h).
2. Review the census (it is the study's attrition documentation): terminal counts by reason, requeue counts by kind, `kept_unreachable` (probe unreachable — rerun later), `kept_capped`.
3. `ddp-transcribe triage` — execute: dead → `failed_terminal` (audited as `triaged_terminal`), alive → `pending` with normalized kind (audited as `requeued`; cap: `--max-attempts`, default 3).
4. `ddp-transcribe process --cookies-file <netscape-cookies.txt>` — refetch requeued rows; cookies ride only on `SensitiveLoginGated` claims (~300 fetches of account exposure, ADR 0035).
5. Expected recovery ≈ +2,400 videos: 87.5% → ~91.5% of the 65k corpus.

## Deferred / open

- Worker-level test replacement candidates (audit verdicts inline in `tests/pipeline_fakes/`) — opportunistic; `run_serial` retirement decision → Epic 5.
- `YtDlpFetcher` output-filename coupling → Epic 5 (FOLLOWUPS split); yt-dlp argv `--` separator → Plan C (split).
- `FfprobePostprocess`-class investigation — only if rows of that kind survive triage + refetch.

## Next

Epic 4 (operator-facing commands: `status`, ADR 0017 done-contract, timestamps) — sketch at `docs/superpowers/plans/2026-05-12-plan-b/EPIC-4-SKETCH.md`.

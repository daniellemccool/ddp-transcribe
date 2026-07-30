# Epic 5b — Plan B close-out: thin-bin restructure, requeue-failures, hygiene sweep → v0.4.0

Design spec, approved 2026-07-30 (brainstorm with operator); revised same day
after external advisor review (all findings verified against the codebase)
and three operator rulings. Feeds the implementation plan under
`docs/superpowers/plans/`.

## Goal and definition of done

Epic 5b closes Plan B. At epic end, every Plan-B-scope FOLLOWUPS entry is
resolved (archived with its resolving SHA) or explicitly re-routed to Plan C
with rationale. The epic produces a **FOLLOWUPS disposition matrix** early
(see Phase 3) so that criterion is mechanically auditable, and ends with the
**v0.4.0** tag + version bump per ADR-0043.

**Deployment context:** releases, tags, and version bumps continue normally.
Only the currently-running campaign workspace stays on v0.3.0 — the SRC
catalog `pipeline_git_ref` is not moved by this epic. The paused older
workspace can be updated for live feedback runs (e.g. the CUDA smoke below);
the live campaign machine is never touched.

**Housekeeping note:** `feat/perf-tweaks` is fully merged (tip is an
ancestor of main) — no conflict risk; the stale worktree/branch can be
cleaned up at any time. The stale `plan-b-epic-4b` worktree likewise, after
the same ancestry check.

## Approach

Restructure-first, three phases. No second module-root restructuring after
Phase 1 — later phases touch files again, but always on the final crate
shape. (Alternatives considered: bundles-first — rejected because the
dead-code-allow purge is impossible before unification; per-subsystem
interleave — rejected because the module declarations are global.)

## Phase 1 — unify on lib (thin bin / fat lib)

Predetermined outcome (operator decision 2026-07-30). Runtime impact nil for
this workload (hot paths are whisper.cpp inference, yt-dlp subprocesses,
SQLite; Rust glue runs at per-video frequency); `lto = "thin"` in
`[profile.release]` removes the cross-crate-inlining question, and a release
build joins the gate so it is actually exercised.

1. **ADR first.** New lean record: thin bin / fat lib with a minimal public
   façade. Amend ADR-0002: with double compilation gone, the policy's
   backstop shifts to visibility narrowing (`pub(crate)` default +
   `unreachable_pub` warn).
2. **The unification.** `lib.rs` becomes the single module root (the five
   bin-only modules — `cli`, `config`, `status`, `backfill`,
   `metadata_loader` — declared there; files do not physically move, so ADR
   `applies_to` globs and doc path references stay valid). **Public façade**
   (binary-facing API is part of the public surface — a package binary is a
   separate crate and cannot see `pub(crate)`):
   - `pub use cli::Cli` (re-export for parsing);
   - `pub async fn dispatch(cli: Cli) -> Result<CommandExit>` in a lib
     `commands` module, where `CommandExit` carries the process-exit
     semantics currently inlined in main (`exit(3)` when a batch claims
     zero videos, `exit(1)` on a failed pause-safety verify);
   - `main.rs` owns argument parsing, tracing init, error rendering, and
     the final `std::process::exit` — the library never calls `exit`.
   `run_serial` is **retained** in Phase 1 (deleting it during a
   behavior-preserving phase would change API/test scope; its fate is a
   Phase-3 disposition-matrix row).
3. **Visibility + allow purge.** Default `pub(crate)`; `pub` = the façade
   plus what `tests/` imports; test-only scaffolding stays behind
   `test-helpers` (ADR-0005). Add `unreachable_pub` warn. Delete the
   dead-code allows the double compilation caused (46 today); re-justify or
   delete the rest per amended ADR-0002.

**Acceptance evidence (behavior-preserving):** unchanged integration suite;
inline unit tests stop running twice — measured expectation **345 → 261**
runnable tests (84 duplicated library unit tests today; exact census
re-verified with `cargo test -- --list` at plan time); clippy clean at the
narrowed visibility; release build green.

## Phase 2 — requeue-failures + fetch hardening

On the new tree. TDD throughout.

### `requeue-failures` (operator ruling 2026-07-30; supersedes the sketched `requeue-retryables`)

ADR-0036 remains the normal retry authority: `batch::run_sweep` already
re-adjudicates parked retryables each batch start, and fresh cookies alone
are handled by that path for rows under the cap. What no automatic mechanism
can do is restore rows blocked by the lifetime cap or already terminalized
after an external condition materially changed (cookie-gated cohort is the
live example); without a command, the operator's escape hatch is manual SQL.

**New ADR (likely 0045) + amendment to ADR-0036.** The carve-out, verbatim
target: *"ADR-0036 remains the normal retry authority. An operator may
explicitly restore failed rows to pending after an external condition has
materially changed. This is a forensic, default-deny override of
eligibility, not an alternate classifier or retry scheduler; the subsequent
fetch remains the liveness oracle."*

Command contract (binding content for the ADR and implementation):

- **Name:** `requeue-failures` (`requeue-retryables` would be misleading
  once terminal rows are eligible).
- **Default eligibility:** `failed_retryable`. Terminal rows require an
  explicit `--include-terminal` **plus at least one narrowing selector**;
  `--all` means all retryables, never terminals.
- **Default-deny:** a bare invocation is an error — at least one selector
  or an explicit `--all` is required.
- **Selectors:** `--error-kinds <K,K,…>`, `--max-attempts <N>` (restored
  from the sketch: skip rows with `attempt_count >= N`), `--older-than
  <DUR>`, `--max <N>` (deterministic ordering specified in the ADR),
  `--dry-run`.
- **`--older-than` source (operator ruling):** `last_failure_at :=
  MAX(video_events.at)` over the failure-event allowlist —
  `failed_retryable`, `failed_terminal`, `retry_requeued`, `cookie_parked`
  (exact strings verified against the event-insert sites at plan time).
  Administrative transitions (e.g. `swept_terminal`, `swept_stale`) never
  reset the clock. `--older-than D` matches `last_failure_at < now - D`
  with one `now` per command invocation; rows with no qualifying events do
  not match. Tests pin the allowlist, including that `swept_terminal` does
  not reset the clock. `videos.updated_at` is not touched (preserves the
  INSERT-OR-IGNORE row-count contract, ADR-0006).
- **State transition:** eligible rows → `pending`. `attempt_count` is never
  reset or decremented — the command grants eligibility for another claim
  while preserving history. `last_retryable_*` and terminal fields are
  retained.
- **Forensics:** one `operator_requeued` event per row carrying prior
  status, prior kind/reason, attempt count, and operator attribution
  (hostname-derived, consistent with 5a).
- **Sequencing:** the transition intentionally happens before — and
  therefore bypasses — the start-of-batch sweep; after the forced claim,
  ordinary ADR-0036 behavior resumes. Documented: additional *automatic*
  attempts require a `process --retries` lifetime cap above the row's
  existing `attempt_count`.
- **Contract details the ADR must also fix:** transaction boundary,
  dry-run output, zero-match exit behavior, case/custom-label matching for
  `--error-kinds`, deterministic `--max` ordering.

**`reset-stale-claims` is dropped as superseded** (operator ruling
2026-07-30): the startup stale-claim sweep recovers claims at every process
start with per-row `swept_stale` forensics since 5a. Its FOLLOWUPS entry is
archived citing that supersession.

### Fetch hardening bundle

- `YtDlpFetcher::acquire` output-discovery hardening with explicit
  **freshness + uniqueness rules**: the fetcher reuses a persistent
  per-video work directory, so discovery must use one of (a) a fresh
  per-attempt directory, (b) pre-run cleanup plus exactly-one-result
  validation, or (c) parsing yt-dlp's reported final path — chosen at plan
  time; stale-file, zero-result, and multiple-result cases all tested.
- `FetchOpts` derived `Debug` cookie-path redaction; `scrub_cookie_path`
  empty-path guard.
- Multi-fetcher provenance: **verify-and-archive, not implementation** —
  `VideoFetcher::name`/`Transcriber::name` and pipelined item stamping
  already exist; confirm nothing in the FOLLOWUPS body remains, archive
  with evidence.

### ADR-0013 backend assertion (implementation task, not audit)

The accepted-but-unimplemented invariant (ADR-0013 Guidance; cross-epic
FOLLOWUPS audit 2026-05-18): wire whisper.cpp's init log through the
`whisper_log_set` callback bridge (global-state design for the callback),
assert the expected backend at engine construction, emit the
`tracing::info!` backend/device line, `cfg(feature = "cuda")`-gate the
assertion per the FOLLOWUPS entry. Tests cover the mismatch path; a CUDA
build + runtime smoke runs on this workstation's GPU (or the paused SRC
workspace if needed). Review rejects softening the contract to a warning
(per the ADR itself).

## Phase 3 — sweeps, audits, close-out

- **FOLLOWUPS disposition matrix (early task):** a table over every Epic 5
  and cross-epic entry — fix / verify-and-archive / accepted-archive /
  re-route Plan C / out of Plan-B scope — making the DoD mechanically
  auditable. Known judgment rows the matrix must decide (surfaced to the
  operator, never silently resolved): ingest-ledger basename-collision and
  1s-fingerprint-resolution entries (contain design choices); the accepted
  tmp-sweep TOCTOU entry; `upsert_metadata_raw` claim-guard (resolve vs
  Plan C); `run_serial` retire-or-keep; T9 `updated_at` (now decoupled from
  requeue — resolves as rename vs document-as-frozen); the closed-reply
  logging fix (its FOLLOWUPS body asks for a video/request id the transcribe
  work item may not carry — verify the item's fields at plan time).
- **Sync-IO sweep (restored to sketch scope):** an audit + policy task, not
  cosmetic cleanup. Audit synchronous I/O inside async paths across ingest,
  transcription, pipeline, and artifacts (current examples: synchronous WAV
  decode, durable writes/fsync, yt-dlp work-directory operations, rusqlite
  under tokio tasks); a lean ADR records the policy — which operations move
  to `tokio::fs`/`spawn_blocking` and which deliberately stay synchronous,
  with rationale. The `walk_recursive`/`cleanup_tmp_files`/shard-comment
  polish rides this task.
- **Bundle tasks** (each from its FOLLOWUPS body): state/mod.rs hygiene
  (post-4a renames, `tx.commit` contexts, assertions, `conn_mut` deletion,
  `pragma_string` → `pub(crate)`, `read_meta` OptionalExtension,
  `shard_dir` deletion); status polish + test-debt (Epic 4b list);
  test-hardening bundles (Epic 3 list; v0.3.1 CLI list); T5-Epic1
  closed-reply logging fix (per its matrix row); PR #23 ingest-ledger
  hardening (per its matrix rows).
- **Verify-then-archive audit:** `--whisper-model` global flag (likely
  resolved by v0.3.1 clap-globals), `From<RunError> for FetchError` mapping
  (Epic 3 leftover), plus whatever the matrix marks verify-and-archive.
- **Close-out:** FOLLOWUPS lifecycle per ADR-0020; documentation pass
  covering README, the operations runbook, state-machine/orchestration
  deepdives, and CLI examples (not only bin/lib-shape statements);
  RELEASE-NOTES-v0.4.0; tag + bump per ADR-0043.

## Testing and review conventions

- Gate everywhere: `cargo fmt && cargo clippy --all-targets -- -D warnings
  && cargo test --features test-helpers -- --test-threads=1`
  (`--test-threads=1` is a workstation invariant), **plus a release build**
  (`cargo build --release`) so thin-LTO is exercised, plus the CUDA
  build/runtime smoke for the 0013 task.
- Phase 1 evidence: unchanged integration suite + the 345→261 census +
  clippy at narrowed visibility. Phases 2–3: TDD per task.
- `requeue-failures` test coverage checklist: command help, missing DB,
  dry-run non-mutation, event emission + operator attribution, filter
  conjunction, deterministic `--max` ordering, zero-match exit behavior,
  exit codes, the `--older-than` allowlist pins.
- Execution: subagent-driven development, per-task brief files (ADR-0001),
  three-tier review per task (implementer → Sonnet spec/quality reviewer →
  codex-advisor through tier 2, per ADR-0018/0019), final whole-branch
  review on the most capable model, worktree isolation. All ADR work via
  `adg` / write-lean-adr, never by hand.

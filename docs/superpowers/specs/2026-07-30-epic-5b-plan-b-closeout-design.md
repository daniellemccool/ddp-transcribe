# Epic 5b — Plan B close-out: thin-bin restructure, operator commands, hygiene sweep → v0.4.0

Design spec, approved 2026-07-30 (brainstorm with operator). Feeds the
implementation plan under `docs/superpowers/plans/`.

## Goal and definition of done

Epic 5b closes Plan B. At epic end, every Plan-B-scope FOLLOWUPS entry is
resolved (archived with its resolving SHA) or explicitly re-routed to Plan C
with rationale — the Plan-B "done" criterion from `EPIC-5-SKETCH.md`. The
epic ends with the **v0.4.0** tag + version bump per ADR-0043.

**Deployment context:** the currently-running campaign workspace stays on
v0.3.0 and will not consume anything from this epic — but releases, tags,
and version bumps continue normally. The SRC catalog `pipeline_git_ref` is
not moved as part of this epic. If operator feedback requires running new
code on real infrastructure, the paused older workspace can be updated;
the live campaign machine is never touched.

**Pre-flight note:** the long-lived `feat/perf-tweaks` branch/worktree will
conflict with the Phase-1 restructure. Land or rebase it before Phase 1
starts, or accept the conflicts afterwards — disposition is the operator's
call at plan kickoff.

## Approach

Restructure-first, three phases. Every later diff lands on the final module
tree; nothing is touched twice. (Alternatives considered: bundles-first —
rejected because the dead-code-allow purge is impossible before unification
and nearly every bundle touches files the restructure re-declares;
per-subsystem interleave — rejected because the bin/lib module declarations
are global and not separable per subsystem.)

## Phase 1 — unify on lib (thin bin / fat lib)

Predetermined outcome (operator decision 2026-07-30, after reviewing the
history, idiomatic-Rust, and runtime analysis): single library crate, thin
binary. Runtime impact is nil for this workload — the hot paths are
whisper.cpp inference (native, untouched), yt-dlp subprocesses, and SQLite;
Rust glue runs at per-video frequency. `lto = "thin"` removes even the
theoretical cross-crate-inlining question.

1. **ADR first.** New lean record: thin bin / fat lib as the crate shape.
   Amend ADR-0002: the dead-code policy's "suppress ahead of consumers"
   rationale weakens once the double compilation is gone; the new backstop
   is visibility narrowing (`pub(crate)` default + `unreachable_pub` warn).
   ADR-0002's own Context anticipates exactly this supersession.
2. **The unification.** `lib.rs` becomes the single module root; the five
   bin-only modules (`cli`, `config`, `status`, `backfill`,
   `metadata_loader`) are declared there. Files do not physically move —
   ADR `applies_to` globs and architecture-doc path references stay valid.
   `main.rs` drops all 18 `mod` declarations and shrinks to parse-args +
   dispatch; the dispatch `match` arms move into a lib `commands` module
   as per-command functions. Behavior-preserving: acceptance
   evidence is the unchanged integration suite plus the documented
   inline-test-count drop (inline `#[cfg(test)]` tests currently run twice
   — once per compilation — and will run once).
3. **Visibility + allow purge.** Default `pub(crate)`; keep `pub` only what
   `tests/` imports; test-only scaffolding stays behind the `test-helpers`
   feature (ADR-0005). Add `unreachable_pub` warn. Delete every
   `#[allow(dead_code)]` that existed because of the double compilation
   (46 total today); re-justify or delete the remainder per amended
   ADR-0002. Add `lto = "thin"` to `[profile.release]`.

## Phase 2 — operator commands + fetch hardening

On the new tree. TDD throughout.

- **`requeue-retryables` subcommand** per `EPIC-5-SKETCH.md` and ADR-0033
  (default-deny: every widening filter is explicit): `--older-than <DUR>`
  (gates on `videos.updated_at`), `--error-kinds <K,K,…>`
  (`last_retryable_kind` match), `--max <N>`, `--dry-run`.
  This resolves the deferred T9 `updated_at` decision as **option (b)**:
  `--older-than` consumes `updated_at`, so `upsert_video` and the status
  mutators must keep it genuinely updated. The task verifies which mutators
  already maintain it and closes the gaps.
- **`reset-stale-claims` subcommand**: manual operator escape hatch,
  distinct from the startup sweep. `--max-age <DUR>` REQUIRED (no default —
  forces a deliberate choice), `--dry-run`. Post-5a forensics consistency:
  writes per-row events like the startup sweep (same provenance fields:
  prior claimant, claimed_at, threshold), operator-attributed `worker_id`,
  so manual recovery is as explainable in the DB as automatic recovery.
  Event semantics must stay consistent with amended ADR-0024.
- **Fetch-hardening bundle**: `YtDlpFetcher::acquire` coupling to the
  `{video_id}.wav` output filename; `FetchOpts` derived `Debug` cookie-path
  redaction; `scrub_cookie_path` empty-path guard; the sketch's
  multi-fetcher provenance cleanup (details from the FOLLOWUPS body at plan
  time).

## Phase 3 — sweeps, audits, close-out

- **Bundle tasks** (each from its FOLLOWUPS body):
  - sync-IO sweep: `ingest::walk_recursive` polish, `cleanup_tmp_files`
    polish, `shard_distributes_uniformly` comment; PR #23 ingest
    file-ledger hardening bundle rides here.
  - state/mod.rs hygiene: post-4a sweep-mutator renames, bare
    `tx.commit()?` contexts, attempt_count assertion, defensive
    claimed_by/claimed_at clearing, capped-requeue no-event assertion;
    `Store::conn_mut` deletion, `pragma_string` → `pub(crate)`,
    `read_meta` OptionalExtension refactor, `shard_dir` deletion.
  - status polish + test-debt (Epic 4b list): `render_event_detail_inline`
    fallback, missing fixtures, `run_verify` `e.ok()` miscount,
    `--respondent-id` typo zero-fill, mid-file `use`.
  - test-hardening bundles: Epic 3 list (signal-capture spawn+kill,
    `classify_message` precedence/case, `transcribe_worker` kind-string
    end-to-end) and v0.3.1 CLI list (global-flag value-propagation
    assertions, `backfill-metadata` `conflicts_with`, PATH-shim dry-run
    test, `statuses()` claim/attempt columns).
  - T5-Epic1: worker-side closed-reply path logging fix.
- **Verify-then-archive audit**: entries that may already be resolved are
  verified against the code, then archived with evidence or fixed —
  `--whisper-model` global flag (likely resolved by v0.3.1 clap-globals),
  `From<RunError> for FetchError` mapping (Epic 3 leftover check), the
  0013 cfg-gate audit note.
- **Judgment calls surface to the operator, never silently resolved**: the
  `upsert_metadata_raw` claim-guard entry (resolve vs re-route to Plan C),
  plus anything the audit finds ambiguous.
- **Close-out**: FOLLOWUPS lifecycle per ADR-0020; architecture doc-set
  touch-ups (small — file paths don't move; index.md §4/§6 and any
  bin/lib-shape statements); RELEASE-NOTES-v0.4.0; tag + bump per
  ADR-0043. `run_serial` downcast-asymmetry entry resolves here or is
  mooted if `run_serial` retires during the restructure — explicit
  disposition required either way.

## Testing and review conventions

- Gate everywhere: `cargo fmt && cargo clippy --all-targets -- -D warnings
  && cargo test --features test-helpers -- --test-threads=1`
  (`--test-threads=1` is a workstation invariant).
- Phase 1 is behavior-preserving: evidence = unchanged integration suite,
  documented inline-test-count delta, clippy clean at the narrowed
  visibility. Phases 2–3: TDD per task; bundles fix tests as their
  FOLLOWUPS bodies specify.
- Execution: subagent-driven development, per-task brief files (ADR-0001),
  three-tier review per task (implementer → Sonnet spec/quality reviewer →
  codex-advisor through tier 2, per ADR-0018/0019), final whole-branch
  review on the most capable model, worktree isolation.

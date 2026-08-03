# Architectural decisions

This index is generated from the ADR frontmatter — do not edit by hand.
Load the ADR(s) whose filename matches the area you are touching.

## Index

### Process

- [0001 — Split plans per task](./0001-implementation-plans-are-split-into-per-task-files.md)
- [0003 — Reserve real TDD for deviations](./0003-test-discipline-is-tiered-batch-test-first-for-plan-prescribed-code-real-tdd-for-deviations.md)
- [0018 — Route codex via the reviewer](./0018-task-reviews-are-three-tier-codex-advisor-is-called-by-the-reviewer-never-the-orchestrator.md)
- [0019 — Cap reports, restart per phase](./0019-subagent-reports-are-capped-and-structured-controllers-restart-at-phase-boundaries.md)
- [0020 — Mark hypotheses unverified](./0020-followups-is-a-scope-index-over-per-epic-files-hypotheses-are-marked-unverified.md)

### Code conventions

- [0002 — Justify every allow(dead_code)](./0002-dead-code-is-suppressed-with-allow-dead-code-plus-a-justification-comment.md)
- [0005 — Gate test items via test-helpers](./0005-integration-tests-reach-library-test-items-via-the-test-helpers-cargo-feature.md)
- [0007 — Count the input side in stats](./0007-stats-structs-count-the-input-side-with-verb-named-parallel-counters.md)
- [0045 — Keep the binary thin, library fat](./0045-the-crate-is-a-fat-library-with-a-thin-binary-behind-a-minimal-public-facade.md)

### Artifacts

- [0004 — Shard by the last two id digits](./0004-transcript-output-shards-by-the-last-two-digits-of-the-video-id.md)
- [0008 — Write artifacts before mark_succeeded](./0008-artifacts-are-durable-on-disk-before-mark-succeeded.md)
- [0010 — Pass confidence signals raw](./0010-raw-signals-passes-whisper-confidence-signals-through-raw.md)

### State machine

- [0006 — Return row counts from mutators](./0006-store-mutators-return-result-usize-row-change-counts.md)
- [0022 — Migrate only via the subcommand](./0022-schema-version-hard-fails-at-store-open-migration-is-an-explicit-cli-subcommand.md)
- [0023 — Keep string kinds and claim guard](./0023-failure-mutators-take-string-kinds-and-keep-the-claim-guard.md)
- [0024 — Sweep stale claims blind, no bump](./0024-stale-claim-sweep-recovers-rows-blind-no-validation-no-attempt-bump.md)
- [0036 — Let the re-fetch judge liveness](./0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)
- [0040 — Move in_window only via recompute-window](./0040-analysis-window-is-computed-at-ingest-recompute-window-is-the-only-flag-mutator.md)
- [0046 — Default-deny requeue-failures](./0046-requeue-failures-is-a-forensic-default-deny-override-of-retry-eligibility.md)

### Whisper engine

- [0009 — Bump whisper-rs with upstream](./0009-whisper-cpp-embeds-via-pinned-whisper-rs-crate-and-upstream-commit-bump-together.md)
- [0012 — Cancel per-request, never engine-wide](./0012-cancellation-is-per-request-an-arc-atomicbool-polled-by-the-abort-callback.md)
- [0013 — Assert the GPU backend at startup](./0013-startup-asserts-the-gpu-backend-and-logs-the-device-name.md)
- [0015 — Never call whisper_full_parallel](./0015-no-whisper-full-parallel.md)
- [0016 — Keep the engine API stable](./0016-engine-api-stays-stable-across-single-and-multi-state-internals.md)

### Audio

- [0014 — Validate 16k mono f32 at decode](./0014-audio-input-invariant-float32-pcm-16-khz-mono-validated-at-decode.md)

### Subprocess

- [0021 — Bound subprocess capture](./0021-subprocess-output-capture-is-bounded-by-construction.md)

### Orchestration

- [0025 — Cancel, drain, then shut the engine](./0025-shutdown-order-is-load-bearing-cancel-drain-the-joinset-then-shut-down-the-engine.md)
- [0026 — Exit on claim_next None, never poll](./0026-workers-drain-and-exit-on-claim-next-none-no-polling.md)
- [0027 — Run 3 fetchers into 1 transcriber](./0027-orchestrator-topology-3-fetch-workers-feed-1-transcribe-worker-over-a-capacity-2-channel.md)
- [0041 — Keep status strictly read-only](./0041-status-is-the-read-only-operator-surface-the-archived-done-contract-lives-behind-verify.md)
- [0044 — Keep checkpoint hooks non-fatal](./0044-in-run-checkpointing-is-an-operator-supplied-hook-that-can-never-abort-the-run.md)
- [0047 — Wrap unbounded IO in spawn_blocking](./0047-blocking-io-on-the-worker-hot-path-runs-on-spawn-blocking-inline-only-when-nothing-can-be-starved.md)

### Failure classification

- [0033 — Classify on evidence, never message text](./0033-failure-classes-are-evidence-derived-message-text-lies-about-causes.md)
- [0037 — Snapshot the policy table per batch](./0037-classification-is-an-operator-editable-toml-table-snapshotted-per-batch.md)

### Fetcher

- [0035 — Send cookies on gated retries only](./0035-cookies-ride-only-requires-cookie-retries-with-argv-redaction.md)
- [0038 — Keep frugal for NoDataBlocks only](./0038-fetch-format-is-download-first-the-frugal-selector-runs-only-on-nodatablocks-retries.md)
- [0042 — Re-parse metadata, never re-fetch](./0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md)

### Ingest

- [0039 — Treat timestamps as UTC-assumed only](./0039-ddp-watch-history-timestamps-are-treated-as-utc-documentary-only-and-empirically-unresolved.md)

### Operations

- [0043 — Deploy only pinned release tags](./0043-production-deployments-build-pinned-release-tags-promotion-is-an-explicit-tag-and-bump.md)

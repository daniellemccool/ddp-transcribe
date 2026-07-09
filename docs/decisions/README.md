# Architectural decisions

This index is generated from the ADR frontmatter — do not edit by hand.
Load the ADR(s) whose filename matches the area you are touching.

## Index

### Code conventions

- [0002 — Dead code is suppressed with #[allow(dead_code)] plus a justification comment](./0002-dead-code-is-suppressed-with-allow-dead-code-plus-a-justification-comment.md)
- [0005 — Integration tests reach library test items via the test-helpers Cargo feature](./0005-integration-tests-reach-library-test-items-via-the-test-helpers-cargo-feature.md)
- [0007 — Stats structs count the input side, with verb-named parallel counters](./0007-stats-structs-count-the-input-side-with-verb-named-parallel-counters.md)

### Artifacts

- [0004 — Transcript output shards by the last two digits of the video id](./0004-transcript-output-shards-by-the-last-two-digits-of-the-video-id.md)
- [0008 — Artifacts are durable on disk before mark_succeeded](./0008-artifacts-are-durable-on-disk-before-mark-succeeded.md)
- [0010 — raw_signals passes whisper confidence signals through raw](./0010-raw-signals-passes-whisper-confidence-signals-through-raw.md)

### State machine

- [0006 — Store mutators return Result<usize> row-change counts](./0006-store-mutators-return-result-usize-row-change-counts.md)
- [0022 — Schema version hard-fails at Store::open; migration is an explicit CLI subcommand](./0022-schema-version-hard-fails-at-store-open-migration-is-an-explicit-cli-subcommand.md)
- [0023 — Failure mutators take string kinds and keep the claim guard](./0023-failure-mutators-take-string-kinds-and-keep-the-claim-guard.md)
- [0024 — Stale-claim sweep recovers rows blind: no validation, no attempt bump](./0024-stale-claim-sweep-recovers-rows-blind-no-validation-no-attempt-bump.md)
- [0036 — Retry is in-pipeline: capped failure-time requeue; the re-fetch is the liveness oracle](./0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)

### Whisper engine

- [0009 — whisper.cpp embeds via pinned whisper-rs; crate and upstream commit bump together](./0009-whisper-cpp-embeds-via-pinned-whisper-rs-crate-and-upstream-commit-bump-together.md)
- [0012 — Cancellation is per-request: an Arc<AtomicBool> polled by the abort callback](./0012-cancellation-is-per-request-an-arc-atomicbool-polled-by-the-abort-callback.md)
- [0013 — Startup asserts the GPU backend and logs the device name](./0013-startup-asserts-the-gpu-backend-and-logs-the-device-name.md)
- [0015 — No whisper_full_parallel](./0015-no-whisper-full-parallel.md)
- [0016 — Engine API stays stable across single- and multi-state internals](./0016-engine-api-stays-stable-across-single-and-multi-state-internals.md)

### Audio

- [0014 — Audio input invariant: float32 PCM 16 kHz mono, validated at decode](./0014-audio-input-invariant-float32-pcm-16-khz-mono-validated-at-decode.md)

### Subprocess

- [0021 — Subprocess output capture is bounded by construction](./0021-subprocess-output-capture-is-bounded-by-construction.md)

### Orchestration

- [0025 — Worker supervision: JoinSet + CancellationToken; shutdown order is load-bearing](./0025-worker-supervision-joinset-cancellationtoken-shutdown-order-is-load-bearing.md)
- [0026 — Workers drain and exit on claim_next None — no polling](./0026-workers-drain-and-exit-on-claim-next-none-no-polling.md)
- [0027 — Orchestrator topology: 3 fetch workers feed 1 transcribe worker over a capacity-2 channel](./0027-orchestrator-topology-3-fetch-workers-feed-1-transcribe-worker-over-a-capacity-2-channel.md)

### Failure classification

- [0033 — Failure classes are evidence-derived; message text lies about causes](./0033-failure-classes-are-evidence-derived-message-text-lies-about-causes.md)
- [0037 — Classification is an operator-editable TOML table, snapshotted per batch](./0037-classification-is-an-operator-editable-toml-table-snapshotted-per-batch.md)

### Fetcher

- [0035 — Cookies ride only SensitiveLoginGated retries, with argv redaction](./0035-cookies-ride-only-sensitivelogingated-retries-with-argv-redaction.md)
- [0038 — Fetch format is download-first; the frugal selector runs only on NoDataBlocks retries](./0038-fetch-format-is-download-first-the-frugal-selector-runs-only-on-nodatablocks-retries.md)

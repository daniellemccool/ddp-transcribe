# Architectural decisions

This index is generated from the ADR frontmatter — do not edit by hand.
Load the ADR(s) whose filename matches the area you are touching.

## Index

### State machine

- [0006 — Store mutators return Result<usize> row-change counts](./0006-store-mutators-return-result-usize-row-change-counts.md)
- [0023 — Failure mutators take string kinds and keep the claim guard](./0023-failure-mutators-take-string-kinds-and-keep-the-claim-guard.md)
- [0036 — Retry is in-pipeline: capped failure-time requeue; the re-fetch is the liveness oracle](./0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)

### Audio

- [0014 — Audio input invariant: float32 PCM 16 kHz mono, validated at decode](./0014-audio-input-invariant-float32-pcm-16-khz-mono-validated-at-decode.md)

### Subprocess

- [0021 — Subprocess output capture is bounded by construction](./0021-subprocess-output-capture-is-bounded-by-construction.md)

### Orchestration

- [0025 — Worker supervision: JoinSet + CancellationToken; shutdown order is load-bearing](./0025-worker-supervision-joinset-cancellationtoken-shutdown-order-is-load-bearing.md)

### Fetcher

- [0035 — Cookies ride only SensitiveLoginGated retries, with argv redaction](./0035-cookies-ride-only-sensitivelogingated-retries-with-argv-redaction.md)

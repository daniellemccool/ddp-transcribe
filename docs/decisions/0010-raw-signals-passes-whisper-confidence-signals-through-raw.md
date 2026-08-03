---
status: accepted
date: "2026-05-12"
category: Artifacts
applies_to:
    - src/output/artifacts.rs
priority: default
---

# Pass confidence signals raw

## Decision

The JSON artifact's `raw_signals` sub-object passes whisper.cpp's confidence
signals through raw (per-segment arrays of per-token data) and carries a
string `schema_version` (`EXPECTED_RAW_SIGNALS_SCHEMA_VERSION`, currently
"1"). The pipeline computes only summaries needed for its own operation,
indexing, or cheap sanity checks — never research aggregations.

## Guidance

- Review rejects adding pre-computed research metrics (mean log-p, fraction-below-threshold, …) to the artifact; downstream consumers aggregate raw data on demand, and pre-computing the wrong metric loses information the artifact can never recover.
- `schema_version` is a JSON **string**, compared as a string — string versioning admits additive minor revisions ("1.1") without re-parsing existing artifacts. Extend the schema additively and bump the constant.
- `lang_probs` stays opt-in (`--compute-lang-probs`, default null) because whisper.cpp re-encodes the audio to produce it — one extra encoder pass per video.
- Keep `raw_signals` inside the per-video `.json`, not a third file; the artifact pair (`.txt` + `.json`) stays stable per shard.

## Why

The project cannot know which aggregations future research questions need;
raw pass-through is the only shape that never destroys signal. The version
string is what lets the operational status check verify artifact
compatibility cheaply across a corpus written by different binary versions.

## Alternatives

- **Aggregate scalars only** — speculative; unrecoverable information loss.
- **Raw + aggregates** — duplication; aggregates are cheap to derive on demand.
- **Separate raw_signals.json** — doubles inodes per shard and makes consumers track two files.

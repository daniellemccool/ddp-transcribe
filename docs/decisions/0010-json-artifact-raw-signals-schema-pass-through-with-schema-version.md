---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-05-12 13:05:51"
      text: '(unrecoverable: legacy comment placeholder "1")'
    - author: Danielle McCool
      date: "2026-05-12 13:18:46"
      text: '2. (2026-05-12 13:18:46) Danielle McCool: Type pin (from codex code-quality review of T1): raw_signals.schema_version is a JSON string (e.g., "1"), not an integer. T4 derives the Rust field as String (or &''static str for the writer-side constant); T10 writes the literal "1"; AD0017''s status-check compares strings via EXPECTED_RAW_SIGNALS_SCHEMA_VERSION: &str = "1". Chosen because string versioning admits additive minor revisions ("1.1") without forcing a re-parse of existing artifacts.'
legacy-outcome: true
---

# JSON artifact raw_signals schema pass through with schema version

## Context and Problem Statement

How do we shape the per-video JSON artifact to carry whisper.cpp's confidence signals without speculative aggregation?

## Considered Options

* Pass-through raw signals (per-segment arrays of per-token data); schema_version on a new raw_signals sub-object
* Aggregate to per-video scalars (mean log-p, fraction-below-threshold, language confidence)
* Both aggregate scalars alongside raw data
* Separate file (raw_signals.json) instead of extending metadata.json

## Decision Drivers

Don't speculatively compute downstream-derived metrics. Per-video confidence required. lang_probs not freely available from whisper_full (sharp-edges.md:13); needs opt-in. Schema must be evolvable. Per-video artifact set stays compact and sharded.

## Decision Outcome
We decided for [Option 1](#option-1) because: The user explicitly stated pass-through, not pre-aggregation for research signals; this ADR codifies the rule. Pass-through rule (verbatim, for downstream reference): Raw pass-through is canonical for research signals; only compute summaries needed for pipeline operation, indexing, or cheap sanity checks. Downstream consumers compute aggregations if they want them. schema_version starts at 1 and extends additively. lang_probs is opt-in via --compute-lang-probs because the call re-encodes the audio (sharp-edges.md:13); default null; opt-in pays one extra encoder pass per video. Rejected alternatives: Option 2 (aggregate scalars only) — speculative, violates YAGNI; the project does not yet know which aggregations will be useful for which research questions, and pre-computing the wrong ones loses information that can never be recovered from the artifact. Option 3 (both aggregate scalars + raw) — doubles the field count without adding value; aggregations are cheap to compute from raw data on demand, so storing them alongside is duplication. Option 4 (separate raw_signals.json file) — fragments the per-video artifact set (currently .txt + .json), doubles the file-system inode count per shard, and makes consumers track two files instead of one; the raw_signals sub-object inside metadata.json keeps the artifact pair stable.

## Comments

* **2026-05-12 13:05:51 — @Danielle McCool:** (unrecoverable: legacy comment placeholder "1")
* **2026-05-12 13:18:46 — @Danielle McCool:** 2. (2026-05-12 13:18:46) Danielle McCool: Type pin (from codex code-quality review of T1): raw_signals.schema_version is a JSON string (e.g., "1"), not an integer. T4 derives the Rust field as String (or &'static str for the writer-side constant); T10 writes the literal "1"; AD0017's status-check compares strings via EXPECTED_RAW_SIGNALS_SCHEMA_VERSION: &str = "1". Chosen because string versioning admits additive minor revisions ("1.1") without forcing a re-parse of existing artifacts.

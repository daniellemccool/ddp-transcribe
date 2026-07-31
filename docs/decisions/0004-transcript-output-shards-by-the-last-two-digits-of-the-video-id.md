---
status: accepted
date: "2026-04-16"
category: Artifacts
applies_to:
    - src/output/mod.rs
    - src/pipeline/mod.rs
priority: default
---

# Transcript output shards by the last two digits of the video id

## Decision

Transcript artifacts are sharded into 100 buckets by the last two characters
of the `video_id`, and `output::shard()` is the single source of truth for
that segment — no other module derives an artifact path scheme.

## Guidance

- Every artifact path takes its shard segment from `output::shard()`; review rejects a second place that hard-codes `transcripts/<xx>/` layout knowledge. The `root.join(shard(id))` join itself lives at exactly one site, `pipeline::write_artifacts_durable` — a second site wanting it is the signal to extract a helper, not to re-derive the scheme.
- Don't switch to the *first* two characters: TikTok IDs are Snowflake derivatives, so high digits are timestamp-clustered (recent videos pile into 1–2 shards) while low digits are effectively random. The `shard_distributes_uniformly` test (100 distinct buckets) is the regression guard.
- The byte-slice contract is ASCII-only and currently implicit; if a `VideoId` newtype lands, move the contract into the type.
- 100 buckets is comfortable to ~1M files on ext4 (~10k dirents per shard); reassess multi-level sharding only if a corpus approaches that.

## Why

A flat directory degrades ext4 dirent lookup and breaks ls/find/backup
tooling at corpus scale, and the low-digit scheme keeps shards
operator-readable — a human holding a `video_id` can locate its file without
tooling, which a hash-derived shard sacrifices for uniformity we already get
from Snowflake low bits.

## Context

A `shard_dir(root, id)` helper carried the join for a year with no caller
outside its own unit test — the one artifact-writing site always open-coded
`root.join(shard(id))`. Epic 5b deleted it: an unused second "source of
truth" is a place for the scheme to drift, not a defense against drift. The
rule is unchanged in substance (one derivation of the segment, one join
site); only the named mechanism moved.

## Alternatives

- **First-two-chars** — time-clustered hot shards (see Guidance).
- **Hash-derived shard** — uniform but not operator-readable; adds hashing for nothing.
- **Multi-level `XX/XX`** — premature below ~10M files; hurts readability.
- **Flat directory** — the failure mode this record exists to prevent.

---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-04-16 23:26:03"
      text: '1. (2026-04-16 23:26:03) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Transcript output sharding by last two digits of video id

## Context and Problem Statement

Plan A writes one transcript file per TikTok video to a local filesystem. A study corpus could grow to hundreds of thousands of files. A flat directory degrades ext4 dirent lookup at scale, breaks `ls`/`find`/backup tooling, and makes operator inspection painful. We need a sharding scheme: how many buckets, derived from what part of the `video_id`, and where the layout policy lives in code so no other module hard-codes a path scheme.

## Considered Options

* Last-two-chars of `video_id` (low Snowflake digits → 100 buckets)
* First-two-chars of `video_id` (high Snowflake digits → 100 buckets)
* Hash-derived shard (e.g. first byte of SHA256(`video_id`) → 256 buckets)
* Multi-level sharding (last-4 split as `XX/XX` → 10000 buckets)
* No sharding (flat directory under transcripts root)

## Decision Drivers

Uniform distribution to avoid hot shards. Cheap to compute (no hashing or DB roundtrip). Operator-readable so a human looking at a known `video_id` can locate its shard without running tooling. Filesystem-friendly bucket count: ~100 keeps each shard at ~10k entries for ~1M files on ext4. Single source of truth for path layout in code so no other module reinvents the scheme.



## Decision Outcome
We decided for [Option 1](#option-1) because: TikTok video IDs are 19-digit Snowflake derivatives whose low bits are essentially random (sequence + machine ID), giving a uniform 100-bucket distribution. Encapsulate in output::shard() as the single source of truth - no other module hard-codes the layout. Rejected option 2 (high digits): timestamp-based and time-clustered, recent videos all land in 1-2 shards. The T8 distribution test counts.len() == 100 is the regression guard against accidentally switching to this. Rejected option 3 (hash-derived): perfectly uniform regardless of input but not operator-readable (must compute hash to locate a video) and adds a hashing dependency or cost - Snowflake low bits already give enough uniformity. Rejected option 4 (multi-level): premature for Plan A scale (low thousands per study), hurts operator readability, revisit if Plan B/C corpora exceed 10M files. Rejected option 5 (flat): ext4 dirent lookup degrades, ls/find/backup all break at scale. Trigger: T8 introduced output::shard() and the verbatim brief made the choice without recording the rejected alternatives. T8 opus code review surfaced the implicit ASCII contract and the distribution-test rationale issues, both signaling that this decision deserves explicit capture. Consequences: positive - cheap, uniform, operator-readable, single source of truth. Negative - the byte-slice contract is ASCII-only and currently implicit (FOLLOWUPS-tracked, made type-enforced when a VideoId newtype lands). Negative - 100 buckets caps comfortable ext4 scale around 1M files (10k per dir), Plan B/C should reassess if corpora grow. Trade-off: chose operator readability and simplicity over cryptographic uniformity guarantee, defended by Snowflake low-bit randomness.

## Comments

* **2026-04-16 23:26:03 — @Danielle McCool:** 1. (2026-04-16 23:26:03) Danielle McCool: marked decision as decided

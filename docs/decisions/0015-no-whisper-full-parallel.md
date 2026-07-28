---
status: accepted
date: "2026-05-12"
category: Whisper engine
applies_to:
    - src/transcribe.rs
priority: invariant
checks:
    - desc: whisper_full_parallel is never called
      grep: 'full_parallel'
      in: ["src/**/*.rs"]
      expect: absent
---

# No whisper_full_parallel

## Decision

`whisper_full_parallel` is never used. It is not a parallel-transcription
tool: it splits one audio across N states with documented quality loss at the
chunk boundaries, which research data cannot absorb.

## Guidance

- Review rejects any call to `whisper_full_parallel` (or a whisper-rs wrapper of it), including for short audio — the correctness/throughput trade is wrong on research data at any clip length, and a future workstream that wants it must bring its own evidence in its own ADR.
- Per-video parallelism is multiple `WhisperState`s on one context; multi-video parallelism is the channel-based orchestrator. Both keep every audio transcribed by a single state end to end.

## Why

The function's name invites exactly this mistake — it reads as "faster
transcription" but is actually "one audio chunked across states", and the
boundary degradation would silently contaminate the study corpus. Recorded
as an explicit non-decision so nobody reaches for it.

## Checks

- The frontmatter check greps `src/` for `full_parallel`; it must stay absent.

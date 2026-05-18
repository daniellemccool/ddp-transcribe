---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-05-18 13:26:27"
      text: '1. (2026-05-18 13:26:27) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Bounded subprocess output capture via streaming VecDeque<u8>

## Context and Problem Statement

`src/process.rs::run` previously read child stdout AND stderr each into unbounded `Vec<u8>` via `tokio::io::AsyncReadExt::read_to_end`, then sliced an 8 KiB tail of stderr via `ring_buffer_tail` for inclusion in `CommandOutcome::stderr_excerpt`. The `stderr_capture_bytes` field only bounded the *retained excerpt*, not the peak memory during read. A misbehaving subprocess emitting many gigabytes to stderr would allocate all of it before truncation. FOLLOWUPS T6 documented this gap; Plan B Epic 2 anticipated an ADR to close it (numbered AD0023 in the Epic 2 sketch, renumbered to AD0021 here because AD0018-AD0020 meta-process ADRs landed on main after the sketch was written).


## Considered Options

* Streaming reader filling a `VecDeque<u8>` of size `cap`; pop leading bytes when full; add `stdout_capture_bytes: usize` field on `CommandSpec` parallel to existing `stderr_capture_bytes`; change `CommandOutcome::stdout` from `Vec<u8>` to `Option<Vec<u8>>` (None when intentionally discarded, Some(bounded Vec) when capture requested); remove `ring_buffer_tail` helper.

* Keep `read_to_end` but bound CommandOutcome's retained bytes via post-hoc tail-slicing using a new field `*_capture_bytes` interpreted as "max retained" rather than "max in-flight." Status quo extended; peak memory remains unbounded.

* Replace `CommandOutcome` with an enum splitting AudioFile-write vs Captured-stdout use cases (`enum CommandOutput { AudioFile, Captured(Vec<u8>) }`). Restructures the public surface; tighter type-level expression of intent.


## Decision Drivers

Peak memory bounded by construction (load-bearing under N concurrent fetches anticipated in Plan B Epic 2 phase 2); shape distinguishes "captured but empty" from "intentionally discarded" cleanly at the type level; idiomatic Rust matches existing call-site style; does not over-fit Epic 2 T13's design freedom for symmetric stdout policy decisions; keeps `process::run` generic.

## Decision Outcome
We decided for [Option 1](#option-1) because: Distinguishes 'captured but empty' from 'intentionally discarded' at the type level (codex-advisor recommendation, idiomatic Rust). Avoids over-fitting Epic 2 T13's design freedom — T13 may add symmetric stdout policy decisions on top without authoring a new ADR. Keeps process::run generic. Option 2 doesn't fix peak memory (only retained); Option 3 is heavier API surface for this worktree's scope.

## Consequences

- **Retained-output memory ceiling** is now `stdout_capture_bytes + stderr_capture_bytes` per subprocess (call-site controlled), not "tool exit + truncation." Total peak memory during a `run` call is `retained + O(read_chunk_size_stdout + read_chunk_size_stderr + tokio task overhead)` — the chunk buffers used by the streaming reader hold at most one `read()` worth of bytes each before draining into the `VecDeque<u8>`. "Bounded by construction" applies to the retained buffer, not to instantaneous transient allocations.
- Call sites must explicitly opt in to stdout capture; `yt-dlp` and ffmpeg-postprocessor paths get `stdout_capture_bytes: 0` (no behavioral change — they did not read `outcome.stdout`).
- Plan B Epic 2's T13 inherits this design. T13 may add symmetric stdout policy decisions (different defaults for specific tools) without authoring a new ADR; if it changes the design, it supersedes AD0021 with a new ADR per existing convention.
- Test coverage in `tests/process_bounded_capture.rs` (5 tests, all passing) covers overflow tail preservation, stdout opt-in/opt-out, exit-code passthrough, and a direct `read_bounded` peak-memory-bounded assertion via an `Arc<AtomicUsize>` counter. Implementation landed in commit `9e84b54` immediately preceding this ADR.
- Filename note: the on-disk slug uses `vecdeque` (lowercase, no `<u8>`) rather than the title's literal `VecDeque<u8>` because `<` and `>` are shell redirection metacharacters and break unquoted globs in scripts. The title in YAML frontmatter retains the precise type name.

## Comments

* **2026-05-18 13:26:27 — @Danielle McCool:** 1. (2026-05-18 13:26:27) Danielle McCool: marked decision as decided

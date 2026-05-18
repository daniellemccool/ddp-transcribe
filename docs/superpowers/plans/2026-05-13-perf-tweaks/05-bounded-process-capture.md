# Task 5 — Bounded streaming subprocess capture

**Goal:** Replace `src/process.rs::run`'s unbounded `read_to_end` calls with a streaming reader that fills a `VecDeque<u8>` of size `cap`, dropping leading bytes when full. Peak memory becomes bounded by construction at the spec's `*_capture_bytes` values rather than by post-hoc tail-slicing. Add `stdout_capture_bytes: usize` to `CommandSpec`. Change `CommandOutcome::stdout` from `Vec<u8>` to `Option<Vec<u8>>` per the codex-advisor recommendation (distinguishes "captured but empty" from "intentionally discarded"). Remove `ring_buffer_tail` (no longer needed — bounded by construction). Author tests for overflow tail preservation, stdout opt-in/opt-out, exit-code passthrough, and peak-buffer-length.

**Spec commit:** 4 — `feat(process): bounded streaming subprocess capture`. (AD0021 is the immediately-following commit 5, in T6.)

**ADRs directly relevant:**
- **AD0001** — feature-derived design lands on `feat/perf-tweaks`; AD0021 (T6) rides with it.
- **AD0005** — new test file `tests/process_bounded_capture.rs` registers with `required-features = ["test-helpers"]`.

Background available if needed: AD0021 spec text in `docs/superpowers/specs/2026-05-13-perf-tweaks-design.md` (commit 62776ab).

**Files:**
- Modify: `src/process.rs` heavily (replace ~lines 100-174 streaming + bounded reader + new field + Option<Vec> + delete ring_buffer_tail + update mod tests)
- Modify: `src/fetcher/ytdlp.rs:78-85` (CommandSpec literal: add `stdout_capture_bytes: 0`)
- Modify: `Cargo.toml` (register `[[test]] name = "process_bounded_capture"`)
- Add: `tests/process_bounded_capture.rs` (new integration test file)

---

- [ ] **Step 1: Write the failing test file**

Create `tests/process_bounded_capture.rs`:

```rust
//! Tier 2 tests for bounded subprocess capture (T5 perf-tweaks; AD0021).
//!
//! Exercise the bounded streaming reader via real subprocesses (echo, sleep,
//! a stderr-spammer) and via direct calls to `process::read_bounded` for the
//! peak-memory assertion that can't be observed through `run` alone.

#![cfg(feature = "test-helpers")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use uu_tiktok::process::{read_bounded, run, CommandSpec};

const CAP: usize = 8 * 1024;

#[tokio::test]
async fn stderr_excerpt_preserves_tail_when_subprocess_overflows_cap() {
    // Spawn a child that emits ~2*CAP bytes to stderr. The excerpt must
    // be CAP bytes long, contain the LAST CAP bytes (the tail), and
    // truncate the head.
    let payload_size = 2 * CAP;
    let cmd = format!(
        "yes 'x' | head -c {} >&2",
        payload_size
    );
    let spec = CommandSpec {
        program: "sh",
        args: vec!["-c".into(), cmd],
        timeout: Duration::from_secs(10),
        stderr_capture_bytes: CAP,
        stdout_capture_bytes: 0,
        redact_arg_indices: &[],
    };
    let outcome = run(spec).await.expect("sh runs");
    assert_eq!(outcome.exit_code, 0);
    let excerpt = outcome.stderr_excerpt;
    assert_eq!(
        excerpt.len(),
        CAP,
        "stderr_excerpt must be exactly CAP bytes, got {}",
        excerpt.len()
    );
    // Tail bytes from `yes 'x' | head -c N` are 'x' or '\n'; just sanity-
    // check the excerpt is non-empty and looks like the expected payload.
    assert!(excerpt.chars().all(|c| c == 'x' || c == '\n'));
}

#[tokio::test]
async fn stdout_is_none_when_capture_bytes_zero() {
    let spec = CommandSpec {
        program: "echo",
        args: vec!["hello world".into()],
        timeout: Duration::from_secs(5),
        stderr_capture_bytes: 1024,
        stdout_capture_bytes: 0,
        redact_arg_indices: &[],
    };
    let outcome = run(spec).await.expect("echo runs");
    assert_eq!(outcome.exit_code, 0);
    assert!(
        outcome.stdout.is_none(),
        "stdout_capture_bytes == 0 must yield None, got Some(...)"
    );
}

#[tokio::test]
async fn stdout_is_some_when_capture_bytes_nonzero() {
    let spec = CommandSpec {
        program: "echo",
        args: vec!["hello world".into()],
        timeout: Duration::from_secs(5),
        stderr_capture_bytes: 1024,
        stdout_capture_bytes: 1024,
        redact_arg_indices: &[],
    };
    let outcome = run(spec).await.expect("echo runs");
    assert_eq!(outcome.exit_code, 0);
    let stdout = outcome.stdout.expect("stdout requested");
    assert_eq!(
        String::from_utf8_lossy(&stdout).trim(),
        "hello world"
    );
}

#[tokio::test]
async fn exit_code_passes_through_bounded_capture() {
    let spec = CommandSpec {
        program: "false",
        args: vec![],
        timeout: Duration::from_secs(5),
        stderr_capture_bytes: 1024,
        stdout_capture_bytes: 0,
        redact_arg_indices: &[],
    };
    let outcome = run(spec).await.expect("false runs");
    assert_ne!(outcome.exit_code, 0);
}

#[tokio::test]
async fn read_bounded_peak_len_never_exceeds_cap() {
    // Use `tokio::io::duplex` to pair an in-memory writer and reader.
    // (`std::io::Cursor<Vec<u8>>` only implements `std::io::Read`, not
    // `tokio::io::AsyncRead`, so it cannot be passed to `read_bounded`.)
    // Spawn the payload-writer concurrently so `read_bounded` drains the
    // reader as bytes arrive; this also exercises the chunked-read loop
    // realistically.
    let (mut writer, mut reader) = tokio::io::duplex(64 * 1024);
    let payload: Vec<u8> = (0..1_000_000_u32).map(|i| (i % 256) as u8).collect();
    let payload_len = payload.len();

    tokio::spawn(async move {
        writer.write_all(&payload).await.expect("write payload");
        drop(writer); // EOF
    });

    let peak = Arc::new(AtomicUsize::new(0));
    let result = read_bounded(&mut reader, CAP, Some(&peak))
        .await
        .expect("read_bounded must succeed");
    let bytes = result.expect("cap > 0, expected Some");
    assert_eq!(bytes.len(), CAP, "captured bytes must equal CAP");

    let final_peak = peak.load(Ordering::Relaxed);
    assert!(
        final_peak <= CAP,
        "peak_buffer_len must be bounded by cap; got {}, cap {}, payload size {}",
        final_peak, CAP, payload_len
    );
    assert!(
        final_peak > 0,
        "counter must have been incremented at least once"
    );
}
```

- [ ] **Step 2: Register the test in `Cargo.toml`**

Append:
```toml
[[test]]
name = "process_bounded_capture"
required-features = ["test-helpers"]
```

- [ ] **Step 3: Run the test — verify it fails to compile**

Run:
```bash
cargo test --features test-helpers --test process_bounded_capture --no-run
```

Expected: compile errors on:
- `CommandSpec { … stdout_capture_bytes: 0, … }` — field doesn't exist yet
- `outcome.stdout.is_none()` — `stdout` is `Vec<u8>`, not `Option<Vec<u8>>`
- `read_bounded` — function doesn't exist yet

This is the failing-test baseline.

- [ ] **Step 4: Update `CommandSpec` and `CommandOutcome` in `src/process.rs`**

Modify the structs (currently around lines 10-39):

```rust
#[derive(Debug)]
pub struct CommandSpec<'a> {
    pub program: &'static str,
    pub args: Vec<String>,
    pub timeout: Duration,
    /// Last-N bytes of stderr retained in `CommandOutcome.stderr_excerpt`.
    /// Bounded by construction (AD0021): the streaming reader maintains a
    /// `VecDeque<u8>` of size `stderr_capture_bytes` and drops leading bytes
    /// when full. Setting to 0 still drains stderr (so the child doesn't
    /// block on a full pipe) but discards bytes as they arrive; the excerpt
    /// is then an empty string.
    pub stderr_capture_bytes: usize,
    /// Last-N bytes of stdout retained in `CommandOutcome.stdout`. Same
    /// bounded-by-construction semantics as `stderr_capture_bytes`. Setting
    /// to 0 yields `CommandOutcome.stdout == None` (intentional discard);
    /// nonzero yields `Some(bounded Vec)` (capture requested, may still be
    /// empty if the child emitted no stdout).
    pub stdout_capture_bytes: usize,
    /// Argument indices to redact in the structured log (e.g., cookie file paths).
    pub redact_arg_indices: &'a [usize],
}

#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub exit_code: i32,
    /// `None` when the caller set `stdout_capture_bytes == 0` (intentional
    /// discard); `Some(bounded Vec)` otherwise. The vec length is bounded
    /// by `stdout_capture_bytes`.
    pub stdout: Option<Vec<u8>>,
    pub stderr_excerpt: String,
    #[allow(dead_code)]
    pub elapsed: Duration,
}
```

(Remove the `#[allow(dead_code)]` that was on the old `stdout` field — `stdout` is no longer dead code now that callers explicitly choose discard vs capture via `stdout_capture_bytes`. The `#[allow]` on `elapsed` stays.)

- [ ] **Step 5: Add the `read_bounded` streaming reader**

Insert this `pub` function in `src/process.rs` (top of file after the existing `use` statements is a good location, or just above the `run` function):

```rust
/// Bounded streaming reader. Drains the input fully, keeping at most `cap`
/// trailing bytes in memory at any moment. Returns `Some(bounded Vec)` when
/// `cap > 0` (capture requested) and `None` when `cap == 0` (intentional
/// discard — bytes are still drained to prevent the child blocking on a
/// full pipe, but not retained).
///
/// AD0021 invariant: peak retained memory is bounded by `cap` regardless of
/// how much the child emits. The optional `peak_buffer_len` counter is for
/// test instrumentation; production callers pass `None`.
pub async fn read_bounded<R>(
    reader: &mut R,
    cap: usize,
    peak_buffer_len: Option<&std::sync::Arc<std::sync::atomic::AtomicUsize>>,
) -> Result<Option<Vec<u8>>, std::io::Error>
where
    R: tokio::io::AsyncRead + Unpin + ?Sized,
{
    use tokio::io::AsyncReadExt;

    const CHUNK: usize = 8 * 1024;
    let mut chunk = [0u8; CHUNK];

    if cap == 0 {
        // Discard mode: drain but don't retain.
        loop {
            match reader.read(&mut chunk).await? {
                0 => return Ok(None),
                _ => continue,
            }
        }
    }

    let mut deque: std::collections::VecDeque<u8> =
        std::collections::VecDeque::with_capacity(cap);
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        for &b in &chunk[..n] {
            if deque.len() == cap {
                deque.pop_front();
            }
            deque.push_back(b);
        }
        if let Some(p) = peak_buffer_len {
            p.fetch_max(deque.len(), std::sync::atomic::Ordering::Relaxed);
        }
    }
    Ok(Some(Vec::from(deque)))
}
```

(Note: `read_bounded` is `pub` so the integration test can import it; the doc comment marks it as a primitive of `run`. If the codebase prefers `#[doc(hidden)]` on internals not part of the stable surface, add that attribute.)

- [ ] **Step 6: Rewrite `run` to use `read_bounded`**

Replace the body of `pub async fn run(spec: CommandSpec<'_>) -> Result<CommandOutcome, RunError>` (currently around lines 82-166). Key changes:

- Replace the `read_outputs = async { … tokio::try_join!(read_to_end…) }` block with two `read_bounded` calls inside `try_join!`.
- The stdout result is `Option<Vec<u8>>` directly.
- The stderr result is `Option<Vec<u8>>`; convert to `String` for the excerpt field via `String::from_utf8_lossy(...).into_owned()` (or empty string if `None`).
- Delete the call to `ring_buffer_tail`.

Concretely, the relevant block becomes:

```rust
    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");

    // Bounded streaming reads (AD0021). Peak memory is bounded by
    // `stdout_capture_bytes + stderr_capture_bytes` retention; transient
    // chunk-read buffers add O(8 KiB × 2) on top, plus tokio task overhead.
    let stdout_cap = spec.stdout_capture_bytes;
    let stderr_cap = spec.stderr_capture_bytes;
    let read_outputs = async {
        tokio::try_join!(
            read_bounded(&mut stdout_pipe, stdout_cap, None),
            read_bounded(&mut stderr_pipe, stderr_cap, None),
        )
    };

    let result = timeout(spec.timeout, async {
        let (stdout, stderr_bytes) = read_outputs.await.map_err(|source| RunError::Io {
            tool: spec.program,
            source,
        })?;
        let status = child.wait().await.map_err(|source| RunError::Io {
            tool: spec.program,
            source,
        })?;
        Ok::<_, RunError>((stdout, stderr_bytes, status))
    })
    .await;

    match result {
        Ok(Ok((stdout, stderr_bytes, status))) => {
            let exit_code = status.code().unwrap_or(-1);
            let stderr_excerpt = stderr_bytes
                .map(|v| String::from_utf8_lossy(&v).into_owned())
                .unwrap_or_default();
            let elapsed = started.elapsed();
            Ok(CommandOutcome {
                exit_code,
                stdout,
                stderr_excerpt,
                elapsed,
            })
        }
        Ok(Err(e)) => Err(e),
        Err(_elapsed) => {
            // Timed out. Defense-in-depth termination unchanged: see
            // existing comment for the SIGKILL + kill_on_drop reasoning.
            let _ = child.start_kill();
            Err(RunError::Timeout {
                tool: spec.program,
                duration: spec.timeout,
            })
        }
    }
```

(Preserve the existing tracing instrumentation and the redact_arg_indices logging at the top of `run`; only the body-after-spawn-and-pipe-take changes.)

- [ ] **Step 7: Delete the `ring_buffer_tail` helper**

Delete the entire `ring_buffer_tail` fn (lines ~168-174 in the original). No replacement; capture is now bounded by construction.

- [ ] **Step 8: Update the existing `#[cfg(test)] mod tests` inside `src/process.rs`**

Three of the four existing tests construct `CommandSpec` literals — they need the new `stdout_capture_bytes` field. Find them at lines 180-240 and add `stdout_capture_bytes: 1024` (or `0` where stdout isn't read; pick `1024` for the `echo_succeeds_with_stdout` test which does check stdout).

For `echo_succeeds_with_stdout` specifically:
```rust
#[tokio::test]
async fn echo_succeeds_with_stdout() {
    let spec = CommandSpec {
        program: "echo",
        args: vec!["hello".into(), "world".into()],
        timeout: Duration::from_secs(5),
        stderr_capture_bytes: 1024,
        stdout_capture_bytes: 1024,
        redact_arg_indices: &[],
    };
    let outcome = run(spec).await.expect("echo runs");
    assert_eq!(outcome.exit_code, 0);
    let stdout = outcome.stdout.expect("stdout requested");
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "hello world");
}
```

For `false_returns_nonzero_exit`, `timeout_kills_long_running_subprocess`, and `missing_program_returns_spawn_error`: add `stdout_capture_bytes: 0` and leave the rest unchanged.

- [ ] **Step 9: Update `src/fetcher/ytdlp.rs` to pass `stdout_capture_bytes: 0`**

In `src/fetcher/ytdlp.rs` around line 78-85, the `CommandSpec` literal needs the new field:

```rust
        let outcome = run(CommandSpec {
            program: "yt-dlp",
            args,
            timeout: self.timeout,
            stderr_capture_bytes: 8 * 1024,
            stdout_capture_bytes: 0, // yt-dlp writes audio to a file; stdout unused
            redact_arg_indices: &[],
        })
        .await?;
```

- [ ] **Step 10: Check for other CommandSpec construction sites**

Run:
```bash
grep -rn "CommandSpec {" src/ tests/
```

Update any other call sites to include `stdout_capture_bytes`. If `tests/pipeline_fakes.rs` or another test constructs CommandSpec, add the field there too.

- [ ] **Step 11: Run the new bounded-capture tests**

Run:
```bash
cargo test --features test-helpers --test process_bounded_capture -- --nocapture
```

Expected: all five tests pass.

If `stderr_excerpt_preserves_tail_when_subprocess_overflows_cap` fails because `sh` or `yes` isn't available on the dev environment, swap the payload command to a Rust-only alternative or wrap the test with `#[cfg(unix)]`. The fixture command must be portable across the dev machine and the bake machine.

- [ ] **Step 12: Run the in-module tests**

Run:
```bash
cargo test --features test-helpers process::tests
```

Expected: all four in-module tests pass with the updated `CommandSpec` literals.

- [ ] **Step 13: Run the full test suite**

Run:
```bash
cargo test --features test-helpers
```

Expected: no regressions. Special attention to integration tests that exercise the fetch path — `tests/pipeline_fakes.rs`, `tests/e2e_real_tools.rs`. They should keep working with the new shape (stdout becomes `Option`; existing tests that don't read stdout aren't affected).

- [ ] **Step 14: cargo fmt + clippy**

Run:
```bash
cargo fmt --all && cargo clippy --all-targets --features test-helpers -- -D warnings
```

Expected: clean.

If clippy complains about `&std::sync::Arc<…>` references being awkward, switch to `Option<Arc<AtomicUsize>>` (owned, not borrowed) on the `read_bounded` signature. The cost is a refcount bump per call; negligible.

- [ ] **Step 15: Commit**

```bash
git add src/process.rs src/fetcher/ytdlp.rs tests/process_bounded_capture.rs Cargo.toml
git commit -m "$(cat <<'EOF'
feat(process): bounded streaming subprocess capture

Replaces unbounded `tokio::read_to_end` reads of subprocess stdout
and stderr with a streaming reader that maintains a `VecDeque<u8>`
of size `cap`, dropping leading bytes when full. Peak retained
memory is now bounded by construction at
`stdout_capture_bytes + stderr_capture_bytes` per subprocess
(transient chunk buffers add O(8 KiB × 2) + tokio task overhead).

New `stdout_capture_bytes: usize` field on `CommandSpec` parallel to
the existing `stderr_capture_bytes`. `CommandOutcome::stdout` shape
changes from `Vec<u8>` to `Option<Vec<u8>>`: `None` when caller set
`stdout_capture_bytes == 0` (intentional discard), `Some(bounded
Vec)` otherwise. Distinguishes "captured but empty" from
"intentionally discarded" — codex-advisor recommendation, idiomatic
Rust.

The `ring_buffer_tail` helper is removed. Capture is bounded by
construction, not by post-hoc tail-slicing. Existing callers that
read `stderr_excerpt` see no behavioral change (excerpt is still
bounded; just produced by construction now).

Call-site updates:
- `src/fetcher/ytdlp.rs`: `stdout_capture_bytes: 0` (yt-dlp writes
  audio to a file; stdout was already unused).
- in-module tests in `src/process.rs`: `stdout_capture_bytes` added
  to each `CommandSpec` literal.

New Tier-2 test file `tests/process_bounded_capture.rs`:
- stderr_excerpt_preserves_tail_when_subprocess_overflows_cap
- stdout_is_none_when_capture_bytes_zero
- stdout_is_some_when_capture_bytes_nonzero
- exit_code_passes_through_bounded_capture
- read_bounded_peak_len_never_exceeds_cap (direct test of the
  bounded primitive; observes via Arc<AtomicUsize> counter)

AD0021 (next commit) records the design decision and consequence
ladder. Resolves FOLLOWUPS L47 + L48 (process::run unbounded
capture and ring_buffer_tail misnaming).

Cross-session coordination: Plan B Epic 2's T13 anticipated this
work; it can now absorb only symmetric stdout policy decisions
(per-tool defaults) on top of AD0021 without re-authoring the
design.

Refs: AD0001, AD0005, AD0021 (T6 commit)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `CommandSpec` has `stdout_capture_bytes: usize` alongside the existing `stderr_capture_bytes: usize`.
- [ ] `CommandOutcome::stdout` is `Option<Vec<u8>>`; the `#[allow(dead_code)]` on the old stdout field is removed.
- [ ] `read_bounded` is `pub` and exported; tests in `tests/process_bounded_capture.rs` import it.
- [ ] `ring_buffer_tail` is gone from `src/process.rs`.
- [ ] All `CommandSpec` literals in the repo have `stdout_capture_bytes` set explicitly.
- [ ] All new tests pass; no regressions in the existing test suite.
- [ ] AD0021 ADR drafted in T6 immediately after this commit.

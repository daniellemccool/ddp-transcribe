# Task 14 — Bounded `process::run` capture; symmetric stdout cap; rename `ring_buffer_tail`

**Goal:** Replace `read_to_end` in `src/process.rs::run` with a streaming reader that maintains a `VecDeque<u8>` capped at `stderr_capture_bytes`. Add symmetric `stdout_capture_bytes` field to `CommandSpec`. Rename `ring_buffer_tail` → `tail_excerpt` (no semantic change). Test against a synthetic stderr flood that would balloon memory under the old path.

**ADRs touched:** AD0026 (bounded capture).

**Files:**
- Modify: `src/process.rs` (`run`, `CommandSpec`, `CommandOutcome`, `ring_buffer_tail` → `tail_excerpt`)
- Modify: `src/fetcher/ytdlp.rs` (caller passes `stdout_capture_bytes`)
- Modify: existing call sites that construct `CommandSpec` (add `stdout_capture_bytes`)
- Modify: `tests` in `src/process.rs::tests` (add stderr-flood test)

**Pre-reqs:** T12 (AD0026 decided). **Note:** the efficiency-tweaks worktree has only doc commits as of plan-write — T14 keeps full scope. If the worktree lands the bounded buffer before Phase 2 starts, this task's scope reduces to rename + symmetric stdout cap + test passes after rename.

---

- [ ] **Step 1: Update `CommandSpec` and `CommandOutcome`**

In `src/process.rs`:

```rust
#[derive(Debug)]
pub struct CommandSpec<'a> {
    pub program: &'static str,
    pub args: Vec<String>,
    pub timeout: Duration,
    /// Last-N bytes of stderr retained in `CommandOutcome.stderr_excerpt`.
    /// AD0026: streaming-bounded; peak memory for stderr ≤ this value.
    pub stderr_capture_bytes: usize,
    /// Last-N bytes of stdout retained in `CommandOutcome.stdout_excerpt`.
    /// AD0026 defense-in-depth: yt-dlp writes audio to a file so stdout is
    /// usually small, but a misbehaving tool could flood. Cap with the
    /// same bound as stderr unless the caller knows it wants more.
    pub stdout_capture_bytes: usize,
    pub redact_arg_indices: &'a [usize],
}

#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub exit_code: i32,
    /// Last `stdout_capture_bytes` of stdout. Per AD0026 the full stream
    /// is not retained — callers that need the body must redirect to a
    /// file (yt-dlp does so via `--output`).
    pub stdout_excerpt: String,
    pub stderr_excerpt: String,
    pub elapsed: Duration,
}
```

Note: `stdout: Vec<u8>` becomes `stdout_excerpt: String`. The `#[allow(dead_code)]` annotations on `stdout` and `elapsed` come off — both are now part of the documented API surface (callers will read them; if not yet, T18's cleanup removes the dead-code attribute on `elapsed` if no caller materializes).

- [ ] **Step 2: Replace `read_to_end` with a streaming bounded reader**

Replace the `run` body's `read_outputs` block:

```rust
let mut stdout = child.stdout.take().expect("piped stdout");
let mut stderr = child.stderr.take().expect("piped stderr");

let stdout_cap = spec.stdout_capture_bytes;
let stderr_cap = spec.stderr_capture_bytes;

let read_outputs = async move {
    // Streaming-bounded readers: each maintains a VecDeque<u8> of size
    // `cap`. As new bytes arrive, append; if length exceeds cap, drain
    // from the front. Peak memory per stream is `cap + 8KB read buffer`.
    use std::collections::VecDeque;
    use tokio::io::AsyncReadExt;

    async fn drain_into_tail(
        mut reader: impl tokio::io::AsyncRead + Unpin,
        cap: usize,
    ) -> std::io::Result<VecDeque<u8>> {
        let mut tail: VecDeque<u8> = VecDeque::with_capacity(cap.min(8 * 1024));
        let mut buf = vec![0u8; 8 * 1024];
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            tail.extend(buf[..n].iter().copied());
            if tail.len() > cap {
                let excess = tail.len() - cap;
                tail.drain(..excess);
            }
        }
        Ok(tail)
    }

    tokio::try_join!(
        drain_into_tail(stdout, stdout_cap),
        drain_into_tail(stderr, stderr_cap),
    )
};
```

Then in the result-match arm:

```rust
match result {
    Ok(Ok((stdout_tail, stderr_tail))) => {
        let status = child.wait().await.map_err(|source| RunError::Io {
            tool: spec.program,
            source,
        })?;
        let exit_code = status.code().unwrap_or(-1);
        let stdout_excerpt = tail_excerpt(&stdout_tail);
        let stderr_excerpt = tail_excerpt(&stderr_tail);
        let elapsed = started.elapsed();
        Ok(CommandOutcome {
            exit_code,
            stdout_excerpt,
            stderr_excerpt,
            elapsed,
        })
    }
    // ... unchanged Timeout arm
}
```

The `child.wait()` call moves out of the inner async block (the timeout block above wraps both the read and the wait). Reorder slightly — the structural concern is that the `read_outputs` future drives both streams to EOF (which happens when the subprocess closes them on exit), so adding `child.wait()` afterwards has zero latency in the happy path.

Replace `ring_buffer_tail` with `tail_excerpt`:

```rust
/// Convert a streaming-bounded VecDeque<u8> (kept at ≤ cap bytes by the
/// reader loop) to a lossy-UTF8 String. The "tail" naming is descriptive:
/// the input contains the last `cap` bytes of the stream, period. Plan A's
/// `ring_buffer_tail` name implied circular-buffer semantics the function
/// never had.
fn tail_excerpt(buf: &VecDeque<u8>) -> String {
    if buf.is_empty() {
        return String::new();
    }
    // Linearize into one slice for `from_utf8_lossy`.
    let (a, b) = buf.as_slices();
    let mut linear = Vec::with_capacity(a.len() + b.len());
    linear.extend_from_slice(a);
    linear.extend_from_slice(b);
    String::from_utf8_lossy(&linear).into_owned()
}
```

The old standalone `fn ring_buffer_tail(buf: &[u8], cap: usize) -> String` is removed. The cap is enforced inside `drain_into_tail`; `tail_excerpt` just converts.

- [ ] **Step 3: Write the failing test (stderr flood)**

Append to `src/process.rs::tests`:

```rust
#[tokio::test]
async fn stderr_flood_does_not_balloon_memory_keeps_last_n_bytes() {
    // sh -c 'for i in $(seq 1 100000); do echo "line $i" >&2; done'
    // produces ~1 MB of stderr. Cap is 1024 bytes. Excerpt should be
    // the last 1024 bytes.
    let spec = CommandSpec {
        program: "sh",
        args: vec![
            "-c".into(),
            "for i in $(seq 1 100000); do echo \"line $i\" >&2; done".into(),
        ],
        timeout: Duration::from_secs(30),
        stderr_capture_bytes: 1024,
        stdout_capture_bytes: 1024,
        redact_arg_indices: &[],
    };
    let outcome = run(spec).await.expect("sh runs");
    assert_eq!(outcome.exit_code, 0);
    assert!(
        outcome.stderr_excerpt.len() <= 1024,
        "stderr_excerpt len {} should be ≤ 1024",
        outcome.stderr_excerpt.len()
    );
    // The tail should end with the last line emitted.
    assert!(
        outcome.stderr_excerpt.contains("line 100000"),
        "tail should contain the last line, got: {}",
        outcome.stderr_excerpt
    );
}

#[tokio::test]
async fn echo_succeeds_with_stdout_excerpt() {
    // Renamed from `echo_succeeds_with_stdout` to track the API rename.
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
    assert_eq!(outcome.stdout_excerpt.trim(), "hello world");
}
```

Update other existing tests in this module to provide the new `stdout_capture_bytes` field (set to 1024 throughout).

Run:

```bash
cargo test --features test-helpers process
```

Expected: FAIL (compile errors — new fields not present yet on the existing tests' `CommandSpec`).

- [ ] **Step 4: Update all `CommandSpec` callers**

```bash
grep -rn 'CommandSpec {' src/ tests/
```

For each call site (Plan A: `src/fetcher/ytdlp.rs`; potentially others), add `stdout_capture_bytes: <value>`. Suggested defaults:

- `fetcher/ytdlp.rs`: `stdout_capture_bytes: 1024` (yt-dlp's stdout is small — diagnostic only).

If any caller reads `outcome.stdout: Vec<u8>` (the old field), update to `outcome.stdout_excerpt: String`. Plan A's `#[allow(dead_code)]` on the old field suggests no current bin caller, but tests may reference it.

- [ ] **Step 5: Run the tests**

```bash
cargo test --features test-helpers
```

Expected: PASS for all tests, including the new flood test.

- [ ] **Step 6: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean. Remove any `#[allow(dead_code)]` that no longer fires (the `stdout` field is renamed; if `elapsed` is now read by Phase 2 callers in T17/T18, remove its annotation too).

- [ ] **Step 7: Commit**

```bash
git add src/process.rs src/fetcher/ytdlp.rs
git commit -m "$(cat <<'EOF'
feat(process): bounded streaming capture; symmetric stdout cap; ring_buffer_tail → tail_excerpt (AD0026)

Replaces Plan A's read_to_end + truncate-after pattern with a streaming
bounded reader. Each stream (stdout, stderr) is drained into a
VecDeque<u8> of size `*_capture_bytes`; oldest bytes drop as new ones
arrive. Peak memory per subprocess is now stdout_capture_bytes +
stderr_capture_bytes + 16KB read buffers, regardless of how much the
subprocess writes.

API changes:
- CommandSpec gains `stdout_capture_bytes: usize` (symmetric with the
  existing `stderr_capture_bytes`).
- CommandOutcome.stdout: Vec<u8> → CommandOutcome.stdout_excerpt: String
  (the full body is no longer retained; subprocesses that need bulk
  stdout must redirect to a file — yt-dlp already does this).
- Helper renamed: ring_buffer_tail (implied circular-buffer semantics
  it didn't have) → tail_excerpt (describes what it returns).

Test: synthesized 100,000-line stderr flood (~1 MB) with cap = 1024
asserts the excerpt is ≤ 1024 bytes and contains the last line written.
The same test under Plan A's read_to_end path would buffer 1 MB before
truncating.

Callers updated (fetcher/ytdlp.rs).

Refs: AD0026, FOLLOWUPS T6 (process::run unbounded + ring_buffer_tail
misnamed) — both resolved.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers process` passes (flood test + renamed existing tests)
- [ ] Full suite green (no callers broken)
- [ ] `ring_buffer_tail` is gone from the codebase (`grep ring_buffer_tail src/` empty)
- [ ] CommandSpec callers all provide `stdout_capture_bytes`
- [ ] Clippy/fmt clean

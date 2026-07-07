# Task 02: Error-type refinements (FetchError split, signal capture, AudioDecode variant)

**Files:**
- Modify: `src/errors.rs` (FetchError variants; TranscribeError::AudioDecode; From impl)
- Modify: `src/process.rs` (CommandOutcome.signal; From<RunError> split; run() capture site)
- Modify: `src/fetcher/ytdlp.rs` (acquire error mappings: work-dir + missing-WAV)
- Tests: in-module unit tests in the three files above

**Interfaces:**
- Consumes: existing `RunError { Spawn, Timeout, Io }`, `CommandOutcome`, `FetchError`, `TranscribeError`, `AudioDecodeError`.
- Produces (Task 03's classifier and Task 07's dispatch depend on these exact shapes):
  - `FetchError::ToolNotFound { tool: &'static str, detail: String }`
  - `FetchError::SystemIo { tool: &'static str, detail: String }`
  - `FetchError::WorkDirCreate { path: PathBuf, detail: String }`
  - `FetchError::MissingOutput { path: PathBuf }`
  - `FetchError::ToolFailed` gains `signal: Option<i32>`
  - `CommandOutcome` gains `signal: Option<i32>`
  - `TranscribeError::AudioDecode { detail: String }` (replaces the Bug mapping)

Resolves FOLLOWUPS: T6 (`From<RunError>` collapse; `unwrap_or(-1)` signal loss), T11 findings 1–2 (`create_dir_all` → NetworkError; missing WAV → ParseError), T5-Epic1 (`From<AudioDecodeError>` → Bug).

- [ ] **Step 1: Write the failing unit tests**

In `src/process.rs` `#[cfg(test)] mod tests` (create the module if the file has none — check first; bounded-capture tests live in `tests/process_bounded_capture.rs`, in-module tests may not exist):

```rust
#[test]
fn run_error_spawn_maps_to_tool_not_found() {
    let e = RunError::Spawn {
        tool: "yt-dlp",
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
    };
    match FetchError::from(e) {
        FetchError::ToolNotFound { tool, .. } => assert_eq!(tool, "yt-dlp"),
        other => panic!("Spawn must map to ToolNotFound, got {other:?}"),
    }
}

#[test]
fn run_error_io_maps_to_system_io() {
    let e = RunError::Io {
        tool: "yt-dlp",
        source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe"),
    };
    match FetchError::from(e) {
        FetchError::SystemIo { tool, .. } => assert_eq!(tool, "yt-dlp"),
        other => panic!("Io must map to SystemIo, got {other:?}"),
    }
}
```

In `src/errors.rs` tests:

```rust
#[test]
fn audio_decode_error_maps_to_audio_decode_not_bug() {
    let e = crate::audio::AudioDecodeError::from(hound::Error::FormatError("truncated"));
    match TranscribeError::from(e) {
        TranscribeError::AudioDecode { .. } => {}
        other => panic!("AudioDecodeError must map to AudioDecode, got {other:?}"),
    }
}
```

(Adjust the `AudioDecodeError` constructor to whatever `src/audio.rs` actually exposes — check its variants first; if no public constructor exists from test scope, construct via a real 0-byte temp file through the public decode fn instead.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers run_error_ --lib -- --test-threads=1`
Expected: compile errors — the variants don't exist yet. That is the failing state.

- [ ] **Step 3: Implement the error-type changes**

`src/errors.rs` — add variants (keep existing ones; `NetworkError` and `ParseError` remain for their legitimate uses):

```rust
#[derive(Debug, Error)]
pub enum FetchError {
    #[error("subprocess `{tool}` timed out after {duration:?}")]
    ToolTimeout { tool: &'static str, duration: Duration },

    #[error("subprocess `{tool}` exited with status {exit_code}: {stderr_excerpt}")]
    ToolFailed {
        tool: &'static str,
        exit_code: i32,
        /// Unix signal that killed the child, when it did not exit normally
        /// (`ExitStatus::code() == None`). Distinguishes OOM-kill (SIGKILL)
        /// from segfault (SIGSEGV) from operator interrupt (SIGINT).
        signal: Option<i32>,
        stderr_excerpt: String,
    },

    #[error("tool not found or not executable: {tool}: {detail}")]
    ToolNotFound { tool: &'static str, detail: String },

    #[error("system io error running {tool}: {detail}")]
    SystemIo { tool: &'static str, detail: String },

    #[error("failed to create work dir {path}: {detail}")]
    WorkDirCreate { path: std::path::PathBuf, detail: String },

    #[error("tool succeeded but expected output {path} is missing")]
    MissingOutput { path: std::path::PathBuf },

    #[error("network error during fetch: {0}")]
    NetworkError(String),

    #[error("failed to parse fetcher output: {0}")]
    ParseError(String),
}
```

`TranscribeError`: add `#[error("audio decode failure: {detail}")] AudioDecode { detail: String },` and rewrite the From impl:

```rust
impl From<crate::audio::AudioDecodeError> for TranscribeError {
    fn from(e: crate::audio::AudioDecodeError) -> Self {
        TranscribeError::AudioDecode { detail: e.to_string() }
    }
}
```

`src/process.rs` — `From<RunError>`:

```rust
impl From<RunError> for FetchError {
    fn from(err: RunError) -> Self {
        match err {
            RunError::Timeout { tool, duration } => FetchError::ToolTimeout { tool, duration },
            RunError::Spawn { tool, source } => FetchError::ToolNotFound {
                tool,
                detail: source.to_string(),
            },
            RunError::Io { tool, source } => FetchError::SystemIo {
                tool,
                detail: source.to_string(),
            },
        }
    }
}
```

Delete the now-stale "Plan A coarse mapping" comment above it. Add `signal: Option<i32>` to `CommandOutcome` and populate it in `run()` where `exit_code` is currently derived (find the `status.code().unwrap_or(-1)` site):

```rust
#[cfg(unix)]
let signal = {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
};
#[cfg(not(unix))]
let signal: Option<i32> = None;
let exit_code = status.code().unwrap_or(-1);
```

`src/fetcher/ytdlp.rs` — in `acquire`:

```rust
std::fs::create_dir_all(&video_dir).map_err(|e| FetchError::WorkDirCreate {
    path: video_dir.clone(),
    detail: e.to_string(),
})?;
```

```rust
if outcome.exit_code != 0 {
    return Err(FetchError::ToolFailed {
        tool: "yt-dlp",
        exit_code: outcome.exit_code,
        signal: outcome.signal,
        stderr_excerpt: outcome.stderr_excerpt,
    });
}
if !wav_path.exists() {
    return Err(FetchError::MissingOutput { path: wav_path });
}
```

Fix all other `ToolFailed` construction sites the compiler flags (add `signal: None` or thread the real value where a `CommandOutcome` is in scope — search `tests/` too).

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --features test-helpers -- --test-threads=1`
Expected: PASS, including the pre-existing `errors.rs` and `process_bounded_capture` suites. `cargo clippy --all-targets -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add src/errors.rs src/process.rs src/fetcher/ytdlp.rs
git commit -m "feat(errors): split RunError mapping, capture kill signal, type audio-decode and acquire failures

Resolves FOLLOWUPS T6 (Spawn/Io collapse + signal loss), T11 findings 1-2,
T5-Epic1 AudioDecode-as-Bug. Feeds ADR 0033's classifier (Task 03)."
```

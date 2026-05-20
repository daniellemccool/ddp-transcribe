use std::time::Duration;

use thiserror::Error;
use tokio::process::Command;
use tokio::time::timeout;

use crate::errors::FetchError;

#[derive(Debug)]
pub struct CommandSpec<'a> {
    pub program: &'static str,
    pub args: Vec<String>,
    pub timeout: Duration,
    /// Last-N bytes of stderr retained in `CommandOutcome.stderr_excerpt`.
    /// Bounded by construction (0021): the streaming reader maintains a
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
    /// by `stdout_capture_bytes`. Per 0002, `#[allow(dead_code)]` retained
    /// because the bin currently sets `stdout_capture_bytes: 0` at every
    /// call site (yt-dlp); the field is part of the lib API surface,
    /// exercised by the integration tests.
    #[allow(dead_code)]
    pub stdout: Option<Vec<u8>>,
    pub stderr_excerpt: String,
    #[allow(dead_code)]
    pub elapsed: Duration,
}

#[derive(Debug, Error)]
pub enum RunError {
    #[error("failed to spawn `{tool}`: {source}")]
    Spawn {
        tool: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("subprocess `{tool}` timed out after {duration:?}")]
    Timeout {
        tool: &'static str,
        duration: Duration,
    },

    #[error("io error reading subprocess output for `{tool}`: {source}")]
    Io {
        tool: &'static str,
        #[source]
        source: std::io::Error,
    },
}

// Plan A coarse mapping: Spawn (environmental, e.g. binary missing) and Io
// (system pipe error) both map to NetworkError, which is semantically wrong.
// Plan B's failure classification (RetryableKind / UnavailableReason) will
// need to revisit this — see docs/FOLLOWUPS.md.
impl From<RunError> for FetchError {
    fn from(err: RunError) -> Self {
        match err {
            RunError::Timeout { tool, duration } => FetchError::ToolTimeout { tool, duration },
            RunError::Spawn { tool, source } => {
                FetchError::NetworkError(format!("failed to spawn {}: {}", tool, source))
            }
            RunError::Io { tool, source } => {
                FetchError::NetworkError(format!("io error reading {} output: {}", tool, source))
            }
        }
    }
}

/// Bounded streaming reader. Drains the input fully, keeping at most `cap`
/// trailing bytes in memory at any moment. Returns `Some(bounded Vec)` when
/// `cap > 0` (capture requested) and `None` when `cap == 0` (intentional
/// discard — bytes are still drained to prevent the child blocking on a
/// full pipe, but not retained).
///
/// 0021 invariant: peak retained memory is bounded by `cap` regardless of
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

    let mut deque: std::collections::VecDeque<u8> = std::collections::VecDeque::with_capacity(cap);
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

#[tracing::instrument(level = "debug", skip(spec), fields(tool = spec.program))]
pub async fn run(spec: CommandSpec<'_>) -> Result<CommandOutcome, RunError> {
    let started = std::time::Instant::now();

    let logged_args: Vec<String> = spec
        .args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if spec.redact_arg_indices.contains(&i) {
                "<redacted>".into()
            } else {
                a.clone()
            }
        })
        .collect();
    tracing::debug!(args = ?logged_args, "spawning subprocess");

    let mut child = Command::new(spec.program)
        .args(&spec.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| RunError::Spawn {
            tool: spec.program,
            source,
        })?;

    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");

    // Bounded streaming reads (0021). Peak memory is bounded by
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
            // Timed out. Defense-in-depth termination:
            //   1. Explicit `start_kill()` sends SIGKILL immediately.
            //   2. `kill_on_drop(true)` (set on spawn) sends SIGKILL again on
            //      drop as a backstop if a future refactor changes this control
            //      flow (e.g., moves `child` out of scope earlier).
            // The second SIGKILL is a no-op on an already-exiting process.
            // Both mechanisms intentional — do not "clean up" by removing one.
            let _ = child.start_kill();
            Err(RunError::Timeout {
                tool: spec.program,
                duration: spec.timeout,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn false_returns_nonzero_exit() {
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
    async fn timeout_kills_long_running_subprocess() {
        let spec = CommandSpec {
            program: "sleep",
            args: vec!["10".into()],
            timeout: Duration::from_millis(200),
            stderr_capture_bytes: 1024,
            stdout_capture_bytes: 0,
            redact_arg_indices: &[],
        };
        let result = run(spec).await;
        match result {
            Err(RunError::Timeout { tool, .. }) => assert_eq!(tool, "sleep"),
            other => panic!("expected timeout, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn missing_program_returns_spawn_error() {
        let spec = CommandSpec {
            program: "this-program-does-not-exist-1234567",
            args: vec![],
            timeout: Duration::from_secs(5),
            stderr_capture_bytes: 1024,
            stdout_capture_bytes: 0,
            redact_arg_indices: &[],
        };
        let result = run(spec).await;
        match result {
            Err(RunError::Spawn { .. }) => {}
            other => panic!("expected Spawn error, got {:?}", other),
        }
    }
}

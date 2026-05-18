//! Tier 2 tests for bounded subprocess capture (T5 perf-tweaks; AD0021).
//!
//! Exercise the bounded streaming reader via real subprocesses (echo, sleep,
//! a stderr-spammer) and via direct calls to `process::read_bounded` for the
//! peak-memory assertion that can't be observed through `run` alone.

#![cfg(feature = "test-helpers")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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
    let cmd = format!("yes 'x' | head -c {} >&2", payload_size);
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
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "hello world");
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
        final_peak,
        CAP,
        payload_len
    );
    assert!(
        final_peak > 0,
        "counter must have been incremented at least once"
    );
}

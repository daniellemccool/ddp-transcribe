use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::errors::TranscribeError;

// ============================================================================
// Plan B Epic 1: TranscribeOutput types
// ============================================================================
//
// Pass-through raw signals from whisper.cpp's C API via the whisper-rs binding.
// See 0010 (raw_signals schema), 0016 (worker-thread invariants).
//
// These types are OWNED data: no references, no whisper-rs handles. They cross
// the worker-thread boundary safely (0016 #1: owned data only).

use serde::{Deserialize, Serialize};

/// Owned output from a single whisper inference. Crosses the worker-thread
/// boundary (0016). T10's artifact writer maps these fields across the
/// artifact JSON: `text` and `model_id` land at the top level (alongside
/// Plan A's existing metadata), while `language`, `lang_probs`, and `segments`
/// are placed inside the `raw_signals` sub-object (0010). This struct is
/// the worker-return type, not a 1:1 mirror of `raw_signals`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscribeOutput {
    /// Concatenated text of all segments.
    pub text: String,
    /// Detected language as a single ISO code, e.g. "en" or "nl".
    /// From whisper_full_lang_id() (free per inference).
    pub language: String,
    /// Per-language probability vector, ONLY when PerCallConfig::compute_lang_probs is true.
    /// Costs one extra encoder pass per video (sharp-edges.md:13).
    pub lang_probs: Option<Vec<(String, f32)>>,
    /// Per-segment raw confidence signals.
    pub segments: Vec<SegmentRaw>,
    /// Model identifier, e.g. "ggml-large-v3-turbo-q5_0.bin".
    /// Already captured by Plan A's metadata.
    pub model_id: String,
}

/// Per-segment raw confidence signals from whisper.cpp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentRaw {
    /// whisper_full_get_segment_no_speech_prob(state, i)
    pub no_speech_prob: f32,
    /// Per-token confidence signals for this segment.
    pub tokens: Vec<TokenRaw>,
}

/// Per-token confidence signals from whisper.cpp.
///
/// `id` and `text` carry token identity so downstream consumers can filter
/// special tokens (`[BEG]`, `[END]`, `<|en|>`, etc.) per 0010's pass-through
/// rule — the prior shape (only `p`/`plog`) numerically included specials but
/// gave consumers no way to identify them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenRaw {
    /// Token id as an index into the model's vocabulary
    /// (`WhisperToken::token_id()`). Special tokens (timestamp, language, BEG,
    /// END, NOT, SOT, EOT, etc.) have id values documented in whisper.cpp.
    pub id: i32,
    /// Token text from `WhisperToken::to_str_lossy()`. May contain non-UTF-8
    /// fragments for multi-byte tokens that span split-points; lossy variant
    /// substitutes replacement chars rather than failing the whole extraction.
    pub text: String,
    /// whisper_full_get_token_p(state, i, j) — token probability in [0.0, 1.0]
    pub p: f32,
    /// Token log-probability (TokenData::plog from whisper-rs)
    pub plog: f32,
}

/// Extract per-segment and per-token raw confidence signals from whisper state.
///
/// 0003 deviation: whisper-rs 0.16.0 does not expose flat
/// `full_get_segment_no_speech_prob` / `full_n_tokens` / `full_get_token_data`
/// methods on `WhisperState`. Everything is accessed via the wrapper types
/// `WhisperSegment` (via `state.get_segment(i)`) and `WhisperToken`
/// (via `seg.get_token(j)`). `WhisperSegment::no_speech_probability()` and
/// `WhisperToken::token_data()` return values directly (not `Result`), so there
/// is no getter-error path to skip — non-finite values are the only error
/// condition and are surfaced as `Err(detail)`.
///
/// Returns `Ok(Vec<SegmentRaw>)` on success, or `Err(String)` with a
/// human-readable diagnostic when a non-finite f32 is encountered (codex T4
/// review forward-pointer: non-finite values must surface as `TranscribeError::Bug`).
///
/// Special tokens (`[BEG]`, `[END]`, language tokens like `<|en|>`, etc.) are
/// retained per 0010's pass-through rule — downstream consumers filter them.
fn extract_segments(state: &whisper_rs::WhisperState) -> Result<Vec<SegmentRaw>, String> {
    let n_segments = state.full_n_segments();
    if n_segments < 0 {
        return Err(format!(
            "whisper-rs returned negative n_segments: {n_segments}"
        ));
    }
    // n_segments is guaranteed >= 0 by the check above, so the cast cannot lose sign.
    #[allow(clippy::cast_sign_loss)]
    let mut segments_raw = Vec::with_capacity(n_segments as usize);

    for i in 0..n_segments {
        // `get_segment` returns None only when `i` is out of bounds — but we
        // are iterating 0..n_segments so this is an invariant violation if it
        // fires. Treat it as a Bug.
        let seg = state
            .get_segment(i)
            .ok_or_else(|| format!("whisper-rs returned None for in-bounds segment {i}"))?;

        let no_speech_prob = seg.no_speech_probability();
        if !no_speech_prob.is_finite() || !(0.0..=1.0).contains(&no_speech_prob) {
            return Err(format!(
                "whisper-rs returned out-of-range no_speech_prob at segment {i}: \
                 {no_speech_prob} (expected finite, [0.0, 1.0])"
            ));
        }

        let n_tokens = seg.n_tokens();
        if n_tokens < 0 {
            return Err(format!(
                "whisper-rs returned negative n_tokens at segment {i}: {n_tokens}"
            ));
        }
        // n_tokens is guaranteed >= 0 by the check above, so the cast cannot lose sign.
        #[allow(clippy::cast_sign_loss)]
        let mut tokens_raw = Vec::with_capacity(n_tokens as usize);

        for j in 0..n_tokens {
            // Same invariant argument as for segments above.
            let tok = seg.get_token(j).ok_or_else(|| {
                format!("whisper-rs returned None for in-bounds token {j} in segment {i}")
            })?;

            let td = tok.token_data();
            if !td.p.is_finite() || !(0.0..=1.0).contains(&td.p) {
                return Err(format!(
                    "whisper-rs returned out-of-range p at segment {i} token {j}: \
                     {p} (expected finite, [0.0, 1.0])",
                    p = td.p,
                ));
            }
            if !td.plog.is_finite() || td.plog > 0.0001 {
                return Err(format!(
                    "whisper-rs returned invalid plog at segment {i} token {j}: \
                     {pl} (expected finite, <= 0)",
                    pl = td.plog,
                ));
            }

            // Token text via to_str_lossy: substitutes replacement chars on
            // non-UTF-8 byte sequences (common for multi-byte tokens that span
            // split-points). Better than erroring out and losing the artifact.
            let text = tok
                .to_str_lossy()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default();

            tokens_raw.push(TokenRaw {
                id: tok.token_id(),
                text,
                p: td.p,
                plog: td.plog,
            });
        }

        segments_raw.push(SegmentRaw {
            no_speech_prob,
            tokens: tokens_raw,
        });
    }

    Ok(segments_raw)
}

#[cfg(test)]
mod plan_b_tests {
    use super::*;

    fn sample_output() -> TranscribeOutput {
        TranscribeOutput {
            text: "Hello world".to_string(),
            language: "en".to_string(),
            lang_probs: None,
            segments: vec![SegmentRaw {
                no_speech_prob: 0.02,
                tokens: vec![
                    TokenRaw {
                        id: 1000,
                        text: "Hello".to_string(),
                        p: 0.99,
                        plog: -0.01,
                    },
                    TokenRaw {
                        id: 1001,
                        text: " world".to_string(),
                        p: 0.95,
                        plog: -0.05,
                    },
                ],
            }],
            model_id: "ggml-tiny.en.bin".to_string(),
        }
    }

    #[test]
    fn transcribe_output_round_trip() {
        let before = sample_output();
        let json = serde_json::to_string(&before).expect("serialize");
        let after: TranscribeOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(before, after);
    }

    #[test]
    fn lang_probs_none_serializes_as_null() {
        let output = sample_output();
        let json = serde_json::to_value(&output).expect("serialize");
        assert_eq!(json["lang_probs"], serde_json::Value::Null);
    }

    #[test]
    fn lang_probs_some_serializes_as_array_of_pairs() {
        let mut output = sample_output();
        output.lang_probs = Some(vec![("en".to_string(), 0.93), ("nl".to_string(), 0.05)]);
        let json = serde_json::to_value(&output).expect("serialize");
        let arr = json["lang_probs"].as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0][0], "en");
        assert!((arr[0][1].as_f64().unwrap() - 0.93).abs() < 1e-6);
    }

    #[test]
    fn empty_segments_round_trip() {
        let mut output = sample_output();
        output.segments = vec![];
        let json = serde_json::to_string(&output).expect("serialize");
        let after: TranscribeOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(output, after);
    }
}

// ============================================================================
// Plan B Epic 1: WhisperEngine shell (T5)
// ============================================================================
//
// Worker-thread architecture per 0016:
// - Only owned data crosses the boundary (samples, configs, output structs)
// - WhisperContext/WhisperState stay inside the worker thread (T6/T7)
// - Closed oneshot reply is Bug-class during normal execution; 0016 comment-2
//   carves out shutdown (relevant when Epic 2 wires shutdown signaling).
//
// Per-request cancellation per 0012 (+ comment-2 refinement):
// - Each request carries its own Arc<AtomicBool> for operator-initiated cancel
//   (per-request, never shared across requests — 0012's no-leak invariant).
// - Each request carries its own `deadline: Instant` for per-call timeout.
// - T7's abort_callback polls BOTH inside whisper.cpp's encoder/decoder loop;
//   no separate timer task is spawned (deviates from the T5 brief's tokio::spawn
//   sketch per 0012 comment-2; see 0003 deviation disclosure in commit body).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use tokio::sync::{mpsc, oneshot};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub model_path: PathBuf,
    pub gpu_device: i32,
    pub flash_attn: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PerCallConfig {
    /// Some("en") to pin; None for auto-detect.
    pub language: Option<String>,
    /// If true, an extra encoder pass populates TranscribeOutput::lang_probs.
    /// See sharp-edges.md:13 — calling lang_detect re-encodes the audio.
    pub compute_lang_probs: bool,
}

#[derive(Debug)]
pub(crate) struct TranscribeRequest {
    pub samples: Vec<f32>,
    pub config: PerCallConfig,
    /// Per-request cancel flag (0012). Operator-initiated cancellation flips
    /// this; T7's abort_callback polls it. Never shared across requests.
    pub cancel: Arc<AtomicBool>,
    /// Per-call deadline (0012 comment-2). T7's abort_callback polls
    /// `Instant::now() >= deadline` directly — no separate timer task.
    pub deadline: Instant,
    pub reply: oneshot::Sender<Result<TranscribeOutput, TranscribeError>>,
}

#[derive(Debug, thiserror::Error)]
pub enum WhisperInitError {
    #[error("loading whisper model from {path}: {detail}")]
    ModelLoad { path: String, detail: String },

    /// 0013: whisper.cpp's init log did not prove the backend this build
    /// expects. Carries BOTH sides so the operator learns what was expected
    /// and what actually happened without re-reading the log.
    #[error(
        "backend mismatch: this build expects {expected}, but whisper.cpp's init log reported \
         {detected} — a CPU fallback runs ~100x slower and would silently invalidate the batch \
         (0013; sharp-edges.md:61)"
    )]
    BackendMismatch { expected: String, detected: String },

    #[error("creating whisper state: {detail}")]
    StateCreate { detail: String },

    #[error("spawning whisper worker thread: {detail}")]
    WorkerSpawn { detail: String },
}

// ============================================================================
// Plan B Epic 5b (T08): whisper.cpp log bridge + 0013 backend assertion
// ============================================================================
//
// 0013 requires engine construction to prove the backend whisper.cpp actually
// engaged, because a silent CPU fallback produces a run that looks completely
// normal and is ~100x slower. The only place whisper.cpp states its backend is
// its init log, so we route that log into Rust and parse it.
//
// The cross-epic FOLLOWUPS invariant governs the mechanism: `whisper_log_set`
// is PROCESS-GLOBAL, not per-context (deepdive sharp-edges.md:65 — the global
// `g_state` holds exactly this callback and nothing else). So:
//
//   1. the callback is installed exactly ONCE, via `Once`, before any context
//      initialization (see `install_log_bridge` call sites);
//   2. every whisper.cpp line flows through this ONE bridge to `tracing` at
//      debug level — never `eprintln`, never a per-engine callback swap;
//   3. backend capture is phase-scoped: `InitCapture` holds a global phase
//      mutex for the duration of one engine's init (context construction AND
//      its primary `create_state`, which is where whisper.cpp v1.8.3 actually
//      selects the backend), so two engines initializing concurrently cannot
//      interleave their init lines into one buffer.
//
// Binding note: whisper-rs 0.16.0 also ships `install_logging_hooks()`, but it
// wires whisper-rs's OWN trampolines straight into `log`/`tracing` with no seam
// to capture the text — and the crate is vendored here with
// `default-features = false`, so neither of its logging backends is even
// compiled. We therefore install our own callback through the crate's thin
// re-export of the raw C call, `whisper_rs::set_log_callback`
// (= `whisper_rs_sys::whisper_log_set`), which is the documented
// `whisper_log_set`-equivalent binding and the only one that yields the text.
// Note it also captures ggml's lines during init: `whisper_backend_init_gpu`
// forwards whisper's callback to `ggml_log_set` on entry (whisper.cpp:1291),
// which is how the `ggml_cuda_init` device banner reaches us.

/// What backend this build is entitled to demand.
///
/// The `cuda` feature is the gate the cross-epic FOLLOWUPS entry asks for:
/// a CUDA build hard-fails on CPU fallback; every other build only logs,
/// because `use_gpu(true)` on a CPU-only build is the expected local-dev path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedBackend {
    /// `--features cuda`: whisper.cpp MUST report a GPU backend.
    Gpu,
    /// Any other build: report the backend, assert nothing.
    Unconstrained,
}

/// The gate itself. Written as `cfg!` rather than a `#[cfg]`-split pair of
/// consts deliberately: both variants stay *constructed* in the source, so
/// neither arm rots into `dead_code` on the build that doesn't select it and
/// no `#[allow]` is needed (0002). `cfg!` is still resolved at compile time,
/// so this is the same feature gate the cross-epic FOLLOWUPS entry asks for.
/// Matches the `flash_attn: cfg!(feature = "cuda")` idiom in `commands.rs`.
const EXPECTED_BACKEND: ExpectedBackend = if cfg!(feature = "cuda") {
    ExpectedBackend::Gpu
} else {
    ExpectedBackend::Unconstrained
};

/// What whisper.cpp's init log says actually happened.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DetectedBackend {
    /// whisper.cpp selected a GPU device AND its backend initialized;
    /// `device` is ggml's device name (e.g. `CUDA0`) as it appears in the
    /// `using ... backend` line.
    Gpu { device: String },
    /// whisper.cpp selected a GPU device but `ggml_backend_dev_init` failed
    /// (whisper.cpp:1321-1324), so `whisper_backend_init_gpu` returned nullptr
    /// and the caller fell silently through to ACCEL/CPU
    /// (whisper.cpp:1332-1358). Kept distinct from `Cpu` because the operator's
    /// remedy differs — a GPU was found, so this is a driver/runtime fault, not
    /// a missing device — and because saying "no GPU found" here would be a lie.
    /// Rejected by a CUDA build exactly like `Cpu`.
    GpuInitFailed { device: String },
    /// whisper.cpp walked the device list and found no GPU — the silent
    /// fallback 0013 exists to catch.
    Cpu,
    /// No verdict line appeared (including a log truncated before whisper.cpp
    /// resolved a backend). Not proof of anything, and therefore not proof of
    /// GPU use: a CUDA build treats this as a failure (fail closed).
    Unknown,
}

impl std::fmt::Display for ExpectedBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gpu => write!(f, "a GPU backend (built with --features cuda)"),
            Self::Unconstrained => write!(f, "any backend (built without --features cuda)"),
        }
    }
}

impl std::fmt::Display for DetectedBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gpu { device } => write!(f, "GPU backend {device}"),
            Self::GpuInitFailed { device } => write!(
                f,
                "a CPU fallback after GPU backend {device} was selected but failed to start \
                 (whisper.cpp logged `failed to initialize {device} backend`)"
            ),
            Self::Cpu => write!(f, "CPU (whisper.cpp logged `no GPU found`)"),
            Self::Unknown => write!(
                f,
                "an unrecognized backend (the init log reached no whisper_backend_init_gpu verdict)"
            ),
        }
    }
}

/// `whisper_backend_init_gpu: using CUDA0 backend` (whisper.cpp:1320).
/// The `_gpu` in the prefix is load-bearing: `whisper_backend_init` (no
/// suffix) logs the identical wording for ACCEL backends such as BLAS
/// (whisper.cpp:1342), which are emphatically not GPUs.
const GPU_BACKEND_LINE_PREFIX: &str = "whisper_backend_init_gpu: using ";
/// `whisper_backend_init_gpu: failed to initialize CUDA0 backend`
/// (whisper.cpp:1323). Note the ACCEL loop logs the SAME wording under the
/// `whisper_backend_init:` prefix (whisper.cpp:1346); a BLAS failure must not
/// be read as a GPU failure, hence the `_gpu`-anchored prefix here too.
const GPU_BACKEND_FAILED_PREFIX: &str = "whisper_backend_init_gpu: failed to initialize ";
const GPU_BACKEND_LINE_SUFFIX: &str = " backend";
/// `whisper_backend_init_gpu: no GPU found` (whisper.cpp:1316).
const NO_GPU_LINE: &str = "whisper_backend_init_gpu: no GPU found";

/// Strip `<prefix>DEVICE backend` down to `DEVICE`.
fn parse_backend_device<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let device = line
        .strip_prefix(prefix)?
        .strip_suffix(GPU_BACKEND_LINE_SUFFIX)?;
    (!device.is_empty()).then_some(device)
}

/// Parse whisper.cpp's captured init log into the backend it engaged.
///
/// Pure and GPU-free, so the whole decision is unit-testable on a CPU box
/// (see `backend_assertion_tests`).
///
/// This is an ORDERED parse, not a substring search, because `using X backend`
/// is logged BEFORE `ggml_backend_dev_init` is even attempted
/// (whisper.cpp:1320-1321). A `using X` line is therefore only a *pending*
/// claim; it is retracted by a following `failed to initialize X backend`, on
/// which whisper.cpp returns nullptr and its caller falls silently through to
/// ACCEL/CPU. Treating the pending claim as proof would let a CUDA build pass
/// this assertion while running on CPU — the precise failure 0013 exists to
/// catch.
///
/// Precedence, most to least conclusive:
///   1. a `using X` that survived to the end of the phase unretracted → `Gpu`
///      (so a failed attempt followed by a later successful one still passes);
///   2. otherwise a retracted attempt → `GpuInitFailed`;
///   3. otherwise `no GPU found` → `Cpu`;
///   4. otherwise → `Unknown` (fail closed; includes truncated logs).
fn detect_backend(log: &str) -> DetectedBackend {
    // The most recent unretracted `using X backend` claim.
    let mut pending: Option<&str> = None;
    // The most recent retracted claim, kept for the diagnostic.
    let mut failed: Option<&str> = None;
    let mut saw_no_gpu = false;

    for line in log.lines() {
        let line = line.trim_end();

        if let Some(device) = parse_backend_device(line, GPU_BACKEND_LINE_PREFIX) {
            pending = Some(device);
        } else if let Some(device) = parse_backend_device(line, GPU_BACKEND_FAILED_PREFIX) {
            failed = Some(device);
            // Retract only the claim this failure actually names. A failure
            // naming some other device leaves an unrelated pending claim
            // standing rather than silently invalidating it.
            if pending == Some(device) {
                pending = None;
            }
        } else if line == NO_GPU_LINE {
            saw_no_gpu = true;
        }
    }

    match (pending, failed, saw_no_gpu) {
        (Some(device), _, _) => DetectedBackend::Gpu {
            device: device.to_string(),
        },
        (None, Some(device), _) => DetectedBackend::GpuInitFailed {
            device: device.to_string(),
        },
        (None, None, true) => DetectedBackend::Cpu,
        (None, None, false) => DetectedBackend::Unknown,
    }
}

/// Pull the human-readable device name out of ggml's init banner, e.g.
/// `  Device 0: NVIDIA A10, compute capability 8.6, VMM: yes`
/// (ggml-cuda.cu:267). 0013 wants the device NAME in the operator log, not
/// just ggml's `CUDA0` handle. Absent on CPU-only inits, hence `Option`.
fn detect_device_description(log: &str) -> Option<String> {
    log.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("Device ")?;
        let (index, description) = rest.split_once(": ")?;
        // Guard against matching unrelated prose: ggml always prints an index.
        index.parse::<u32>().ok()?;
        let description = description.trim();
        (!description.is_empty()).then(|| description.to_string())
    })
}

/// The 0013 decision. Hard-fails a CUDA build whose init log does not prove a
/// GPU — including the `Unknown` case, because "we could not tell" is not
/// proof of GPU use and 0013 rejects softening the contract to a warning.
fn check_backend(
    expected: ExpectedBackend,
    detected: &DetectedBackend,
) -> Result<(), WhisperInitError> {
    match (expected, detected) {
        (ExpectedBackend::Unconstrained, _)
        | (ExpectedBackend::Gpu, DetectedBackend::Gpu { .. }) => Ok(()),
        (
            ExpectedBackend::Gpu,
            DetectedBackend::GpuInitFailed { .. } | DetectedBackend::Cpu | DetectedBackend::Unknown,
        ) => Err(WhisperInitError::BackendMismatch {
            expected: expected.to_string(),
            detected: detected.to_string(),
        }),
    }
}

/// One-shot install guard for the process-global log callback.
static LOG_BRIDGE_INSTALL: std::sync::Once = std::sync::Once::new();

/// The active init-phase capture buffer. `Some` only while an `InitCapture`
/// guard is alive. Locked briefly by the callback; never held across a
/// `INIT_PHASE` acquisition, so the two locks cannot deadlock.
static INIT_CAPTURE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Serializes init phases. Held for the whole of one `WhisperContext`
/// construction so concurrent engine inits cannot interleave their log lines
/// into a shared buffer (the cross-epic FOLLOWUPS synchronization requirement).
static INIT_PHASE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_capture() -> std::sync::MutexGuard<'static, Option<String>> {
    // Poison-tolerant: a panicking init must not permanently break logging,
    // and the buffer's contents are already treated as untrusted text.
    INIT_CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Install the process-global whisper.cpp log callback. Idempotent, cheap
/// after the first call, and safe to race — `Once` blocks later callers until
/// the install completes, so "installed before any context init" holds even if
/// two engines are constructed simultaneously.
fn install_log_bridge() {
    LOG_BRIDGE_INSTALL.call_once(|| {
        // SAFETY: `set_log_callback` is whisper-rs's thin wrapper over
        // `whisper_log_set`. Its contract is that the callback must be safe to
        // call from C: `log_bridge_trampoline` takes no user_data, does not
        // unwind (an `extern "C"` fn aborts rather than unwinding across the
        // FFI boundary), and only touches poison-tolerant statics. Installing
        // once, before any context init, is the FOLLOWUPS invariant.
        unsafe {
            whisper_rs::set_log_callback(Some(log_bridge_trampoline), std::ptr::null_mut());
        }
    });
}

/// FFI entry point for whisper.cpp/ggml log lines.
///
/// `level` is `ggml_log_level` (a `c_uint` in whisper-rs-sys' bindings). We
/// deliberately ignore it: whisper.cpp's severities do not map onto ours (its
/// routine init banner is INFO, and an operator debugging a run wants the raw
/// stream), so every line lands at `tracing` debug under one target.
unsafe extern "C" fn log_bridge_trampoline(
    _level: std::ffi::c_uint,
    text: *const std::ffi::c_char,
    _user_data: *mut std::ffi::c_void,
) {
    if text.is_null() {
        return;
    }
    // SAFETY: whisper.cpp passes a NUL-terminated C string it owns for the
    // duration of the call; we copy out of it before returning.
    let line = unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy();
    record_bridge_line(&line);
}

/// The bridge body, split out from the trampoline so it is reachable from
/// tests without an FFI round-trip.
fn record_bridge_line(line: &str) {
    let trimmed = line.trim_end();
    if !trimmed.is_empty() {
        tracing::debug!(target: "whisper_cpp", line = trimmed, "whisper.cpp");
    }

    // Append verbatim — whisper.cpp's own lines already carry their newline,
    // and ggml's CONT-level fragments deliberately do not.
    if let Some(buf) = lock_capture().as_mut() {
        buf.push_str(line);
    }
}

/// RAII scope for one init phase's log capture.
///
/// Holding `INIT_PHASE` for the guard's lifetime is what makes the captured
/// buffer attributable to exactly one `WhisperContext` construction. `Drop`
/// clears the buffer, so a panicking or early-returning init cannot leak its
/// lines into the next phase.
struct InitCapture {
    _phase: std::sync::MutexGuard<'static, ()>,
}

impl InitCapture {
    fn begin() -> Self {
        // Belt and braces: the bridge must exist before anything logs into it.
        install_log_bridge();
        let phase = INIT_PHASE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *lock_capture() = Some(String::new());
        Self { _phase: phase }
    }

    /// Copy out what has been captured so far. Cheap enough at init scale
    /// (a few dozen short lines).
    fn snapshot(&self) -> String {
        lock_capture().clone().unwrap_or_default()
    }
}

impl Drop for InitCapture {
    fn drop(&mut self) {
        *lock_capture() = None;
    }
}

/// FFI trampoline for whisper.cpp's abort_callback. `user_data` must be the
/// raw pointer returned by `Box::into_raw(Box::new(closure))` where `closure`
/// is `Box<dyn FnMut() -> bool>`. See the 0003 deviation comment inside the
/// worker loop for why we hand-roll this instead of using
/// `FullParams::set_abort_callback_safe`.
unsafe extern "C" fn abort_trampoline(user_data: *mut std::ffi::c_void) -> bool {
    // SAFETY: `user_data` is the pointer produced by `Box::into_raw(Box::new(cb))`
    // where `cb: Box<dyn FnMut() -> bool>` (see the set_abort_callback site below).
    // whisper.cpp hands it back unchanged on each callback and the Box outlives
    // inference (it is reclaimed via Box::from_raw only after `state.full` returns),
    // so the reborrow is to a live, unaliased value.
    let cb = unsafe { &mut *user_data.cast::<Box<dyn FnMut() -> bool>>() };
    cb()
}

/// Drop guard that flips the per-request cancel flag when the caller's
/// `transcribe()` future is dropped before the worker replies. Without this,
/// a caller cancelling the future would leave the worker chewing on an
/// orphaned request whose result no one will read. Per 0012 comment-2,
/// the cancel flag is the operator-initiated cancellation channel; future-drop
/// is a special case of operator-initiated.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Worker-thread-owning engine handle. See 0016 for the parallelism contract
/// (engine API stays stable across single- and multi-state worker pools).
///
/// Both fields are `Option` so `shutdown()` and `Drop::drop` can share the same
/// teardown sequence: drop the sender FIRST (closes the channel, lets the
/// worker's `blocking_recv` return `None`), THEN join. If the sender were
/// dropped after the join attempt, the worker would park forever in
/// `blocking_recv` and the join would hang. (Brief code had this hazard;
/// 0003 deviation — see commit body.)
pub struct WhisperEngine {
    request_tx: Option<mpsc::Sender<TranscribeRequest>>,
    handle: Option<thread::JoinHandle<()>>,
    /// Counter incremented each time the worker thread lazily allocates
    /// `lang_state` (at most once per worker lifetime). Always present so the
    /// worker capture doesn't branch on a feature flag; only **read** outside
    /// the worker via the `test-helpers` getter below.
    // 0002: this allow survives the Epic 5b purge. The field is private and
    // written-to via a cloned `Arc` in the worker, but its only reader is
    // `WhisperEngine::lang_state_allocations` below, gated on
    // `feature = "test-helpers"` — so a default build genuinely has no read
    // and fires `dead_code`. The named consumer is `tests/transcribe_lang_state.rs`
    // (0005/0016). Deleting this allow requires the counter to gain a
    // production reader; `#[expect]` is not an option because the deadness is
    // configuration-dependent (0002 Guidance).
    #[allow(dead_code)]
    lang_state_allocations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl WhisperEngine {
    /// Construct a WhisperEngine: spawn the worker thread, load the model,
    /// verify init, return the handle.
    ///
    /// **Blocks the caller** until the worker reports init success or failure
    /// via the internal rendezvous channel. Model load for tiny.en is ~1s on
    /// CPU and faster on GPU; for large-v3-turbo expect a few seconds. Call
    /// from a sync startup path (e.g., main()'s setup before the tokio runtime
    /// hands off to async work) — not from inside a latency-sensitive async
    /// task, because the rendezvous recv() will block the executor thread.
    pub fn new(config: &EngineConfig) -> Result<Self, WhisperInitError> {
        // 0013: the whisper.cpp log bridge is process-global and must exist
        // before ANY context initialization. Installing here — before the
        // worker is even spawned — means no init line can predate it, on any
        // path that reaches a `WhisperContext`. `InitCapture::begin` calls it
        // again for defence in depth; `Once` makes both cheap.
        install_log_bridge();

        // Channel capacity 1: each TranscribeRequest carries a Vec<f32> of decoded
        // audio (~MB scale for a single-minute video). Epic 1's serial pipeline
        // never needs more than one request in flight. Epic 2's pipelined
        // orchestrator decides its own outer queue depth.
        let (request_tx, mut request_rx) = mpsc::channel::<TranscribeRequest>(1);

        let model_path = config.model_path.clone();
        let gpu_device = config.gpu_device;
        let flash_attn = config.flash_attn;

        // Rendezvous channel to surface init errors back to the caller before
        // the worker enters its request loop. std::sync::mpsc since the worker
        // is a std::thread and the caller (this fn) is synchronous.
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<(), WhisperInitError>>(0);

        // T2 perf-tweaks: lang_state is now lazily allocated inside the worker
        // on the first `compute_lang_probs=true` request. This Arc<AtomicUsize>
        // is the only thing about the lazy lifecycle that crosses the worker
        // boundary (read-only from outside via the test-helpers getter).
        // 0016 invariant preserved: WhisperState stays inside the worker.
        let lang_state_allocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lang_state_allocations_worker = std::sync::Arc::clone(&lang_state_allocations);

        let handle = thread::Builder::new()
            .name("ddp-transcribe-whisper-worker".to_string())
            .spawn(move || {
                // whisper-rs 0.16.0: setters take &mut self and return &mut Self.
                // use_gpu(true) is harmless on a CPU build — whisper.cpp falls
                // back, and `EXPECTED_BACKEND` is `Unconstrained` there. On a
                // cuda build that same fallback is a hard failure (0013).
                let mut ctx_params = WhisperContextParameters::default();
                ctx_params
                    .use_gpu(true)
                    .flash_attn(flash_attn)
                    .gpu_device(gpu_device);

                // 0013: capture whisper.cpp's init log across the WHOLE init
                // phase. The guard holds the global init-phase lock, so a second
                // engine constructed concurrently waits rather than mixing its
                // lines into ours, and `Drop` clears the buffer on every exit
                // path below.
                //
                // The phase deliberately spans BOTH the context construction and
                // the primary `create_state()`: at the pinned whisper.cpp v1.8.3
                // the backend is selected in `whisper_init_state`
                // (whisper.cpp:3377 calls `whisper_backend_init`), NOT in the
                // `..._no_state` constructor. Capturing only the constructor
                // yields an empty backend verdict — verified against a real
                // tiny.en init, where `whisper_backend_init_gpu: no GPU found`
                // lands between `whisper_model_load` and `whisper_init_state`.
                let capture = InitCapture::begin();

                // Allocate WhisperState ONCE in the init phase and reuse it for
                // every request. Per whisper.cpp's concurrency model
                // (see whisper-cpp deepdive concurrency.md + sharp-edges.md:21):
                // WhisperState owns ~500MB-1GB of KV caches and compute
                // buffers; allocating one per request would defeat Plan B's
                // efficiency goal. `whisper_full_with_state` clears `result_all`
                // on entry (sharp-edges.md:19), so state reuse across calls is
                // safe. Epic 1 ships single-state; Plan C may allocate N states
                // per context for intra-GPU parallelism (0016 architecture).
                //
                // `ctx` and `state` live until this closure exits — keep the
                // model in memory for the worker's lifetime. 0016:
                // WhisperContext and WhisperState stay inside the worker
                // thread; they never escape.
                //
                // whisper-rs 0.16.0 accepts P: AsRef<Path>; pass the PathBuf directly.
                // 0003 deviation from brief sketch (brief did .to_str().unwrap_or("")).
                let ctx = match WhisperContext::new_with_params(&model_path, ctx_params) {
                    Ok(c) => {
                        tracing::info!(
                            gpu_device = gpu_device,
                            flash_attn = flash_attn,
                            model_path = %model_path.display(),
                            "WhisperEngine: model loaded"
                        );
                        c
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(WhisperInitError::ModelLoad {
                            path: model_path.display().to_string(),
                            detail: format!("{e}"),
                        }));
                        return;
                    }
                };

                let state_result = ctx.create_state();
                let init_log = capture.snapshot();
                drop(capture);

                let mut state = match state_result {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = init_tx.send(Err(WhisperInitError::StateCreate {
                            detail: format!("primary state: {e}"),
                        }));
                        return;
                    }
                };

                // 0013: the backend/device audit line, on every init, plus the
                // assertion. This still runs at CONSTRUCTION — `WhisperEngine::new`
                // is blocked on `init_rx` until the `Ok(())` below — so an
                // operator learns about a CPU fallback before any batch work
                // starts, never later and never as a warning.
                let detected = detect_backend(&init_log);
                let device_description = detect_device_description(&init_log);
                let (backend, device) = match &detected {
                    DetectedBackend::Gpu { device } => ("GPU", device.as_str()),
                    // The GPU was found and named but never started: report the
                    // CPU reality, and keep the device so the audit line says
                    // WHICH backend failed to come up.
                    DetectedBackend::GpuInitFailed { device } => {
                        ("CPU (GPU init failed)", device.as_str())
                    }
                    DetectedBackend::Cpu => ("CPU", "none"),
                    DetectedBackend::Unknown => ("unknown", "none"),
                };
                tracing::info!(
                    backend = backend,
                    device = device,
                    device_name = device_description.as_deref().unwrap_or("unreported"),
                    gpu_device = gpu_device,
                    expected = %EXPECTED_BACKEND,
                    "WhisperEngine: whisper.cpp backend (0013)"
                );
                if let Err(e) = check_backend(EXPECTED_BACKEND, &detected) {
                    let _ = init_tx.send(Err(e));
                    return;
                }

                // T2 perf-tweaks: secondary state used only for opt-in
                // lang_detect is now **lazily allocated** on the first
                // `compute_lang_probs=true` request. Non-opt-in workers pay
                // zero VRAM/host overhead for lang_state; opt-in workers pay
                // exactly once (on first use), with subsequent opt-in
                // requests reusing the same state. See sharp-edges.md:15 —
                // whisper_lang_auto_detect_with_state clobbers state (reuses
                // decoders[0] and logits), so it must NOT run on the primary
                // state used for inference. 0016: this WhisperState stays
                // inside the worker thread; the only thing crossing out is
                // the `lang_state_allocations` counter Arc.
                let mut lang_state: Option<whisper_rs::WhisperState> = None;
                let lang_state_allocations = lang_state_allocations_worker;

                // Init success: model loaded and primary state allocated.
                if init_tx.send(Ok(())).is_err() {
                    return; // caller went away
                }

                // model_id is derived from the path file_name once, outside the
                // hot loop. 0010: this lands in the artifact's top-level
                // `model_id` field; T9/T10 thread it through.
                let model_id = model_path
                    .file_name()
                    .and_then(|os| os.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                while let Some(req) = request_rx.blocking_recv() {
                    // Lazy lang_state allocation per T2 perf-tweaks. The
                    // `WhisperContext::create_state` call is non-trivial (a
                    // second mel encoder + decoder context on the same
                    // model). Defer until first opt-in request. 0016: state
                    // stays inside this thread; the counter Arc is the only
                    // thing that crosses out.
                    if req.config.compute_lang_probs && lang_state.is_none() {
                        match ctx.create_state() {
                            Ok(s) => {
                                lang_state = Some(s);
                                lang_state_allocations
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            Err(e) => {
                                // Failure on the lazy path: surface as
                                // TranscribeError::Bug (matches the existing
                                // AudioDecodeError -> Bug convention at
                                // src/errors.rs; Epic 3's failure-classification
                                // taxonomy will reclassify). Worker continues
                                // so subsequent non-opt-in requests still work.
                                let _ = req.reply.send(Err(TranscribeError::Bug {
                                    detail: format!(
                                        "lazy lang_state create_state failure \
                                         (should be classified, not Bug, in Epic 3): {e}"
                                    ),
                                }));
                                continue;
                            }
                        }
                    }

                    // Early cancellation check: if the caller already dropped
                    // the future (CancelOnDrop fired) or the deadline elapsed
                    // before we even dequeued the request, return Cancelled
                    // without doing any encoder work — including the opt-in
                    // lang_detect pass.
                    if req.cancel.load(std::sync::atomic::Ordering::Relaxed)
                        || Instant::now() >= req.deadline
                    {
                        let _ = req.reply.send(Err(TranscribeError::Cancelled));
                        continue;
                    }

                    // FullParams configuration — embedding hygiene defaults per
                    // 0013 + sharp-edges.md:66 (`print_progress = true` is the
                    // upstream default).
                    // SamplingStrategy::Greedy { best_of: 1 } — memory-conservative
                    // choice for Epic 1's bake. Plan A's whisper-cli used the
                    // default best_of=5; sharp-edges.md:35 notes "beam_size=5
                    // takes ~7× the KV memory of greedy. Memory-bounded? Prefer
                    // greedy with low best_of." Revisit after T13's bake numbers:
                    // on A10 (24GB) memory pressure is unlikely to be the
                    // binding constraint, and best_of=5 may give a quality
                    // bump worth the throughput cost. Tracked in FOLLOWUPS.
                    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
                    params.set_print_progress(false);
                    params.set_print_realtime(false);
                    params.set_print_special(false);
                    params.set_print_timestamps(false);

                    // Language pin (auto-detect when None). For monolingual
                    // checkpoints (e.g., tiny.en) whisper.cpp accepts "auto"
                    // and falls back to "en" internally.
                    let lang = req.config.language.as_deref().unwrap_or("auto");
                    params.set_language(Some(lang));

                    // Cooperative cancellation per 0012 comment-2: the abort
                    // callback polls BOTH `Instant::now() >= deadline` AND
                    // `cancel.load()` — deadline covers per-call timeout,
                    // cancel covers operator-initiated / future-drop.
                    //
                    // 0003 deviation: whisper-rs 0.16.0's
                    // `set_abort_callback_safe` has a type-mismatch bug — at
                    // whisper_params.rs:645 it registers `trampoline::<F>`
                    // while the user_data pointer is actually
                    // `*mut Box<dyn FnMut() -> bool>` (whisper_params.rs:643);
                    // compare to the correct `set_progress_callback_safe` at
                    // whisper_params.rs:597 which uses
                    // `trampoline::<Box<dyn FnMut(i32)>>`. Using the safe
                    // wrapper produces spurious `true` returns from the
                    // callback (encode aborts with -6 even on a 60s deadline).
                    // Fall back to the raw `unsafe set_abort_callback` with a
                    // manual trampoline, and reclaim the Box after `full`
                    // returns to avoid leaking ~16 bytes per request.
                    // `abort_fired` is set INSIDE the callback when the predicate
                    // first returns true. Post-inference we attribute an Err to
                    // Cancelled only when the callback actually fired — not
                    // merely when the deadline happens to have elapsed by the
                    // time state.full returns. (codex review of T7: without
                    // this, a non-cancellation Err that returns just after the
                    // deadline would be misclassified as Cancelled.)
                    let abort_fired = Arc::new(AtomicBool::new(false));
                    let abort_fired_for_cb = Arc::clone(&abort_fired);
                    let cancel_for_abort = Arc::clone(&req.cancel);
                    let deadline_for_abort = req.deadline;
                    let abort_box: Box<Box<dyn FnMut() -> bool>> = Box::new(Box::new(move || {
                        let should_abort = Instant::now() >= deadline_for_abort
                            || cancel_for_abort.load(std::sync::atomic::Ordering::Relaxed);
                        if should_abort {
                            abort_fired_for_cb.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        should_abort
                    }));
                    let abort_user_data = Box::into_raw(abort_box);
                    // SAFETY: `abort_trampoline` reinterprets the user-data pointer as
                    // `*mut Box<dyn FnMut() -> bool>`, which is exactly what
                    // `Box::into_raw(abort_box)` produced just above. The pointer stays
                    // valid until reclaimed via `Box::from_raw` after `state.full`
                    // returns (both exit paths below reclaim exactly once), so
                    // whisper.cpp never invokes the trampoline with a dangling pointer.
                    unsafe {
                        params.set_abort_callback(Some(abort_trampoline));
                        params.set_abort_callback_user_data(
                            abort_user_data.cast::<std::ffi::c_void>(),
                        );
                    }

                    // Compute lang_probs only when opt-in. Pays an extra encoder
                    // pass per sharp-edges.md:13 — lang_detect re-encodes the
                    // audio. Run on lang_state (separate from primary state) so
                    // it doesn't clobber the primary state's logits per
                    // sharp-edges.md:15. Runs BEFORE state.full so the
                    // lang_detect re-encode doesn't see post-inference state.
                    //
                    // Thread count: 4 matches whisper.cpp's default
                    // (api-and-pipeline.md:51 — `n_threads = min(4, hw_concurrency)`).
                    // Hardcoding 1 (as the brief originally pseudocoded) makes
                    // the opt-in path slower than necessary on a CPU build;
                    // whisper-rs's inference uses 4 too, so we match.
                    //
                    // Failure handling is best-effort by design: a pcm_to_mel
                    // or lang_detect failure emits a tracing::warn! and yields
                    // `lang_probs: None` rather than aborting the transcribe.
                    // The primary inference (and its text + language output) is
                    // the contractual value; lang_probs is the speculative
                    // research signal. Epic 3's classification taxonomy may
                    // reclassify (FOLLOWUPS tracks). The opt-in caller can
                    // detect "feature requested but unavailable" via
                    // `compute_lang_probs == true && lang_probs.is_none()`.
                    //
                    // 0003 deviation from brief pseudocode:
                    // - `lang_state.lang_detect()` returns `(i32, Vec<f32>)` not
                    //   just `Vec<f32>`; we destructure and discard the detected
                    //   lang_id (the primary inference gives us language via
                    //   full_lang_id_from_state, which is more reliable).
                    // - The probs Vec is pre-sized to get_lang_max_id()+1 by
                    //   whisper-rs; no `.take(max_id+1)` needed.
                    let lang_probs = if req.config.compute_lang_probs {
                        // The lazy-alloc branch above guarantees Some(_) here
                        // when we reach this point. `expect` documents the
                        // invariant; if this panics, the lazy branch's
                        // continue-on-error didn't fire.
                        // Construction-time invariant: the lazy-alloc branch above sets
                        // lang_state to Some whenever compute_lang_probs is true.
                        #[allow(clippy::expect_used)]
                        let lang_state = lang_state
                            .as_mut()
                            .expect("lazy alloc branch above guarantees Some(_)");
                        match lang_state.pcm_to_mel(&req.samples, 4) {
                            Ok(()) => {
                                match lang_state.lang_detect(0, 4) {
                                    Ok((_lang_id, probs_vec)) => {
                                        // id is a whisper language index (< ~100); the i32
                                        // cast is always in range.
                                        #[allow(
                                        clippy::cast_possible_truncation,
                                        clippy::cast_possible_wrap
                                    )]
                                    let mut paired: Vec<(String, f32)> = probs_vec
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(id, p)| {
                                            // Drop non-finite probabilities (consistent
                                            // with the finiteness checks on segment/token
                                            // signals) and any id without a language code.
                                            let code = whisper_rs::get_lang_str(id as i32)?;
                                            p.is_finite().then(|| (code.to_string(), *p))
                                        })
                                        .collect();
                                        // Sort descending by probability. total_cmp gives a total
                                        // order over f32 (no partial_cmp().unwrap_or(...) fallback);
                                        // non-finite probs were already filtered out above.
                                        paired.sort_by(|a, b| b.1.total_cmp(&a.1));
                                        Some(paired)
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "lang_detect failed: {e}; emitting null lang_probs"
                                        );
                                        None
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("pcm_to_mel failed: {e}; emitting null lang_probs");
                                None
                            }
                        }
                    } else {
                        None
                    };

                    // Re-check cancellation after the opt-in lang_detect pass.
                    // pcm_to_mel + lang_detect can take seconds; if the caller
                    // dropped the future or the deadline elapsed during that
                    // work, surface Cancelled before paying for primary inference.
                    if req.cancel.load(std::sync::atomic::Ordering::Relaxed)
                        || Instant::now() >= req.deadline
                    {
                        // Reclaim the abort closure box even on the early-exit
                        // path; whisper.cpp's abort_callback won't fire here
                        // (state.full not yet called) so this is safe.
                        drop(unsafe { Box::from_raw(abort_user_data) });
                        let _ = req.reply.send(Err(TranscribeError::Cancelled));
                        continue;
                    }

                    let run_result = state.full(params, &req.samples);

                    // Reclaim the closure box now that whisper.cpp no longer
                    // holds the pointer. Safety: we own this allocation
                    // (created via Box::into_raw above); whisper.cpp's
                    // abort_callback only runs synchronously inside
                    // `state.full`, which has returned.
                    let _ = unsafe { Box::from_raw(abort_user_data) };

                    // Attribute the Err. abort_fired captures "did the callback
                    // actually return true during inference?", which avoids the
                    // race where Instant::now() crosses req.deadline after
                    // state.full returned with an unrelated Err.
                    let was_cancelled = abort_fired.load(std::sync::atomic::Ordering::Relaxed);

                    match run_result {
                        Err(_) if was_cancelled => {
                            let _ = req.reply.send(Err(TranscribeError::Cancelled));
                        }
                        Err(e) => {
                            let _ = req.reply.send(Err(TranscribeError::Bug {
                                detail: format!("whisper_full failed: {e}"),
                            }));
                        }
                        Ok(()) => {
                            // Extract text and raw signals in one pass over
                            // segments. 0003 deviation note: whisper-rs 0.16.0
                            // has no `full_get_segment_text`; use `get_segment(i)`
                            // + `WhisperSegment::to_str()` instead.
                            let n_segments = state.full_n_segments();
                            let mut text = String::new();
                            for i in 0..n_segments {
                                if let Some(seg) = state.get_segment(i) {
                                    // to_str_lossy mirrors the token path (substitute
                                    // replacement chars rather than silently dropping a
                                    // non-UTF-8 segment); on a hard WhisperError default
                                    // to "" exactly as the token extraction does.
                                    let seg_text = seg
                                        .to_str_lossy()
                                        .map(std::borrow::Cow::into_owned)
                                        .unwrap_or_default();
                                    text.push_str(&seg_text);
                                }
                            }

                            // Detected language. 0003 deviation: the method
                            // is `full_lang_id_from_state` (not `full_lang_id`)
                            // and the helper is the standalone
                            // `whisper_rs::get_lang_str`.
                            let lang_id = state.full_lang_id_from_state();
                            let language = whisper_rs::get_lang_str(lang_id)
                                .unwrap_or("unknown")
                                .to_string();

                            // T9: extract raw signals. Non-finite values in
                            // the whisper-rs output surface as Bug per codex's
                            // T4 review forward-pointer.
                            let segments = match extract_segments(&state) {
                                Ok(segs) => segs,
                                Err(detail) => {
                                    let _ = req.reply.send(Err(TranscribeError::Bug { detail }));
                                    continue;
                                }
                            };

                            let _ = req.reply.send(Ok(TranscribeOutput {
                                text,
                                language,
                                lang_probs, // Some(paired) when opt-in, None otherwise
                                segments,
                                model_id: model_id.clone(),
                            }));
                        }
                    }
                }
                // Sender dropped → channel closed → orderly exit. Per 0016
                // comment-2, this is the shutdown-carve-out path (not Bug).
            })
            .map_err(|e| WhisperInitError::WorkerSpawn {
                detail: format!("spawn whisper worker thread: {e}"),
            })?;

        // Block this sync fn on the init result. WhisperEngine::new is sync,
        // so blocking the calling thread on init_rx.recv() is fine.
        match init_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // Best-effort reap: the worker already reported this init error over
                // the channel; its JoinHandle result is not separately actionable.
                let _ = handle.join();
                return Err(e);
            }
            Err(_) => {
                // Worker died before sending an init result; reap it, surface below.
                let _ = handle.join();
                return Err(WhisperInitError::ModelLoad {
                    path: config.model_path.display().to_string(),
                    detail: "worker thread died before sending init result".to_string(),
                });
            }
        }

        Ok(Self {
            request_tx: Some(request_tx),
            handle: Some(handle),
            lang_state_allocations,
        })
    }

    /// Test-only accessor: returns the number of times the worker thread has
    /// lazily allocated `lang_state`. Used by `tests/transcribe_lang_state.rs`
    /// to assert the lazy lifecycle (0 for non-opt-in workers; exactly 1 for
    /// opt-in workers regardless of request count). 0016: the counter is
    /// the only piece of the lazy lifecycle exposed outside the worker thread.
    /// Gated per 0005 — matches the `Store::get_video_for_test` / `EventRow`
    /// pattern in `src/state/mod.rs`.
    #[cfg(feature = "test-helpers")]
    pub fn lang_state_allocations(&self) -> usize {
        self.lang_state_allocations
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn transcribe(
        &self,
        samples: Vec<f32>,
        config: PerCallConfig,
        timeout: Duration,
    ) -> Result<TranscribeOutput, TranscribeError> {
        let tx = self
            .request_tx
            .as_ref()
            .ok_or_else(|| TranscribeError::Bug {
                detail: "engine already shut down (request_tx taken)".to_string(),
            })?;
        transcribe_via_tx(tx, samples, config, timeout).await
    }

    /// Construct a clone-able `Arc<dyn Transcriber>` backed by this engine's
    /// request channel. Workers in the pipelined orchestrator hold one of
    /// these; the engine itself stays owned by `main` so `engine.shutdown()`
    /// can run last per 0025.
    ///
    /// Cloning the returned `Arc` is cheap; each clone holds an additional
    /// reference to the engine's `mpsc::Sender<TranscribeRequest>`. The
    /// engine's worker thread exits its `blocking_recv` loop only when the
    /// last `Sender` clone is dropped — which means `engine.shutdown()`
    /// MUST run AFTER all handle clones are dropped, or the worker
    /// thread parks until process exit (0025 shutdown ORDER step 4 is
    /// load-bearing).
    ///
    /// T18: consumed by `main.rs`'s Process arm (pipelined orchestrator
    /// wiring) and by integration tests. No `dead_code` suppression
    /// needed.
    pub fn transcriber_handle(&self) -> std::sync::Arc<dyn Transcriber> {
        // Internal API invariant: request_tx is Some until shutdown() takes it;
        // calling this accessor afterward is a programmer error, not a runtime or
        // external-input condition.
        #[allow(clippy::expect_used)]
        let request_tx = self
            .request_tx
            .as_ref()
            .expect("transcriber_handle called after engine shutdown")
            .clone();
        std::sync::Arc::new(WhisperEngineHandle { request_tx })
    }

    /// Drop the sender (closing the channel and letting the worker exit), then
    /// join the worker thread. Idempotent with `Drop::drop`.
    ///
    /// 0025 shutdown ORDER step 4 (LAST). Callers in `main.rs` must ensure
    /// every `WhisperEngineHandle` clone has been dropped before calling
    /// this; otherwise the engine's request channel stays open and the
    /// worker thread parks in `blocking_recv` until process exit.
    pub fn shutdown(mut self) {
        self.teardown();
    }

    fn teardown(&mut self) {
        // Order matters: closing the channel must happen BEFORE the join, or
        // the worker stays parked in blocking_recv and the join hangs forever.
        drop(self.request_tx.take());
        if let Some(handle) = self.handle.take() {
            // Discard the join result: a worker panic surfaces in its own logs, and
            // teardown must stay infallible (it also runs from Drop).
            let _ = handle.join();
        }
    }
}

impl Drop for WhisperEngine {
    fn drop(&mut self) {
        self.teardown();
    }
}

// T5's `engine_tests` module is removed in T6.
//
// Both T5 tests (`shell_returns_bug_error_on_transcribe`, `shutdown_joins_cleanly`)
// used a `dummy_config()` pointing model_path at `/dev/null` and relied on
// `WhisperEngine::new` NOT actually loading the model. T6's `new` does load
// the model, so `/dev/null` now correctly fails before construction returns,
// making the T5 assertions unreachable. The replacements live in
// `tests/whisper_engine_init.rs` (test-helpers gated, uses ggml-tiny.en.bin):
//   - engine_loads_tiny_en_model_successfully → exercises load → real
//     transcribe (T7 returns Ok with text+language; 5s shutdown wallclock
//     guard catches Drop-ordering regressions).
//   - engine_rejects_missing_model_path → exercises the WhisperInitError
//     path that T5's `/dev/null`-construct-then-Bug-on-transcribe could not.
//   - transcribe_silence_returns_empty_or_short_text → exercises the
//     fixture-decoded silence path end-to-end.
//   - transcribe_respects_short_deadline → exercises abort_callback firing
//     on deadline elapse.
// See 0003 deviation disclosure in the commit body.

// ============================================================================
// Plan B Epic 1 (T11): Transcriber trait
// ============================================================================
//
// Object-safe trait that `pipeline::process_one` consumes via `&dyn Transcriber`.
// Production wires `WhisperEngine`; tests wire a `FakeTranscriber` over the
// scripted `TranscribeOutput`. The `name()` method records provenance into
// `TranscriptMetadata::transcript_source` (replaces Plan A's hardcoded
// "whisper.cpp"; partial resolution of FOLLOWUPS T14).

#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(
        &self,
        samples: Vec<f32>,
        config: PerCallConfig,
        timeout: Duration,
    ) -> Result<TranscribeOutput, TranscribeError>;

    fn name(&self) -> &'static str;
}

#[async_trait]
impl Transcriber for WhisperEngine {
    async fn transcribe(
        &self,
        samples: Vec<f32>,
        config: PerCallConfig,
        timeout: Duration,
    ) -> Result<TranscribeOutput, TranscribeError> {
        WhisperEngine::transcribe(self, samples, config, timeout).await
    }

    fn name(&self) -> &'static str {
        "whisper-rs"
    }
}

/// Shared engine-call body. Both `WhisperEngine::transcribe` and
/// [`WhisperEngineHandle::transcribe`] delegate here so the
/// `CancelOnDrop` guard + oneshot dance lives in exactly one place.
///
/// T18 factor-out: the original body lived inline on
/// `WhisperEngine::transcribe`. Lifting it lets the new clone-able
/// handle (needed by 0025's shutdown ORDER) share the same code path
/// without duplicating the cancel/deadline plumbing.
async fn transcribe_via_tx(
    tx: &mpsc::Sender<TranscribeRequest>,
    samples: Vec<f32>,
    config: PerCallConfig,
    timeout: Duration,
) -> Result<TranscribeOutput, TranscribeError> {
    let cancel = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + timeout;
    let (reply_tx, reply_rx) = oneshot::channel();

    // CancelOnDrop fires `cancel = true` if this future is dropped before
    // the worker replies (caller-initiated future cancellation). The worker
    // owns its own Arc clone via the request and polls it in T7's
    // abort_callback. Post-reply firing is a no-op (worker has already moved on).
    let _cancel_guard = CancelOnDrop(Arc::clone(&cancel));

    let req = TranscribeRequest {
        samples,
        config,
        cancel,
        deadline,
        reply: reply_tx,
    };

    // No tokio::spawn timer here: T7's abort_callback polls deadline + cancel
    // directly inside whisper.cpp's encoder/decoder loop. 0012 comment-2.

    tx.send(req).await.map_err(|_| TranscribeError::Bug {
        detail: "worker thread channel closed (engine shut down mid-flight)".to_string(),
    })?;

    reply_rx.await.unwrap_or_else(|_| {
        Err(TranscribeError::Bug {
            detail: "worker dropped reply oneshot (worker panicked or restarted)".to_string(),
        })
    })
}

/// Clone-able transcriber handle backed by the engine's request channel
/// (T18, 0025).
///
/// `WhisperEngine::shutdown(self)` consumes `self`, which means handing
/// the engine to spawned worker tasks as `Arc<dyn Transcriber>` would
/// prevent the orchestrator's main from ever shutting it down. This
/// handle wraps the engine's existing `mpsc::Sender<TranscribeRequest>`
/// instead — workers hold cheap clones; `WhisperEngine` itself stays
/// owned in `main`.
///
/// **Shutdown coupling (load-bearing, 0025 step 4).** The engine's
/// worker thread exits `blocking_recv` only when the LAST clone of the
/// request sender is dropped. `engine.shutdown()` MUST run after every
/// `WhisperEngineHandle` clone has been dropped — typically by dropping
/// main's own clone of the `Arc<dyn Transcriber>` after
/// `run_pipelined` has joined every worker.
///
/// T18: constructed by `WhisperEngine::transcriber_handle` (called by
/// `main.rs`'s Process arm); its `request_tx` field is read on every
/// trait `transcribe` call. No `dead_code` suppression needed.
pub struct WhisperEngineHandle {
    request_tx: mpsc::Sender<TranscribeRequest>,
}

#[async_trait]
impl Transcriber for WhisperEngineHandle {
    async fn transcribe(
        &self,
        samples: Vec<f32>,
        config: PerCallConfig,
        timeout: Duration,
    ) -> Result<TranscribeOutput, TranscribeError> {
        transcribe_via_tx(&self.request_tx, samples, config, timeout).await
    }

    fn name(&self) -> &'static str {
        // Same provenance as the owned engine — the wire format's
        // `transcript_source` field doesn't distinguish "engine vs
        // handle-to-engine"; both routes hit the same worker thread.
        "whisper-rs"
    }
}

// ============================================================================
// Plan B Epic 5b (T08): 0013 backend assertion — parser + decision tests
// ============================================================================
//
// The fixtures below are the real whisper.cpp v1.8.3 init log shapes (the
// pinned commit 2eeeba56, via whisper-rs-sys 0.15.0). Sources:
//   - `whisper_init_with_params_no_state` banner: src/whisper.cpp:3713-3718
//   - `whisper_backend_init_gpu` device enumeration / selection:
//     src/whisper.cpp:1290-1325 ("device %zu: %s (type: %d)",
//     "found GPU device %zu: %s (type: %d, cnt: %d)", "no GPU found",
//     "using %s backend")
//   - `whisper_backend_init` ACCEL loop, which reuses the SAME
//     "using %s backend" wording under a DIFFERENT function prefix
//     (src/whisper.cpp:1342) — this is why the parser anchors on the full
//     `whisper_backend_init_gpu:` prefix rather than on "using ... backend".
//   - `ggml_cuda_init` device banner: ggml/src/ggml-cuda/ggml-cuda.cu:206,267.
//     Reaches our callback because `whisper_backend_init_gpu` forwards
//     whisper's log callback to ggml via `ggml_log_set` on entry
//     (src/whisper.cpp:1291).

#[cfg(test)]
mod backend_assertion_tests {
    use super::*;

    /// CUDA build on the A10 workspace: ggml enumerates one CUDA device and
    /// whisper selects it. Line shapes from the v1.8.3 sources cited above;
    /// the `using CUDA0 backend` line is the one `docs/operations/src-vm.md`
    /// already tells the operator to look for at startup.
    const CUDA_INIT_LOG: &str = "\
whisper_init_with_params_no_state: use gpu    = 1
whisper_init_with_params_no_state: flash attn = 1
whisper_init_with_params_no_state: gpu_device = 0
whisper_init_with_params_no_state: dtw        = 0
whisper_model_load: loading model
whisper_model_load: model size    = 1533.14 MB
ggml_cuda_init: GGML_CUDA_FORCE_MMQ:    no
ggml_cuda_init: GGML_CUDA_FORCE_CUBLAS: no
ggml_cuda_init: found 1 CUDA devices:
  Device 0: NVIDIA A10, compute capability 8.6, VMM: yes
whisper_backend_init_gpu: device 0: CUDA0 (type: 1)
whisper_backend_init_gpu: found GPU device 0: CUDA0 (type: 1, cnt: 0)
whisper_backend_init_gpu: using CUDA0 backend
whisper_init_state: kv self size  =    3.15 MB
";

    /// CPU-only build (or a CUDA build whose GPU init failed): whisper walks
    /// the device list, finds nothing of GPU type, and falls back silently.
    /// This is precisely the run 0013 exists to catch.
    ///
    /// Verbatim (elided) capture from this bridge on a tiny.en init, which is
    /// also what pinned down the capture-window bug: the
    /// `whisper_backend_init_gpu` lines land during `create_state()`, AFTER
    /// `whisper_model_load` — not during the `..._no_state` constructor.
    const CPU_INIT_LOG: &str = "\
whisper_init_with_params_no_state: use gpu    = 1
whisper_init_with_params_no_state: flash attn = 0
whisper_init_with_params_no_state: gpu_device = 0
whisper_init_with_params_no_state: dtw        = 0
whisper_init_with_params_no_state: devices    = 1
whisper_init_with_params_no_state: backends   = 1
whisper_model_load: loading model
whisper_model_load: model size    =   77.11 MB
whisper_backend_init_gpu: device 0: CPU (type: 0)
whisper_backend_init_gpu: no GPU found
whisper_init_state: kv self size  =    3.15 MB
";

    #[test]
    fn detect_backend_reads_cuda_init_log_as_gpu() {
        assert_eq!(
            detect_backend(CUDA_INIT_LOG),
            DetectedBackend::Gpu {
                device: "CUDA0".to_string()
            }
        );
    }

    #[test]
    fn detect_backend_reads_cpu_only_init_log_as_cpu() {
        assert_eq!(detect_backend(CPU_INIT_LOG), DetectedBackend::Cpu);
    }

    #[test]
    fn detect_backend_reads_garbage_as_unknown() {
        assert_eq!(detect_backend(""), DetectedBackend::Unknown);
        assert_eq!(
            detect_backend("some unrelated log\nlines that are not whisper init\n"),
            DetectedBackend::Unknown
        );
    }

    /// `whisper_backend_init` (no `_gpu` suffix) logs "using BLAS backend" for
    /// ACCEL devices. Reading that as a GPU would defeat the whole assertion.
    #[test]
    fn detect_backend_ignores_the_accel_backend_line() {
        let log = "\
whisper_backend_init_gpu: no GPU found
whisper_backend_init: using BLAS backend
";
        assert_eq!(detect_backend(log), DetectedBackend::Cpu);
    }

    /// The false-Gpu-positive gap. whisper.cpp logs `using X backend` BEFORE
    /// calling `ggml_backend_dev_init` (whisper.cpp:1320-1321); when that call
    /// fails it logs `failed to initialize X backend` and returns nullptr, and
    /// the caller falls silently through to ACCEL/CPU (whisper.cpp:1332-1358).
    /// Treating the `using` line as proof of GPU would let a CUDA build sail
    /// past the assertion while running on CPU — exactly 0013's target class.
    const CUDA_INIT_FAILED_LOG: &str = "\
whisper_init_with_params_no_state: use gpu    = 1
whisper_init_with_params_no_state: flash attn = 1
whisper_model_load: loading model
ggml_cuda_init: found 1 CUDA devices:
  Device 0: NVIDIA A10, compute capability 8.6, VMM: yes
whisper_backend_init_gpu: device 0: CUDA0 (type: 1)
whisper_backend_init_gpu: found GPU device 0: CUDA0 (type: 1, cnt: 0)
whisper_backend_init_gpu: using CUDA0 backend
whisper_backend_init_gpu: failed to initialize CUDA0 backend
whisper_init_state: kv self size  =    3.15 MB
";

    #[test]
    fn detect_backend_rejects_a_gpu_whose_backend_init_failed() {
        assert_eq!(
            detect_backend(CUDA_INIT_FAILED_LOG),
            DetectedBackend::GpuInitFailed {
                device: "CUDA0".to_string()
            }
        );
        let err = check_backend(ExpectedBackend::Gpu, &detect_backend(CUDA_INIT_FAILED_LOG))
            .expect_err("a failed GPU backend init is a silent CPU fallback — must hard-fail");
        match err {
            WhisperInitError::BackendMismatch { detected, .. } => {
                assert!(
                    detected.contains("CUDA0") && detected.contains("failed to initialize"),
                    "the error must name the failure the operator has to act on: {detected}"
                );
            }
            other => panic!("expected BackendMismatch, got {other:?}"),
        }
    }

    /// Ordered parse, not "any failure poisons the log": a failed attempt
    /// followed by a LATER successful GPU selection is a working GPU.
    #[test]
    fn detect_backend_accepts_a_gpu_selected_after_an_earlier_failure() {
        let log = "\
whisper_backend_init_gpu: using CUDA0 backend
whisper_backend_init_gpu: failed to initialize CUDA0 backend
whisper_backend_init_gpu: using CUDA1 backend
whisper_init_state: kv self size  =    3.15 MB
";
        assert_eq!(
            detect_backend(log),
            DetectedBackend::Gpu {
                device: "CUDA1".to_string()
            }
        );
        check_backend(ExpectedBackend::Gpu, &detect_backend(log))
            .expect("a GPU that did initialize satisfies a CUDA build");
    }

    /// The ACCEL loop reuses the failure wording too (whisper.cpp:1346) under
    /// the `whisper_backend_init:` prefix. A BLAS failure says nothing about
    /// the GPU verdict — here, CPU.
    #[test]
    fn detect_backend_ignores_an_accel_backend_init_failure() {
        let log = "\
whisper_backend_init_gpu: no GPU found
whisper_backend_init: using BLAS backend
whisper_backend_init: failed to initialize BLAS backend
";
        assert_eq!(detect_backend(log), DetectedBackend::Cpu);
    }

    /// Fail closed on a log that stops before whisper.cpp reaches a verdict:
    /// device enumeration alone proves nothing, and `Unknown` is rejected by
    /// the CUDA assertion.
    #[test]
    fn detect_backend_reads_a_truncated_init_log_as_unknown() {
        let log = "\
whisper_init_with_params_no_state: use gpu    = 1
ggml_cuda_init: found 1 CUDA devices:
  Device 0: NVIDIA A10, compute capability 8.6, VMM: yes
whisper_backend_init_gpu: device 0: CUDA0 (type: 1)
whisper_backend_init_gpu: found GPU device 0: CUDA0 (type: 1, cnt: 0)
";
        assert_eq!(detect_backend(log), DetectedBackend::Unknown);
        check_backend(ExpectedBackend::Gpu, &detect_backend(log))
            .expect_err("an unresolved init log is not proof of GPU use");
    }

    #[test]
    fn detect_device_description_extracts_the_cuda_device_name() {
        assert_eq!(
            detect_device_description(CUDA_INIT_LOG).as_deref(),
            Some("NVIDIA A10, compute capability 8.6, VMM: yes")
        );
        assert_eq!(detect_device_description(CPU_INIT_LOG), None);
    }

    #[test]
    fn expecting_gpu_and_detecting_cpu_is_a_backend_mismatch() {
        let err = check_backend(ExpectedBackend::Gpu, &DetectedBackend::Cpu)
            .expect_err("CPU fallback under a CUDA build must hard-fail");
        match err {
            WhisperInitError::BackendMismatch { expected, detected } => {
                // The variant carries BOTH sides so the operator does not have
                // to go re-read the log to learn what actually happened.
                assert!(expected.contains("GPU"), "expected side: {expected}");
                assert!(detected.contains("CPU"), "detected side: {detected}");
            }
            other => panic!("expected BackendMismatch, got {other:?}"),
        }
    }

    #[test]
    fn expecting_gpu_and_detecting_gpu_passes_and_names_the_device() {
        let detected = detect_backend(CUDA_INIT_LOG);
        check_backend(ExpectedBackend::Gpu, &detected).expect("CUDA0 satisfies a CUDA build");
    }

    /// Fail closed: an unparseable init log is not proof of GPU use, and 0013
    /// rejects softening the contract. A CUDA build that cannot prove GPU must
    /// abort at construction rather than run 100x slow.
    #[test]
    fn expecting_gpu_and_detecting_unknown_fails_closed() {
        let err = check_backend(ExpectedBackend::Gpu, &DetectedBackend::Unknown)
            .expect_err("unproven backend must not pass a CUDA build");
        assert!(matches!(err, WhisperInitError::BackendMismatch { .. }));
    }

    /// Non-CUDA builds only log; CPU is the expected backend for local dev
    /// (cross-epic FOLLOWUPS: "the assertion must NOT fire on non-CUDA builds").
    #[test]
    fn an_unconstrained_expectation_accepts_every_detected_backend() {
        for detected in [
            DetectedBackend::Cpu,
            DetectedBackend::Unknown,
            DetectedBackend::Gpu {
                device: "CUDA0".to_string(),
            },
            DetectedBackend::GpuInitFailed {
                device: "CUDA0".to_string(),
            },
        ] {
            check_backend(ExpectedBackend::Unconstrained, &detected)
                .expect("non-CUDA builds never hard-fail on backend");
        }
    }

    /// The cfg gate itself: `cuda` builds expect a GPU, everything else is
    /// unconstrained. Compiled under both feature sets.
    #[test]
    fn expected_backend_tracks_the_cuda_feature() {
        if cfg!(feature = "cuda") {
            assert_eq!(EXPECTED_BACKEND, ExpectedBackend::Gpu);
        } else {
            assert_eq!(EXPECTED_BACKEND, ExpectedBackend::Unconstrained);
        }
    }

    /// The bridge install is idempotent and safe to call from many threads —
    /// it is the precondition for "installed once, before any context init".
    #[test]
    fn installing_the_log_bridge_is_idempotent_across_threads() {
        let handles: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(install_log_bridge))
            .collect();
        for h in handles {
            h.join().expect("install thread should not panic");
        }
        install_log_bridge();
    }

    /// Capture is phase-scoped: lines logged inside the guard's lifetime are
    /// captured, the slot is cleared on drop, and a second capture phase does
    /// not see the first phase's lines.
    #[test]
    fn init_capture_is_phase_scoped_and_clears_on_drop() {
        {
            let capture = InitCapture::begin();
            record_bridge_line("whisper_backend_init_gpu: using CUDA0 backend\n");
            let log = capture.snapshot();
            assert!(log.contains("CUDA0"), "captured log: {log:?}");
        }
        // Outside the guard, lines go nowhere (tracing only).
        record_bridge_line("whisper_backend_init_gpu: no GPU found\n");
        let capture = InitCapture::begin();
        let log = capture.snapshot();
        assert!(
            log.is_empty(),
            "a fresh capture phase must not inherit earlier lines: {log:?}"
        );
    }
}

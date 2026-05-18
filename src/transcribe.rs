use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::errors::TranscribeError;

// ============================================================================
// Plan B Epic 1: TranscribeOutput types
// ============================================================================
//
// Pass-through raw signals from whisper.cpp's C API via the whisper-rs binding.
// See AD0010 (raw_signals schema), AD0016 (worker-thread invariants).
//
// These types are OWNED data: no references, no whisper-rs handles. They cross
// the worker-thread boundary safely (AD0016 #1: owned data only).

use serde::{Deserialize, Serialize};

/// Owned output from a single whisper inference. Crosses the worker-thread
/// boundary (AD0016). T10's artifact writer maps these fields across the
/// artifact JSON: `text` and `model_id` land at the top level (alongside
/// Plan A's existing metadata), while `language`, `lang_probs`, and `segments`
/// are placed inside the `raw_signals` sub-object (AD0010). This struct is
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
/// special tokens (`[BEG]`, `[END]`, `<|en|>`, etc.) per AD0010's pass-through
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
/// AD0003 deviation: whisper-rs 0.16.0 does not expose flat
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
/// retained per AD0010's pass-through rule — downstream consumers filter them.
fn extract_segments(state: &whisper_rs::WhisperState) -> Result<Vec<SegmentRaw>, String> {
    let n_segments = state.full_n_segments();
    if n_segments < 0 {
        return Err(format!(
            "whisper-rs returned negative n_segments: {n_segments}"
        ));
    }
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
                .map(|s| s.into_owned())
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
// Worker-thread architecture per AD0016:
// - Only owned data crosses the boundary (samples, configs, output structs)
// - WhisperContext/WhisperState stay inside the worker thread (T6/T7)
// - Closed oneshot reply is Bug-class during normal execution; AD0016 comment-2
//   carves out shutdown (relevant when Epic 2 wires shutdown signaling).
//
// Per-request cancellation per AD0012 (+ comment-2 refinement):
// - Each request carries its own Arc<AtomicBool> for operator-initiated cancel
//   (per-request, never shared across requests — AD0012's no-leak invariant).
// - Each request carries its own `deadline: Instant` for per-call timeout.
// - T7's abort_callback polls BOTH inside whisper.cpp's encoder/decoder loop;
//   no separate timer task is spawned (deviates from the T5 brief's tokio::spawn
//   sketch per AD0012 comment-2; see AD0003 deviation disclosure in commit body).

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
    /// Per-request cancel flag (AD0012). Operator-initiated cancellation flips
    /// this; T7's abort_callback polls it. Never shared across requests.
    pub cancel: Arc<AtomicBool>,
    /// Per-call deadline (AD0012 comment-2). T7's abort_callback polls
    /// `Instant::now() >= deadline` directly — no separate timer task.
    pub deadline: Instant,
    pub reply: oneshot::Sender<Result<TranscribeOutput, TranscribeError>>,
}

// AD0002: BackendMismatch is constructed by T13's backend-assertion path;
// suppress dead_code until then.
#[derive(Debug, thiserror::Error)]
pub enum WhisperInitError {
    #[error("loading whisper model from {path}: {detail}")]
    ModelLoad { path: String, detail: String },

    #[allow(dead_code)]
    #[error(
        "backend mismatch: expected GPU but whisper.cpp engaged CPU fallback (sharp-edges.md:61)"
    )]
    BackendMismatch,

    #[error("creating whisper state: {detail}")]
    StateCreate { detail: String },

    #[error("spawning whisper worker thread: {detail}")]
    WorkerSpawn { detail: String },
}

/// FFI trampoline for whisper.cpp's abort_callback. `user_data` must be the
/// raw pointer returned by `Box::into_raw(Box::new(closure))` where `closure`
/// is `Box<dyn FnMut() -> bool>`. See the AD0003 deviation comment inside the
/// worker loop for why we hand-roll this instead of using
/// `FullParams::set_abort_callback_safe`.
unsafe extern "C" fn abort_trampoline(user_data: *mut std::ffi::c_void) -> bool {
    let cb = unsafe { &mut *(user_data as *mut Box<dyn FnMut() -> bool>) };
    cb()
}

/// Drop guard that flips the per-request cancel flag when the caller's
/// `transcribe()` future is dropped before the worker replies. Without this,
/// a caller cancelling the future would leave the worker chewing on an
/// orphaned request whose result no one will read. Per AD0012 comment-2,
/// the cancel flag is the operator-initiated cancellation channel; future-drop
/// is a special case of operator-initiated.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Worker-thread-owning engine handle. See AD0016 for the parallelism contract
/// (engine API stays stable across single- and multi-state worker pools).
///
/// Both fields are `Option` so `shutdown()` and `Drop::drop` can share the same
/// teardown sequence: drop the sender FIRST (closes the channel, lets the
/// worker's `blocking_recv` return `None`), THEN join. If the sender were
/// dropped after the join attempt, the worker would park forever in
/// `blocking_recv` and the join would hang. (Brief code had this hazard;
/// AD0003 deviation — see commit body.)
pub struct WhisperEngine {
    request_tx: Option<mpsc::Sender<TranscribeRequest>>,
    handle: Option<thread::JoinHandle<()>>,
    /// Counter incremented each time the worker thread lazily allocates
    /// `lang_state` (at most once per worker lifetime). Always present so the
    /// worker capture doesn't branch on a feature flag; only **read** outside
    /// the worker via the `test-helpers` getter below. AD0002: `dead_code`
    /// is allowed because the field is constructed and written-to in the
    /// worker (via a cloned `Arc`) but only read in test-helpers builds.
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
        // AD0016 invariant preserved: WhisperState stays inside the worker.
        let lang_state_allocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lang_state_allocations_worker = std::sync::Arc::clone(&lang_state_allocations);

        let handle = thread::Builder::new()
            .name("uu-tiktok-whisper-worker".to_string())
            .spawn(move || {
                // whisper-rs 0.16.0: setters take &mut self and return &mut Self.
                // use_gpu(true) is harmless on a CPU build — whisper.cpp falls back.
                // AD0013 backend-mismatch assertion lands in T13.
                let mut ctx_params = WhisperContextParameters::default();
                ctx_params
                    .use_gpu(true)
                    .flash_attn(flash_attn)
                    .gpu_device(gpu_device);

                // whisper-rs 0.16.0 accepts P: AsRef<Path>; pass the PathBuf directly.
                // AD0003 deviation from brief sketch (brief did .to_str().unwrap_or("")).
                let ctx_result = WhisperContext::new_with_params(&model_path, ctx_params);
                let ctx = match ctx_result {
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

                // Allocate WhisperState ONCE in the init phase and reuse it for
                // every request. Per whisper.cpp's concurrency model
                // (see whisper-cpp deepdive concurrency.md + sharp-edges.md:21):
                // WhisperState owns ~500MB-1GB of KV caches and compute
                // buffers; allocating one per request would defeat Plan B's
                // efficiency goal. `whisper_full_with_state` clears `result_all`
                // on entry (sharp-edges.md:19), so state reuse across calls is
                // safe. Epic 1 ships single-state; Plan C may allocate N states
                // per context for intra-GPU parallelism (AD0016 architecture).
                //
                // `ctx` and `state` live until this closure exits — keep the
                // model in memory for the worker's lifetime. AD0016:
                // WhisperContext and WhisperState stay inside the worker
                // thread; they never escape.
                let mut state = match ctx.create_state() {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = init_tx.send(Err(WhisperInitError::StateCreate {
                            detail: format!("primary state: {e}"),
                        }));
                        return;
                    }
                };

                // T2 perf-tweaks: secondary state used only for opt-in
                // lang_detect is now **lazily allocated** on the first
                // `compute_lang_probs=true` request. Non-opt-in workers pay
                // zero VRAM/host overhead for lang_state; opt-in workers pay
                // exactly once (on first use), with subsequent opt-in
                // requests reusing the same state. See sharp-edges.md:15 —
                // whisper_lang_auto_detect_with_state clobbers state (reuses
                // decoders[0] and logits), so it must NOT run on the primary
                // state used for inference. AD0016: this WhisperState stays
                // inside the worker thread; the only thing crossing out is
                // the `lang_state_allocations` counter Arc.
                let mut lang_state: Option<whisper_rs::WhisperState> = None;
                let lang_state_allocations = lang_state_allocations_worker;

                // Init success: model loaded and primary state allocated.
                if init_tx.send(Ok(())).is_err() {
                    return; // caller went away
                }

                // model_id is derived from the path file_name once, outside the
                // hot loop. AD0010: this lands in the artifact's top-level
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
                    // model). Defer until first opt-in request. AD0016: state
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
                    // AD0013 + sharp-edges.md:66 (`print_progress = true` is the
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

                    // Cooperative cancellation per AD0012 comment-2: the abort
                    // callback polls BOTH `Instant::now() >= deadline` AND
                    // `cancel.load()` — deadline covers per-call timeout,
                    // cancel covers operator-initiated / future-drop.
                    //
                    // AD0003 deviation: whisper-rs 0.16.0's
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
                    unsafe {
                        params.set_abort_callback(Some(abort_trampoline));
                        params
                            .set_abort_callback_user_data(abort_user_data as *mut std::ffi::c_void);
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
                    // AD0003 deviation from brief pseudocode:
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
                        let lang_state = lang_state
                            .as_mut()
                            .expect("lazy alloc branch above guarantees Some(_)");
                        match lang_state.pcm_to_mel(&req.samples, 4) {
                            Ok(()) => match lang_state.lang_detect(0, 4) {
                                Ok((_lang_id, probs_vec)) => {
                                    let mut paired = Vec::with_capacity(probs_vec.len());
                                    for (id, p) in probs_vec.iter().enumerate() {
                                        if let Some(code) = whisper_rs::get_lang_str(id as i32) {
                                            paired.push((code.to_string(), *p));
                                        }
                                    }
                                    // Sort descending by probability for
                                    // operator-readable JSON output.
                                    paired.sort_by(|a, b| {
                                        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                                    });
                                    Some(paired)
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "lang_detect failed: {e}; emitting null lang_probs"
                                    );
                                    None
                                }
                            },
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
                        let _ = unsafe { Box::from_raw(abort_user_data) };
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
                            // segments. AD0003 deviation note: whisper-rs 0.16.0
                            // has no `full_get_segment_text`; use `get_segment(i)`
                            // + `WhisperSegment::to_str()` instead.
                            let n_segments = state.full_n_segments();
                            let mut text = String::new();
                            for i in 0..n_segments {
                                if let Some(seg) = state.get_segment(i) {
                                    if let Ok(s) = seg.to_str() {
                                        text.push_str(s);
                                    }
                                }
                            }

                            // Detected language. AD0003 deviation: the method
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
                // Sender dropped → channel closed → orderly exit. Per AD0016
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
                let _ = handle.join();
                return Err(e);
            }
            Err(_) => {
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
    /// opt-in workers regardless of request count). AD0016: the counter is
    /// the only piece of the lazy lifecycle exposed outside the worker thread.
    ///
    /// AD0002/AD0005: `dead_code` is allowed because workspace-level
    /// `--features test-helpers` activates this getter on the bin compilation
    /// path too, but only integration tests call it — matches the
    /// `Store::get_video_for_test` / `EventRow` pattern in `src/state/mod.rs`.
    #[cfg(feature = "test-helpers")]
    #[allow(dead_code)]
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
        // directly inside whisper.cpp's encoder/decoder loop. AD0012 comment-2.

        let tx = self
            .request_tx
            .as_ref()
            .ok_or_else(|| TranscribeError::Bug {
                detail: "engine already shut down (request_tx taken)".to_string(),
            })?;

        tx.send(req).await.map_err(|_| TranscribeError::Bug {
            detail: "worker thread channel closed (engine shut down mid-flight)".to_string(),
        })?;

        reply_rx.await.unwrap_or_else(|_| {
            Err(TranscribeError::Bug {
                detail: "worker dropped reply oneshot (worker panicked or restarted)".to_string(),
            })
        })
    }

    /// Drop the sender (closing the channel and letting the worker exit), then
    /// join the worker thread. Idempotent with `Drop::drop`.
    pub fn shutdown(mut self) {
        self.teardown();
    }

    fn teardown(&mut self) {
        // Order matters: closing the channel must happen BEFORE the join, or
        // the worker stays parked in blocking_recv and the join hangs forever.
        drop(self.request_tx.take());
        if let Some(handle) = self.handle.take() {
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
// See AD0003 deviation disclosure in the commit body.

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

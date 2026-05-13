# Task 2 — Lazy-allocate `lang_state` on first opt-in request

**Goal:** Stop unconditionally allocating a second `WhisperState` at worker startup. Allocate the lang-detect state only when the first request with `compute_lang_probs=true` arrives; reuse it across subsequent opt-in requests. Validate via an `Arc<AtomicUsize>` counter wired into `WhisperEngine`'s test-helpers surface.

**Spec commit:** 1 — `refactor(transcribe): lazy-allocate lang_state on first opt-in request`.

**ADRs directly relevant:**
- **AD0001** — branch placement (this commit lands on `feat/perf-tweaks`).
- **AD0005** — new test file gets `[[test]] required-features = ["test-helpers"]`.
- **AD0016** — `WhisperState` stays inside the worker thread; the Arc counter is the only thing that crosses the boundary (and it's read-only from outside).

Background available if needed: AD0009 (whisper-rs binding), AD0012 (cooperative cancellation).

**Files:**
- Modify: `src/transcribe.rs:264-268` (`EngineConfig`: no field change — counter lives on `WhisperEngine`)
- Modify: `src/transcribe.rs:345-348` (`WhisperEngine` struct: add `lang_state_allocations: Arc<AtomicUsize>` field, unconditional)
- Modify: `src/transcribe.rs:360–453` (`WhisperEngine::new`: clone counter into worker; replace eager `lang_state` alloc with `Option<WhisperState>`)
- Modify: `src/transcribe.rs:540–605` (request loop: lazy-alloc branch before the existing `compute_lang_probs` branch; rebind `lang_state` from `Option<WhisperState>` to `&mut WhisperState` inside the branch)
- Add: `tests/transcribe_lang_state.rs` (new file — uses Arc counter pattern)
- Modify: `Cargo.toml` (`[[test]] name = "transcribe_lang_state"` with `required-features = ["test-helpers"]`)
- Modify: `src/transcribe.rs` (add `#[cfg(feature = "test-helpers")] pub fn lang_state_allocations(&self) -> usize` on `WhisperEngine`)

---

- [ ] **Step 1: Write the failing test file**

Create `tests/transcribe_lang_state.rs`:

```rust
//! Tier 2 test for T2 perf-tweaks: lazy lang_state allocation.
//!
//! Requires ./models/ggml-tiny.en.bin on disk; gated by test-helpers feature
//! per AD0005 because it depends on a non-trivial fixture and uses the
//! test-only `WhisperEngine::lang_state_allocations()` accessor.

#![cfg(feature = "test-helpers")]

use std::path::PathBuf;
use std::time::Duration;

use uu_tiktok::transcribe::{EngineConfig, PerCallConfig, WhisperEngine};

fn tiny_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/ggml-tiny.en.bin")
}

fn skip_if_no_model() -> bool {
    if !tiny_model_path().exists() {
        eprintln!("Skipping: ./models/ggml-tiny.en.bin not found");
        true
    } else {
        false
    }
}

fn engine_config() -> EngineConfig {
    EngineConfig {
        model_path: tiny_model_path(),
        gpu_device: 0,
        flash_attn: false,
    }
}

#[tokio::test]
async fn lang_state_not_allocated_when_compute_lang_probs_never_true() {
    if skip_if_no_model() {
        return;
    }
    let engine = WhisperEngine::new(&engine_config()).expect("engine loads");

    // Send three non-opt-in requests; counter must stay at 0.
    for _ in 0..3 {
        let samples = vec![0.0_f32; 16000];
        let _ = engine
            .transcribe(samples, PerCallConfig::default(), Duration::from_secs(30))
            .await
            .expect("transcribe of silence should succeed");
    }

    assert_eq!(
        engine.lang_state_allocations(),
        0,
        "non-opt-in worker must never allocate lang_state"
    );
    engine.shutdown();
}

#[tokio::test]
async fn lang_state_allocated_exactly_once_across_opt_in_requests() {
    if skip_if_no_model() {
        return;
    }
    let engine = WhisperEngine::new(&engine_config()).expect("engine loads");

    let opt_in = PerCallConfig {
        language: None,
        compute_lang_probs: true,
    };

    // Three opt-in requests; counter goes 0 -> 1 -> stays 1.
    for i in 0..3 {
        let samples = vec![0.0_f32; 16000];
        let _ = engine
            .transcribe(samples, opt_in.clone(), Duration::from_secs(60))
            .await
            .expect("transcribe of silence should succeed");
        let count = engine.lang_state_allocations();
        assert_eq!(
            count, 1,
            "expected counter == 1 after request {}, got {}",
            i + 1, count
        );
    }

    engine.shutdown();
}
```

- [ ] **Step 2: Register the new test file in `Cargo.toml`**

Append to `Cargo.toml`:

```toml
[[test]]
name = "transcribe_lang_state"
required-features = ["test-helpers"]
```

- [ ] **Step 3: Run the test and verify it fails to compile**

Run:
```bash
cargo test --features test-helpers --test transcribe_lang_state --no-run
```

Expected: compile error mentioning `lang_state_allocations` (method does not exist on `WhisperEngine`). This is the failing-test baseline.

- [ ] **Step 4: Add the counter field + getter to `WhisperEngine`**

Modify `src/transcribe.rs` around line 345 (the existing `WhisperEngine` struct). Add the field; keep it unconditional (one heap alloc per engine, negligible cost in production):

```rust
pub struct WhisperEngine {
    request_tx: Option<mpsc::Sender<TranscribeRequest>>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Counter incremented each time the worker thread lazily allocates
    /// `lang_state` (at most once per worker lifetime). Always present so the
    /// worker capture doesn't branch on a feature flag; only exposed via the
    /// `test-helpers` getter below.
    lang_state_allocations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
```

(Use the actual existing field order/comments — only ADD the new field; do not rewrite existing fields. The exact JoinHandle field name lives in the file; preserve it.)

Add this method to the `impl WhisperEngine { … }` block, after `pub fn new`:

```rust
#[cfg(feature = "test-helpers")]
pub fn lang_state_allocations(&self) -> usize {
    self.lang_state_allocations
        .load(std::sync::atomic::Ordering::Relaxed)
}
```

- [ ] **Step 5: Wire the counter into `WhisperEngine::new` and replace the eager `lang_state` alloc**

Modify `src/transcribe.rs::new` (around line 360-453). Two changes:

**(a)** Before the `thread::Builder::new()` block, create the counter and a clone for the worker:

```rust
let lang_state_allocations = std::sync::Arc::new(
    std::sync::atomic::AtomicUsize::new(0),
);
let lang_state_allocations_worker = std::sync::Arc::clone(&lang_state_allocations);
```

**(b)** Replace the unconditional `let mut lang_state = match ctx.create_state() { … };` block at lines ~440-448 with:

```rust
// Lazily allocated on first compute_lang_probs=true request. AD0016:
// `WhisperState` stays inside the worker thread. AD0021 perf-tweaks
// rationale: opt-in workloads pay zero VRAM/host overhead for lang_state
// until first use; non-opt-in workers pay nothing. Counter on
// `WhisperEngine` is per-engine; the worker increments via the cloned Arc.
let mut lang_state: Option<whisper_rs::WhisperState> = None;
```

(Update the surrounding explanatory comment block — the old comment at lines 434-439 says "always allocated"; replace that with the new lazy-lifecycle description above.)

- [ ] **Step 6: Move the counter clone into the worker closure and add the lazy-alloc branch**

The worker thread closure starts around `let handle = thread::Builder::new()…spawn(move || { … })`. Two changes inside the closure:

**(a)** Add `let lang_state_allocations = lang_state_allocations_worker;` (or `let lang_state_allocations = std::sync::Arc::clone(&lang_state_allocations_worker);`) at the top of the closure body so the inner request-loop can access it.

**(b)** Inside the request loop body (the `while let Some(req) = request_rx.blocking_recv() { … }` block at line ~464), BEFORE the existing cancellation-check block at lines 470-475, add the lazy-alloc branch:

```rust
// Lazy lang_state allocation per T2 perf-tweaks. The
// `WhisperState::create_state` call is non-trivial (a second mel encoder
// + decoder context on the same model). Defer until first opt-in
// request. AD0016: state stays inside this thread; the counter Arc is
// the only thing that crosses out.
if req.config.compute_lang_probs && lang_state.is_none() {
    match ctx.create_state() {
        Ok(s) => {
            lang_state = Some(s);
            lang_state_allocations
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Err(e) => {
            // Failure on the lazy path: surface as TranscribeError::Bug
            // (matches the existing AudioDecodeError → Bug convention at
            // src/errors.rs; Epic 3's failure-classification taxonomy
            // will reclassify). Worker continues so subsequent non-opt-in
            // requests still work.
            let _ = req.reply.send(Err(crate::errors::TranscribeError::Bug {
                detail: format!(
                    "lazy lang_state create_state failure \
                     (should be classified, not Bug, in Epic 3): {e}"
                ),
            }));
            continue;
        }
    }
}
```

- [ ] **Step 7: Update the lang_detect call site to use `lang_state.as_mut()`**

The existing code at line ~574 reads `match lang_state.pcm_to_mel(…)`. After the type change, `lang_state` is `Option<WhisperState>`. Update:

Find the line:
```rust
let lang_probs = if req.config.compute_lang_probs {
    match lang_state.pcm_to_mel(&req.samples, 4) {
```

Change to:
```rust
let lang_probs = if req.config.compute_lang_probs {
    // The lazy-alloc branch above guarantees Some(_) here when we
    // reach this point. `expect` documents the invariant; if this
    // panics, the lazy branch's continue-on-error didn't fire.
    let lang_state = lang_state
        .as_mut()
        .expect("lazy alloc branch above guarantees Some(_)");
    match lang_state.pcm_to_mel(&req.samples, 4) {
```

The shadowing `let lang_state` rebinds in the local scope; all existing references to `lang_state.lang_detect(...)` etc. in that branch keep working unchanged.

- [ ] **Step 8: Wire the counter Arc into the `WhisperEngine` constructor return**

At the end of `WhisperEngine::new`, the existing code constructs and returns `Self { request_tx: Some(request_tx), handle: Some(handle) }` (or similar — preserve the exact existing field-order). Add the counter to the struct literal:

```rust
Ok(Self {
    request_tx: Some(request_tx),
    handle: Some(handle),
    lang_state_allocations,
})
```

- [ ] **Step 9: Run the test and verify it now compiles AND passes**

Run:
```bash
cargo test --features test-helpers --test transcribe_lang_state -- --nocapture
```

Expected:
- Both tests compile.
- If `models/ggml-tiny.en.bin` is on disk: both pass.
- If not: both print "Skipping: …" and pass trivially.

- [ ] **Step 10: Run the full test suite to verify no regression**

Run:
```bash
cargo test --features test-helpers
```

Expected: all existing tests pass. Special attention to `tests/whisper_engine_init.rs` — its `engine_loads_tiny_en_model_successfully` test exercises the same `WhisperEngine::new` path; if it skipped (no model) before, it skips now; if it ran, it passes.

If anything fails, **stop and resolve before committing.** Most likely failure mode: the `Option<WhisperState>` borrow in the lang_detect branch is incompatible with the surrounding control flow — adjust the rebinding pattern in Step 7.

- [ ] **Step 11: Run cargo fmt and clippy**

Run:
```bash
cargo fmt --all && cargo clippy --all-targets --features test-helpers -- -D warnings
```

Expected: no changes from fmt; clippy clean.

If clippy complains about the `.expect("lazy alloc branch above guarantees Some(_)")` pattern (sometimes flagged as `expect_used`), `#[allow(clippy::expect_used)]` with a one-line justification comment is the right call — this is a documented invariant.

- [ ] **Step 12: Commit**

```bash
git add src/transcribe.rs tests/transcribe_lang_state.rs Cargo.toml
git commit -m "$(cat <<'EOF'
refactor(transcribe): lazy-allocate lang_state on first opt-in request

Replaces the unconditional second WhisperState allocation at worker
startup with a lazy `Option<WhisperState>` allocated on the first
request with `compute_lang_probs=true`. Non-opt-in workers now never
pay for lang_state's VRAM/host overhead; opt-in workers pay exactly
once (on first use), with subsequent opt-in requests reusing the
same state.

AD0016 invariant preserved: WhisperState stays inside the worker
thread. The new `lang_state_allocations: Arc<AtomicUsize>` counter on
WhisperEngine is read-only from outside via a #[cfg(feature =
"test-helpers")] getter.

Failure on the lazy path surfaces as `TranscribeError::Bug { detail
}` per the existing AudioDecodeError -> Bug convention. Epic 3's
failure-classification taxonomy will reclassify.

New Tier-2 test file `tests/transcribe_lang_state.rs` asserts:
- non-opt-in worker: counter stays at 0 across N requests.
- opt-in worker: counter goes 0 -> 1 on first request, stays at 1.

Resolves FOLLOWUPS L87 (T8-Epic1 lazy lang_state, originally routed
to Plan C).

Refs: AD0001, AD0005, AD0016

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test transcribe_lang_state` passes (or skips with model-not-found).
- [ ] `cargo test --features test-helpers` shows no regression.
- [ ] `cargo clippy --all-targets --features test-helpers -- -D warnings` clean.
- [ ] `WhisperEngine` struct gained exactly one field (`lang_state_allocations: Arc<AtomicUsize>`); other fields unchanged.
- [ ] The eager `match ctx.create_state()` block for `lang_state` is gone from the worker spawn path.
- [ ] The lang_detect branch dereferences `lang_state` via `as_mut().expect(...)`, with a documented invariant comment.

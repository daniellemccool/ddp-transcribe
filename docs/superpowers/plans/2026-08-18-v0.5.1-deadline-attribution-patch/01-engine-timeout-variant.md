# Task 01 — Engine: deadline-fired aborts return `Timeout`, not `Cancelled`

**Files:**
- Modify: `src/transcribe.rs` (request struct ~:292, wrapper ~:1457, three
  attribution sites ~:988 / ~:1157 / ~:1183, request-literal in unit test
  ~:1842), `src/errors.rs:73-83` (enum doc comment)
- Test: `tests/whisper_engine_init.rs:122` (`transcribe_respects_short_deadline`
  — flip expected variant), `src/transcribe.rs` unit tests (request-literal
  sweep)

**Interfaces:**
- Consumes: `TranscribeError::Timeout { duration: Duration }`
  (`src/errors.rs:85`, already classified `Retryable` by
  `src/failure.rs:188`).
- Produces: the engine now returns `Timeout { duration }` whenever an abort
  or early-exit was caused by deadline elapse and the per-request cancel
  flag is NOT set; `Cancelled` is returned ONLY when `req.cancel` is set.
  Task 02's pipeline test relies on this attribution rule. The request
  struct gains `pub timeout: Duration` (the configured per-call budget,
  same value the wrapper used to compute `deadline`).

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Attribution rule (applies identically at all three sites): the predicate
that fires is `cancel || deadline`. Attribute by checking the cancel flag
at attribution time — if `req.cancel` is set, a coordinated shutdown is in
progress and `Cancelled` is correct regardless of the deadline; if it is
NOT set, the only remaining cause is the deadline → `Timeout`. (A racing
token-cancel that lands after a deadline-abort yields `Timeout` for this
item and clean cancellation for the next — both dispositions are safe.)

- [ ] **Step 1: Fetch the model if absent, then flip the engine test**

If `./models/ggml-tiny.en.bin` is missing:
`mkdir -p models && curl -L -o models/ggml-tiny.en.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin`

In `tests/whisper_engine_init.rs:122` (`transcribe_respects_short_deadline`),
change the expected variant from `Cancelled` to `Timeout`. The test currently
asserts something shaped like
`matches!(result, Err(TranscribeError::Cancelled))` — make it:

```rust
assert!(
    matches!(result, Err(TranscribeError::Timeout { .. })),
    "a deadline-fired abort must attribute as Timeout (retryable), not \
     Cancelled (coordinated shutdown); got: {result:?}"
);
```

(Adapt to the test's actual assertion idiom — read the test first; keep its
deadline setup unchanged.)

- [ ] **Step 2: Run it to verify it fails for the real reason**

Run: `cargo test --features test-helpers --test whisper_engine_init -- --test-threads=1 --ignored transcribe_respects_short_deadline`
Expected: FAIL — the engine still returns `Cancelled`. (If the model could
not be fetched, record the deviation and rely on Steps 4–6's compile-time
sweep + Task 02's pipeline test; do not skip silently.)

- [ ] **Step 3: Add the `timeout` field to the request struct**

In `src/transcribe.rs`: the request struct (the one carrying
`pub deadline: Instant` at ~:292) gains:

```rust
    /// The configured per-call budget `deadline` was computed from —
    /// carried so a deadline-fired abort can construct
    /// `TranscribeError::Timeout { duration }`.
    pub timeout: Duration,
```

Set it everywhere a request is constructed:
- the public `transcribe(...)` wrapper (~:1457): `timeout,` alongside the
  existing `deadline` computation;
- every other construction site — sweep with `rg 'deadline:' src/transcribe.rs`
  (known: the unit-test literal at ~:1842 uses
  `deadline: Instant::now() + Duration::from_secs(60)` → add
  `timeout: Duration::from_secs(60)`).

- [ ] **Step 4: Re-attribute the three sites**

Replace each conflated `Cancelled` return with the attribution rule.

Site 1 — early dequeue check (~:988):

```rust
    if req.cancel.load(std::sync::atomic::Ordering::Relaxed) {
        reply_and_log(req, Err(TranscribeError::Cancelled), request_seq, started);
        continue;
    }
    if Instant::now() >= req.deadline {
        let d = req.timeout;
        reply_and_log(
            req,
            Err(TranscribeError::Timeout { duration: d }),
            request_seq,
            started,
        );
        continue;
    }
```

Site 2 — post-lang_detect recheck (~:1157): same split; keep the existing
`drop(unsafe { Box::from_raw(abort_user_data) })` reclaim exactly where it
is (it must run on BOTH new early-exit arms — factor the two arms so the
reclaim happens before either `reply_and_log`, or duplicate the drop line in
each arm; do not leak the box on either path).

Site 3 — post-inference attribution (~:1183):

```rust
    match run_result {
        Err(_) if was_cancelled => {
            // The abort callback fired. Attribute by cause: a set cancel
            // flag means coordinated shutdown (ADR 0012); otherwise the
            // deadline is the only other predicate — a per-item timeout.
            let err = if req.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                TranscribeError::Cancelled
            } else {
                TranscribeError::Timeout { duration: req.timeout }
            };
            reply_and_log(req, Err(err), request_seq, started);
        }
        // ... (Bug and Ok arms unchanged)
```

- [ ] **Step 5: Rewrite the stale enum doc comment**

`src/errors.rs:73-83`: the comment currently says the embedded engine
"surfaces deadline-elapse via `Cancelled`" and that `Timeout` is
unconstructed. Rewrite the relevant sentences to state the new contract:
`Timeout { duration }` = per-request deadline elapsed (constructed by the
embedded engine since v0.5.1; classified Retryable); `Cancelled` = the
per-request cancel flag was set (coordinated shutdown / future drop —
ADR 0012). Keep the rest of the comment's history intact.

Also update the cancellation-composition doc comment in
`src/pipeline/pipelined.rs:565-590`: the "Error classification" bullet list
must note that `Cancelled` now means coordinated shutdown ONLY, and
deadline elapse arrives as `Timeout` via the classifier arm (Retryable).

- [ ] **Step 6: Run the engine test to verify it passes**

Run: `cargo test --features test-helpers --test whisper_engine_init -- --test-threads=1 --ignored transcribe_respects_short_deadline`
Expected: PASS. Also run the non-gated unit tests touched by the struct
change: `cargo test --features test-helpers --lib transcribe -- --test-threads=1`
Expected: PASS (compile-time sweep of request literals complete).

- [ ] **Step 7: Full gate and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "fix(transcribe): attribute deadline-fired aborts as Timeout, not Cancelled"`
(Disclose in the body: evidence is the 2026-08-17 run-kill; `Timeout` was
previously unconstructed by the embedded engine per the old enum comment.)

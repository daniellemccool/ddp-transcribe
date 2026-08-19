# Task 04 — Cap yt-dlp's internal retries in the argv

**Files:**
- Modify: `src/fetcher/ytdlp.rs` — `build_yt_dlp_args` (~:97) and
  `build_metadata_only_args` (~:174)
- Test: the existing argv unit tests in `src/fetcher/ytdlp.rs`'s
  `#[cfg(test)]` module (extend; follow their exact assertion idiom)

**Interfaces:**
- Consumes: nothing from other tasks (independent).
- Produces: both production argv builders emit `--retries 3`. Resolves the
  FOLLOWUPS entry "yt-dlp internal retry count is uncapped in our argv
  (default 10)" — Task 05 archives it citing this task's commit.

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth: yt-dlp's own `--retries` defaults to 10; observed live
2026-08-13 as `Giving up after 10 retries` after ~3.5 minutes of 20 s
connect timeouts — one stalled download worker per occurrence, 3/1,458
claims. Our claim-level retry (ADR-0036 fetch-as-oracle) re-adjudicates
anyway, so deep internal retries are redundant. The giving-up message lands
in the `YtDlpOther` fallback — no ADR-0033 pattern pins that text, so
changing the count breaks no classification. Do NOT add `--socket-timeout`
(considered and deferred: the 20 s connect default is fine; YAGNI).
Placement: insert the pair adjacent to whichever stable flags each builder
already emits (match the surrounding argv style — each flag and its value
are separate `String` elements, same as the existing entries).

- [ ] **Step 1: Extend the failing tests**

In the existing argv unit tests (there is at least one asserting
`build_yt_dlp_args`'s contents and one for `build_metadata_only_args` —
read them first, extend in their idiom):

```rust
// In the build_yt_dlp_args test(s):
let args = build_yt_dlp_args(/* the test's existing inputs */);
let pos = args.iter().position(|a| a == "--retries")
    .expect("--retries present in fetch argv");
assert_eq!(args[pos + 1], "3", "internal retry cap is 3");

// Mirror the same two assertions in the build_metadata_only_args test(s).
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --features test-helpers --lib ytdlp -- --test-threads=1 retries`
(adjust the filter to the amended tests' names)
Expected: FAIL — `--retries` absent from both argvs.

- [ ] **Step 3: Implement**

In both builders, alongside the existing stable flags:

```rust
    args.push("--retries".into());
    args.push("3".into());
```

(Adapt to each function's construction style — if it builds a `vec![...]`
literal, add the two elements there instead of `push`.) Add one comment at
the first insertion:

```rust
    // Cap yt-dlp's internal retry loop (default 10): our claim-level retry
    // (ADR-0036) re-adjudicates transient failures anyway, and 10×20 s
    // connect timeouts stalled a download worker ~3.5 min per occurrence
    // (observed 2026-08-13). Argv is code + params_json — never a config
    // file (incident-2 lesson).
```

- [ ] **Step 4: Run the touched tests, then the full gate**

Run: `cargo test --features test-helpers --lib ytdlp -- --test-threads=1`
Expected: PASS (including all pre-existing argv assertions — if any
existing test asserts an exact full argv vector, update that expectation in
the same commit and say so in the report).
Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "fix(fetcher): cap yt-dlp internal retries at 3 in both argv builders"`

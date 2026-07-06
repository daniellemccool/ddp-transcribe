# Task 06: Split `tests/pipeline_fakes.rs` into modules; strip narration; audit worker-level tests

**Files:**
- Create: `tests/pipeline_fakes/main.rs`, `tests/pipeline_fakes/fakes.rs`, `tests/pipeline_fakes/serial_tests.rs`, `tests/pipeline_fakes/fetch_worker_tests.rs`, `tests/pipeline_fakes/transcribe_worker_tests.rs`, `tests/pipeline_fakes/pipelined_tests.rs`
- Delete: `tests/pipeline_fakes.rs`
- No `Cargo.toml` change: the existing `[[test]] name = "pipeline_fakes"` target resolves `tests/pipeline_fakes/main.rs` automatically once the single file is gone (cargo's standard tests-dir layout; a directory test target's entry point is `main.rs`).

**Interfaces:**
- Consumes: the current ~1000-line `tests/pipeline_fakes.rs`.
- Produces: the same test suite, relocated. **Zero behavioral change** — this is a mechanical move + comment strip so Tasks 07/08 land their extensions in focused files. `FakeTranscriber` and shared fixture helpers end up in `fakes.rs`, re-exported through `main.rs` (`mod fakes; use fakes::*;` in each sibling via `super::fakes` or crate-level `pub use`).

Resolves FOLLOWUPS: "pipeline_fakes.rs is 1000 lines mixing concerns" + "over-reliance on worker-level entry points" (audit part).

- [ ] **Step 1: Record the baseline**

```bash
cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1 2>&1 | tail -3
```
Note the exact test count and names (`--list` if needed: `cargo test --features test-helpers --test pipeline_fakes -- --list`). The split is complete only if the identical set passes afterward.

- [ ] **Step 2: Move code into the module layout**

`tests/pipeline_fakes/main.rs`:

```rust
//! Pipeline integration suite over controllable fakes. Split by concern
//! (Epic 3): fakes + fixtures in `fakes`, then one module per entry point.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test code; matches sibling suites

mod fakes;
mod fetch_worker_tests;
mod pipelined_tests;
mod serial_tests;
mod transcribe_worker_tests;
```

(Carry over the exact clippy-allow header the current file uses — check its first lines and copy verbatim.)

Distribution rules:
- `fakes.rs`: `FakeTranscriber`, `FetchedItem` constructors, fixture-DB helpers, WAV/tempdir helpers — everything `pub(crate)` so sibling modules reach it via `crate::fakes::…` (in an integration-test binary, `main.rs` is the crate root, so `use crate::fakes::helper;` works from each module).
- `serial_tests.rs`: `run_serial` / `process_one`-level tests (`pipeline_processes_one_video_to_succeeded`, `run_serial_classifies_*`, …).
- `fetch_worker_tests.rs`: direct `fetch_worker` tests incl. the gated stale-race test.
- `transcribe_worker_tests.rs`: direct `transcribe_worker` tests incl. its stale-race test.
- `pipelined_tests.rs`: `run_pipelined` orchestration tests.

While moving, **strip the phase narration**: delete `T16`/`T17`/`T18` task references, "this design was added in commit X" comments, and ADR citations *inside test bodies*. Keep comments that state the behavior under test or a non-obvious fixture constraint. Do not rename tests; do not change assertions.

- [ ] **Step 3: Run the suite; compare to baseline**

Run: `cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1`
Expected: PASS with the same test count as Step 1. `cargo clippy --all-targets -- -D warnings` clean.

- [ ] **Step 4: Worker-level test audit (documentation, not deletion)**

For each test in `fetch_worker_tests.rs` / `transcribe_worker_tests.rs`, answer: *does this exercise a path `run_pipelined` couldn't reach with appropriate fakes?* Add a one-line verdict comment above each test:
- `// worker-level: REQUIRED — deterministic interleaving via gate, unreachable from run_pipelined` (the stale-race tests), or
- `// worker-level: candidate for run_pipelined-level replacement (audit Epic 3 T06); kept as-is`

Do NOT rewrite candidates in this task — the audit output is the comment layer plus a count in the commit message. (Rewrites would churn the same files Tasks 07/08 are about to extend; the FOLLOWUPS entry's replacement suggestion lands opportunistically when those tasks touch a candidate.)

- [ ] **Step 5: Commit**

```bash
git rm tests/pipeline_fakes.rs
git add tests/pipeline_fakes/
git commit -m "test: split pipeline_fakes into per-concern modules; strip phase narration; audit worker-level tests

Mechanical relocation, zero behavioral change: N tests before == N after.
Audit verdicts inline per FOLLOWUPS; X of Y worker-level tests marked
REQUIRED (timing-dependent), rest marked replacement candidates."
```

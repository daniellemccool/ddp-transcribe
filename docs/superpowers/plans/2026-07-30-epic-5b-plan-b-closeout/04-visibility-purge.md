# Task 04: visibility narrowing + dead-code-allow purge (Phase 1)

**Files:**
- Modify: `src/lib.rs` (`#![warn(unreachable_pub)]`), every `src/**/*.rs` with over-broad `pub` or a stale `#[allow(dead_code)]`
- Test: no new tests — suite-preservation evidence

**Interfaces:**
- Consumes: Task 03's façade (`pub use cli::{Cli, LogFormat}; pub use commands::{dispatch, CommandExit};`).
- Produces: the final visibility surface Phase 2/3 tasks build on — `pub` = façade + `tests/` imports + `test-helpers`-gated items (ADR-0005); all else `pub(crate)`/private.

**Semantics (binding):**
- Behavior-preserving; no logic edits. Mechanical visibility narrowing + allow deletion only.
- Add `#![warn(unreachable_pub)]` to `src/lib.rs`; drive warnings to zero (clippy gate runs with `-D warnings`, so unreachable_pub findings are build failures — that is intended).
- Of the 46 baseline `#[allow(dead_code)]`s: delete every allow whose justification comment names the bin/lib double compilation or the `--features test-helpers` unification wrinkle; for each remaining allow, either its justification still names a real future consumer (keep, comment refreshed if stale) or it is deletable because the item is now visibly dead (delete item + allow). Zero bare allows survive (amended ADR-0002).
- What `tests/` legitimately imports stays `pub`; prefer moving test-only items behind `#[cfg(feature = "test-helpers")]` (ADR-0005) over leaving them `pub` — but do NOT restructure test scaffolding beyond visibility.
- Record the final allow census in the commit body (`rg -c 'allow\(dead_code\)' src/`).

- [ ] **Step 1:** Add `#![warn(unreachable_pub)]`; `cargo clippy --all-targets --features test-helpers -- -D warnings` and fix every finding by narrowing (never by adding façade exports without an ADR-grounded reason, disclosed per ADR-0003).
- [ ] **Step 2:** Allow purge per the binding rule; `rg 'allow\(dead_code\)' src/` before/after.
- [ ] **Step 3: Full gate** (fmt, clippy, single-threaded suite, `cargo build --release`). Census must still be 261 runnable / 10 ignored.
- [ ] **Step 4: Commit**

```bash
git add src/
git commit -m "refactor(visibility): pub(crate) sweep + unreachable_pub warn; dead-code allow purge (46 -> <N>) per amended ADR-0002"
```

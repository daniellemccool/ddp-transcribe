# Task 02: crate-shape ADR + ADR-0002 amendment (Phase 1)

**Files:**
- Create (via adg ONLY): new lean ADR — thin bin / fat lib crate shape with a minimal public façade
- Modify (via the write-lean-adr amendment workflow): `docs/decisions/0002-dead-code-is-suppressed-with-allow-dead-code-plus-a-justification-comment.md`

**Interfaces:**
- Consumes: ADR-0002's Context (which already anticipates this restructure); the spec's Phase-1 section (façade, visibility policy).
- Produces: the accepted record Tasks 03–04 implement. Its number is whatever `adg` assigns — later tasks reference it by title, not number.

**Semantics (binding):**
- The executing subagent MUST load `write-adr:write-lean-adr` before touching `docs/decisions/` and author through `adg lean new --from-stdin`; amendments follow the skill's evolved-record path. Run `adg lean index --root .` + `adg lean check`; fix, never bypass.
- **New record's Decision (content, not wording):** the crate is a fat library with a thin binary; `src/lib.rs` is the single module root; `src/main.rs` contains no `mod` declarations and owns exactly: argument parsing, tracing init, error rendering, final `std::process::exit`. The library never calls `process::exit`.
- **Guidance the record must carry:** the public façade is exactly `pub use cli::{Cli, LogFormat}` and `pub use commands::{dispatch, CommandExit}` at the crate root, plus items `tests/` imports and `test-helpers`-gated scaffolding (ADR-0005); everything else `pub(crate)` or private; `#[warn(unreachable_pub)]` is the backstop; review rejects new root-level `pub` items without a façade rationale. `[profile.release] lto = "thin"` exists so cross-crate inlining is a non-question; the release build is part of the verification gate.
- **ADR-0002 amendment:** with double compilation gone, pub-lib items are exempt from `dead_code` — the suppression policy's backstop shifts to visibility narrowing; allows remain only for genuinely-forward scaffolding with a named consumer; the `#[expect]` prohibition rationale (bin+lib shape) is updated to match the new single-root reality.
- `applies_to` for the new record: `src/main.rs`, `src/lib.rs`, `Cargo.toml` (match how existing ADRs scope; adg shapes it).

- [ ] **Step 1:** Load `write-adr:write-lean-adr`; author the new record via `adg lean new --from-stdin` with the Decision/Guidance substance above (skill's format governs wording).
- [ ] **Step 2:** Amend ADR-0002 per the skill's amendment path (Decision text, Guidance bullets, Why updated; Context keeps the history).
- [ ] **Step 3:** `adg lean index --root .` + `adg lean check` on both records — 0 failures (the 2 pre-existing 0040/0041 warnings are acceptable).
- [ ] **Step 4: Verification** — full gate (docs-only; must stay green): `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`.
- [ ] **Step 5: Commit**

```bash
git add docs/decisions/
git commit -m "docs(adr): thin-bin/fat-lib crate shape with minimal facade; 0002 amended — visibility narrowing replaces double-compilation suppression"
```

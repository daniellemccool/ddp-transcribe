# Task 12: test-hardening bundles — Epic 3 list + v0.3.1 CLI list (Phase 3)

**Files:**
- Test: `tests/process_bounded_capture.rs` (signal-capture spawn+kill), inline `src/classification.rs` tests (precedence/case), `tests/pipeline_fakes/pipelined_tests.rs` (kind-string end-to-end), `tests/cli.rs` + `tests/backfill_metadata.rs` (v0.3.1 CLI list), `src/cli.rs` (one production change: `conflicts_with`)
- Modify: `src/cli.rs` ONLY for `backfill-metadata --dry-run`/`--limit` `conflicts_with` (the single behavior change in this task)

**Interfaces:**
- Consumes: FOLLOWUPS bodies — "Epic 3 final review: test-hardening bundle" (in `docs/followups/epic-5.md`) and "v0.3.1 review: CLI test-hardening bundle" (same file); the post-Task-03 crate shape (tests import via the façade/test-helpers).
- Produces: tests only (plus the one clap constraint).

**Semantics (binding — each item's full text lives in its body; read both first):**
- Epic 3 list: a real spawn+kill test proving signal capture (the `signal` field lands in `CommandOutcome` since 9974d69 — the test exercises it end-to-end via a shim that self-kills); `classify_message` precedence/case tests per the body's cases; `transcribe_worker` kind-string end-to-end assertion in the fakes harness.
- v0.3.1 CLI list: the `global = true` both-position test gains value-propagation + duplicate-precedence assertions (not just parse acceptance); `backfill-metadata --dry-run --limit N` becomes a parse error via `conflicts_with` (TDD: the parse test first); the backfill dry-run test gains a PATH shim so it can't silently hit a real binary; `statuses()` snapshot gains claim/attempt columns per the body.
- Test-only task except the `conflicts_with` line; every new test asserts real behavior (no mock-only assertions); `-- --test-threads=1` always.

- [ ] **Step 1: Read both bodies**; enumerate items in the report.
- [ ] **Step 2:** For the `conflicts_with` change: failing parse test → RED → clap change → GREEN. For pure test additions: write each, run focused, confirm it passes AND that it fails when its subject is deliberately broken (mutate locally, revert — state this evidence for at least the signal-capture and value-propagation tests).
- [ ] **Step 3: Full gate** (incl. release build).
- [ ] **Step 4: Commit**

```bash
git add tests/ src/cli.rs src/classification.rs
git commit -m "test: Epic 3 + v0.3.1 hardening bundles — signal capture, classification precedence, kind-string e2e, CLI value-propagation, backfill dry-run/limit conflict + PATH shim"
```

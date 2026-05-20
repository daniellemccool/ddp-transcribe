# Cosmetic FOLLOWUPS — deferred indefinitely

Entries deferred indefinitely — touch when the surrounding file gets edited
for unrelated reasons. These are not on the orchestrator's planning-time
reading path. Sibling files: `docs/FOLLOWUPS.md` (active-scope entries),
`docs/bake-findings.md` (operational bake observations),
`docs/archive/followups-resolved.md` (append-only history).

---

## `whisper_engine_init` integration tests serialize for cleaner timing assertions

**Found in:** T8 (lang_probs opt-in) — wallclock guard in `transcribe_respects_short_deadline` had to be relaxed from 10s to 30s because parallel cargo test execution (5 whisper tests, each allocating ~1GB of WhisperState buffers and running model load + inference) causes 10s+ elapsed under CPU contention.
**Disposition:** Defer; current 30s guard catches true hangs (which would exceed the test-harness 60s timeout). Revisit if flakiness recurs in T9+.
**Trigger to revisit:** A subsequent `cargo test --features test-helpers` run shows whisper_engine_init flaking on `transcribe_respects_short_deadline` or any other tightly-timed test, OR T9/T11/T12 adds further whisper_engine_init tests that increase parallelism.

Approaches when this comes up:
1. Add `serial_test = "3"` to dev-deps; annotate `#[serial(whisper_engine_init)]` on each test. Cleanest semantics; adds one dev-dep.
2. Move whisper_engine_init's tests into a single `#[tokio::test]` function (serial within tokio's runtime). Loses test isolation but no new deps.
3. Document `cargo test -- --test-threads=1` for whisper_engine_init binary specifically (brittle; requires CI to know).

Cost of (1) is one crate dep + ~5 attribute lines. Worth it if the tighter timing assertions become important again (e.g., catching a cancellation latency regression).

---

## `adg comment` rewrites the rendered Comments section with only the latest entry

**Found in:** T2 (cargo-deps amendment to 0009 via `adg comment`).
**Disposition:** Tool quirk; tracked but not blocking.
**Trigger to revisit:** If future ADR amendments require the full comment history visible in the rendered body — e.g., a multi-step decision with several attributed clarifications.

When `adg comment --id NNNN` is invoked on an ADR that already has comments,
the rendered .md body's `## Comments` section is rewritten to show only the
new comment's anchor and line; prior comments remain in `index.yaml` but their
`<a name="comment-N"></a>` anchors disappear from the body. `adg validate`
accepts this state (it checks the anchors that ARE present, not that all
indexed comments are anchored). Workaround for T2: manually restored
comment-1's anchor in 0009 before commit so the rendered body matches
`index.yaml`'s comment list. If this pattern recurs in T3-T12, propose an
upstream `adg` fix.

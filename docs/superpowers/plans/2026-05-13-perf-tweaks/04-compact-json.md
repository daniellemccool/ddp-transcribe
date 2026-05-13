# Task 4 — Compact JSON for transcript metadata

**Goal:** Replace `serde_json::to_vec_pretty` with `serde_json::to_vec` in `src/pipeline.rs:151` so transcript `.json` artifacts are written in compact form. Removes leading-whitespace indentation that bloats the per-token raw_signals payload.

**Spec commit:** 3 — `perf(pipeline): write compact JSON for transcript metadata`.

**ADRs directly relevant:**
- **AD0008** — artifact-write-before-mark_succeeded ordering. This task changes only the encoder, not the order; the invariant is preserved.
- **AD0010** — raw_signals schema v1 pass-through rule. JSON is a textual serialization; compact form does NOT change the JSON object shape. Schema unchanged.

**Files:**
- Modify: `src/pipeline.rs:151` (one-line swap)
- Modify or extend: `tests/output_artifacts.rs` (existing or new file; add compact-form assertions)

---

- [ ] **Step 1: Check if `tests/output_artifacts.rs` exists**

Run:
```bash
ls tests/output_artifacts.rs 2>&1
```

If the file does NOT exist, create it (Step 1a). If it DOES exist, extend it (Step 1b).

- [ ] **Step 1a (if new file): Create `tests/output_artifacts.rs`**

```rust
//! Tier 1 tests for output artifact serialization.

use uu_tiktok::output::artifacts::{
    EXPECTED_RAW_SIGNALS_SCHEMA_VERSION, RawSegment, RawSignals, RawToken, TranscriptMetadata,
};

fn sample_metadata() -> TranscriptMetadata {
    TranscriptMetadata {
        video_id: "abc123".into(),
        source_url: "https://example.com/v".into(),
        duration_s: 35.6,
        language_detected: Some("en".into()),
        transcribed_at: "2026-05-13T10:00:00+00:00".into(),
        fetcher: "ytdlp".into(),
        transcript_source: "whisper-rs".into(),
        model: "ggml-tiny.en.bin".into(),
        raw_signals: Some(RawSignals {
            schema_version: EXPECTED_RAW_SIGNALS_SCHEMA_VERSION.to_string(),
            language: "en".into(),
            lang_probs: None,
            segments: vec![RawSegment {
                no_speech_prob: 0.02,
                tokens: vec![RawToken {
                    id: 50364,
                    text: " hello".into(),
                    p: 0.95,
                    plog: -0.051,
                }],
            }],
        }),
    }
}
```

Add `[[test]]` registration in `Cargo.toml` if needed:

```toml
[[test]]
name = "output_artifacts"
```

(No `required-features` — this is a Tier 1 test with no fixture dependency.)

- [ ] **Step 1b (if file exists): No setup needed**

Skip to Step 2; you'll append the new tests to the existing file.

- [ ] **Step 2: Write the failing tests**

Append to `tests/output_artifacts.rs`:

```rust
#[test]
fn compact_json_round_trips_metadata() {
    let metadata = sample_metadata();

    // Compact serialization (perf-tweaks T4 change).
    let bytes = serde_json::to_vec(&metadata).expect("compact serialize");

    // Round-trip: compact bytes parse back to the same structural value.
    let parsed: TranscriptMetadata =
        serde_json::from_slice(&bytes).expect("parse compact bytes");
    assert_eq!(parsed.video_id, metadata.video_id);
    assert_eq!(parsed.duration_s, metadata.duration_s);
    assert_eq!(
        parsed.raw_signals.as_ref().map(|r| &r.schema_version),
        metadata.raw_signals.as_ref().map(|r| &r.schema_version),
    );
}

#[test]
fn compact_json_has_no_indent_whitespace() {
    let metadata = sample_metadata();
    let bytes = serde_json::to_vec(&metadata).expect("compact serialize");
    let s = std::str::from_utf8(&bytes).expect("utf8");

    // The structural test that compact differs from pretty: no
    // newline-followed-by-spaces patterns indicating pretty-print indent.
    assert!(
        !s.contains("\n  "),
        "compact JSON must not contain newline+spaces indent"
    );
    assert!(
        !s.contains("\n    "),
        "compact JSON must not contain newline+4-spaces indent"
    );
}
```

(The size-reduction assertion is informational; not asserted per the second-review tightening — a one-token fixture is too small. If a multi-token fixture exists elsewhere, the implementation plan can opportunistically add a size-reduction assertion using it.)

- [ ] **Step 3: Run the new tests — verify they pass NOW (since they test the new compact form, which already works via `to_vec`)**

Run:
```bash
cargo test --test output_artifacts
```

Expected: both new tests pass — `serde_json::to_vec` is already available; the tests just verify its output shape. (The "failing test" pattern here is: the tests assert the *new* compact-output property; the pipeline.rs change is what wires that property into actual artifact writes.)

- [ ] **Step 4: Now make the pipeline change**

In `src/pipeline.rs` around line 151, find:
```rust
    let json_bytes =
        serde_json::to_vec_pretty(&metadata).context("serializing transcript metadata")?;
```

Change to:
```rust
    // T4 perf-tweaks: compact JSON shrinks raw_signals payload meaningfully
    // (per-token id+text+p+plog dominates by token count; pretty-print added
    // ~3x bloat). AD0008 ordering preserved; AD0010 schema shape unchanged
    // (compact and pretty are equivalent JSON values).
    let json_bytes =
        serde_json::to_vec(&metadata).context("serializing transcript metadata")?;
```

- [ ] **Step 5: Run the full test suite**

Run:
```bash
cargo test --features test-helpers
```

Expected: all tests pass. The pipeline-integration tests (if any read the JSON back) keep working because round-trip parsing is unaffected.

- [ ] **Step 6: cargo fmt + clippy**

Run:
```bash
cargo fmt --all && cargo clippy --all-targets --features test-helpers -- -D warnings
```

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/pipeline.rs tests/output_artifacts.rs Cargo.toml
git commit -m "$(cat <<'EOF'
perf(pipeline): write compact JSON for transcript metadata

Replaces `serde_json::to_vec_pretty` with `serde_json::to_vec` in
the artifact-writing path. Per-token raw_signals payload (id + text
+ p + plog × N tokens) dominates JSON size by token count; pretty-
print added ~3x bloat across pretty-print whitespace. Compact form
keeps the schema shape identical (AD0010), preserves AD0008's
artifact-write-before-mark_succeeded ordering, and round-trips
losslessly.

Partially resolves FOLLOWUPS L89 (T10-Epic1 per-token text doubles
raw_signals payload). The drop-text-field component of L89 remains
deferred to Plan C (requires AD0010 amendment ADR + bake validation
of downstream filtering).

Tier-1 tests in tests/output_artifacts.rs:
- compact_json_round_trips_metadata: parse-back equality.
- compact_json_has_no_indent_whitespace: structural difference from
  pretty form.

Size reduction documented as informational rather than asserted; a
one-token fixture is too small to make a non-brittle relative-size
claim.

Refs: AD0008, AD0010

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `src/pipeline.rs:151` uses `to_vec`, not `to_vec_pretty`.
- [ ] Both new tests pass: round-trip + no-indent-whitespace.
- [ ] `cargo test --features test-helpers` shows no regression.
- [ ] FOLLOWUPS L89 amendment is queued for T11 (the FOLLOWUPS-update commit). This task does NOT touch FOLLOWUPS.md itself — T11 handles all archive moves with the commit SHAs as references.

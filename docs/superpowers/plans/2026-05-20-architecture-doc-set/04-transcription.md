# Task 4 — Populate `transcription.md`

**Goal:** Replace every `(TBD)` in `docs/reference/architecture/transcription.md` with real content covering three boundaries: audio prep (`src/audio.rs`), transcription (`src/transcribe.rs` and its whisper-rs integration), and output (`src/output/`). This is the most ADR-redirect-heavy file in the set — eight ADRs directly govern this subsystem.

**ADRs referenced:** 0004, 0008, 0009, 0010, 0012, 0013, 0014, 0015, 0016 (full titles in the local ADR-map step below).

**Files:**
- Modify: `docs/reference/architecture/transcription.md`
- Read (to extract integration depth): `src/audio.rs`, `src/transcribe.rs`, `src/output/` (mod.rs and any helpers), `Cargo.toml` (verify `whisper-rs` version pin), `docs/reference/whisper-cpp-deepdive.md` (for cross-references — *do not duplicate*)

**Pre-reqs:** T01, T02 complete. (T03 not required, but recommended — having `data-input.md` complete makes the "audio handoff from fetcher" cross-link concrete.)

---

## Discipline reminder

`whisper-cpp-deepdive.md` already covers whisper.cpp internals at external-tool depth. This file covers *our integration* at design depth. The split:
- **In `transcription.md`:** the parameters we set, the callbacks we wire, the configuration choices we made, the invariants we enforce on our side.
- **In `whisper-cpp-deepdive.md`:** how the upstream code uses those parameters internally, what other parameters exist, the upstream sampling/fallback loop.

Cross-reference into `whisper-cpp-deepdive.md` where understanding the *why* of our choice requires upstream context.

---

- [ ] **Step 1: Survey source files**

```bash
wc -l src/audio.rs src/transcribe.rs && ls src/output/
grep -n "^fn\|^pub fn\|^impl\|whisper_rs\|WhisperContext\|FullParams" src/transcribe.rs
grep -n "^fn\|^pub fn\|^impl" src/audio.rs
ls src/output/ && wc -l src/output/*.rs
grep -A2 "whisper-rs" Cargo.toml
```

Note for later:
- whisper-rs version (pinned per ADR 0009; cite the exact version in §"whisper-rs integration").
- The configuration choices in `transcribe.rs`: `WhisperContextParameters` (use_gpu, flash_attn, gpu_device), `FullParams` (language, temperature, no_speech_thold, suppress_*).
- Cancellation callback wiring (look for `set_abort_callback` or similar).
- Engine-state shape (look for `WhisperState` references and how many states are created).
- Raw signals extraction (look for `no_speech_prob`, `lang_probs`, segment getters).

Read further with `Read` to ground claims; do not paraphrase upstream behavior — defer to `whisper-cpp-deepdive.md` for that.

- [ ] **Step 2: Write Audio preparation section**

Replace the `## Audio preparation` `(TBD)` block with content covering:

**Intro (3 sentences):** explain that audio prep is the boundary between fetched media and the whisper engine. It enforces the PCM 16kHz mono float32 invariant ([ADR 0014](../../decisions/0014-audio-input-invariant-float32-pcm-16khz-mono-via-hound.md)) — whisper.cpp requires this format and we normalize at this single point. Implemented in `src/audio.rs`.

**`### PCM format invariant`** — describe what the invariant requires, where in `src/audio.rs` it's enforced (cite file:line), how violations are caught (panic? typed error? assert?). Redirect to ADR 0014 for the *why*. Half-sentence only on the *why* ("whisper.cpp requires float32 PCM 16kHz mono").

**`### Source: the fetcher handoff`** — describe the input format `src/audio.rs` accepts (probably a WAV path produced by the fetcher's audio-extraction post-step) and what conversion the audio prep does (resampling? channel mixdown? sample-format conversion?). Cite `hound` usage explicitly — the WAV reader is hound, and the byte-to-float32 conversion is our code on top. Cross-link to `data-input.md`'s "Audio extraction handoff" section.

Aim for ~30-50 lines for the whole Audio prep section.

- [ ] **Step 3: Write Transcription section intro**

Replace `## Transcription` `(TBD)` with a 2-3 sentence intro:

```markdown
## Transcription

Transcription is a thin wrapper around `whisper-rs`, pinned to a specific upstream commit per [ADR 0009](../../decisions/0009-use-whisper-rs-for-whisper-cpp-embedding-with-version-pin-and-fallback-policy.md). The wrapper configures the context and per-inference parameters, wires a cancellation callback for cooperative shutdown ([ADR 0012](../../decisions/0012-cooperative-cancellation-via-per-request-arc-atomicbool-and-abort-callback.md)), verifies GPU backend at startup ([ADR 0013](../../decisions/0013-gpu-verification-at-startup-assert-backend-and-log-device-name.md)), and extracts the raw confidence signals whisper.cpp exposes ([ADR 0010](../../decisions/0010-json-artifact-raw-signals-schema-pass-through-with-schema-version.md)). For the upstream sampling and fallback loop these choices feed into, see [`whisper-cpp-deepdive.md`](../whisper-cpp-deepdive.md).
```

- [ ] **Step 4: Write `### whisper-rs integration`**

Cover:
- The pinned `whisper-rs` version (read from `Cargo.toml`; cite the exact version string).
- The underlying whisper.cpp commit/version this corresponds to (the `Cargo.toml` comment likely names it; cite verbatim).
- The fallback policy ADR 0009 specifies (link, don't restate).
- That whisper-rs is embedded — there is no separate subprocess or server; the model is loaded in-process and inference runs against a `WhisperContext` we own.

- [ ] **Step 5: Write `### Context and full-params configuration`**

This is the meatiest sub-section. For each `WhisperContextParameters` field and each `FullParams` field we explicitly set, write a bullet:
- The parameter name.
- The value we set.
- One sentence on what it does (avoid restating the whisper.cpp manual).
- Half-sentence on why we set it that way (skip if obvious).

If a parameter is explicitly *not* overridden (defaults relied on), call that out where the default has architectural significance — e.g., we rely on `no_context = true` (default) to clear prompt history per call.

Cross-link to the relevant ADRs:
- `use_gpu = true` → [ADR 0013](../../decisions/0013-gpu-verification-at-startup-assert-backend-and-log-device-name.md) for GPU verification.
- Any temperature/threshold setting → cite the rationale ADR if one exists; otherwise describe the choice plainly.

For details that would balloon this section (full whisper.cpp option matrix), redirect: "Other defaults inherited; see `whisper-cpp-deepdive.md §2`."

- [ ] **Step 6: Write `### Cancellation wiring`**

Describe how cooperative cancellation is plumbed:
- An `Arc<AtomicBool>` per request (per [ADR 0012](../../decisions/0012-cooperative-cancellation-via-per-request-arc-atomicbool-and-abort-callback.md)).
- How the abort callback is registered on `FullParams` (cite file:line).
- How the orchestrator triggers cancellation (likely via `CancellationToken` whose listener flips the `AtomicBool`; cite the relevant code).
- That cancellation is *cooperative* — whisper.cpp polls the abort callback at well-defined points (see `whisper-cpp-deepdive.md`); ungraceful termination requires process kill.

Redirect the *why* to ADR 0012; describe the *what* and *where* in this file.

- [ ] **Step 7: Write `### GPU verification`**

Brief — one paragraph. The startup path verifies the GPU backend is active (per [ADR 0013](../../decisions/0013-gpu-verification-at-startup-assert-backend-and-log-device-name.md)) and logs the device name. If verification fails, the binary exits with a clear error rather than silently falling back to CPU (which would slow inference by ~50×). Cite the code path that performs the check.

- [ ] **Step 8: Write `### Engine-state model`**

Describe the per-inference state model:
- A `WhisperContext` is the model (loaded once, read-only after load).
- A `WhisperState` is per-inference scratch (KV caches, decoders, etc.).
- We hold one context across all transcribe calls; how many states we hold depends on the topology (currently one — see [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md) — but the engine API is stable across single-state and multi-state per [ADR 0016](../../decisions/0016-architecture-for-parallelism-engine-api-stable-across-single-and-multi-state.md)).
- Explicit non-use of `whisper_full_parallel` per [ADR 0015](../../decisions/0015-explicit-non-use-of-whisper-full-parallel.md) — we drive parallelism via engine states (when we have them) at the orchestrator layer, not via the upstream parallel function.

- [ ] **Step 9: Write `### Raw signals extraction`**

Describe what raw confidence/quality signals we extract per segment and per request:
- `no_speech_prob` (per-segment) — surfaced from whisper.cpp's pre-logit-filtering snapshot (see `whisper-cpp-deepdive.md §2` for the upstream extraction point).
- `lang_probs` (per-request, when language is auto-detected).
- Token-level logprobs / segment-level metrics if extracted.

Redirect the schema details to [ADR 0010](../../decisions/0010-json-artifact-raw-signals-schema-pass-through-with-schema-version.md) — restate only the names and where they end up in the artifact. Do not re-document the full schema.

- [ ] **Step 10: Write Output section**

Replace `## Output` `(TBD)` and its three sub-sections.

**Intro (2 sentences):** the output writer turns a completed transcription into a JSON artifact on disk. The artifact is written atomically and *before* the state row is flipped to succeeded (per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md)) so a crash mid-write leaves the row in `in_progress` for retry rather than orphaning the artifact.

**`### Artifact shape`** — describe the top-level JSON structure (one artifact per video). Cite the schema-version field and reference [ADR 0010](../../decisions/0010-json-artifact-raw-signals-schema-pass-through-with-schema-version.md) for the full schema. Don't reproduce the schema; describe only the top-level shape and what raw signals are passed through.

**`### Sharding`** — describe the on-disk layout. Per [ADR 0004](../../decisions/0004-transcript-output-sharding-by-last-two-digits-of-video-id.md), artifacts are sharded by the last two digits of the video ID, producing directories like `00/`, `01/`, …, `99/`. Cite the code that computes the shard path. Redirect rationale to 0004.

**`### Artifact-before-mark_succeeded`** — re-emphasize the durability invariant in the output context: the writer completes (fsync if applicable) before `mark_succeeded` is called. Cite the code path that enforces this ordering. Redirect to [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md) for crash-recovery reasoning.

- [ ] **Step 11: Write ADRs section**

```markdown
## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0004 | Transcript output sharding | `src/output/` shard-path computation. |
| 0008 | Artifact-before-mark_succeeded | The durability ordering between output writer and state mutation. Cross-cuts state machine. |
| 0009 | whisper-rs version pin + fallback | The pinned crate version and fallback discipline. |
| 0010 | JSON artifact schema with raw signals pass-through | Artifact schema and the raw-signals passthrough. |
| 0012 | Cooperative cancellation via per-request Arc<AtomicBool> | Abort callback wiring. |
| 0013 | GPU verification at startup | Backend assert + device-name log. |
| 0014 | Audio input invariant: float32 PCM 16kHz mono via hound | Format invariant at the audio-prep boundary. |
| 0015 | Explicit non-use of `whisper_full_parallel` | Engine-state parallelism instead. |
| 0016 | Engine API stable across single- and multi-state | Engine-state concurrency model. |
```

- [ ] **Step 12: Verify, lint, commit**

```bash
grep -n "(TBD" docs/reference/architecture/transcription.md
wc -l docs/reference/architecture/transcription.md
```

Expected: no `(TBD)` remains; line count 190-260 (spec budget ~220).

```bash
grep -oP '\(\.\./\.\./decisions/\K[^)]+' docs/reference/architecture/transcription.md | sort -u | while read f; do
  test -f "docs/decisions/$f" && echo "OK ADR: $f" || echo "MISSING ADR: $f"
done
grep -oP '`src/\K[^`:]+' docs/reference/architecture/transcription.md | sort -u | while read f; do
  test -f "src/$f" && echo "OK src: $f" || echo "MISSING src: $f"
done
test -f docs/reference/whisper-cpp-deepdive.md && echo "OK deepdive" || echo "MISSING deepdive"
```

Expected: every line `OK`. Investigate any `MISSING`.

```bash
git add docs/reference/architecture/transcription.md
git commit -m "$(cat <<'EOF'
docs(reference): populate architecture/transcription.md

Audio prep (PCM invariant per 0014), whisper-rs integration depth
(version pin per 0009, params, cancellation per 0012, GPU verification
per 0013, engine-state model per 0015/0016, raw signals per 0010), and
output (artifact shape per 0010, sharding per 0004, durability per 0008).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

# uu-tiktok — transcription

The transcription subsystem covers three stages: preparing audio in the format whisper.cpp requires, running the transcription via the embedded `whisper-rs` library, and writing the resulting artifact to disk before the state machine acknowledges success.

## Audio preparation

Audio preparation is the boundary between fetched media and the whisper engine. It enforces the PCM 16 kHz mono float32 invariant ([ADR 0014](../../decisions/0014-audio-input-invariant-float32-pcm-16khz-mono-via-hound.md)) — whisper.cpp requires this exact format and we normalize at this single point. Implemented in `src/audio.rs`.

### PCM format invariant

`src/audio.rs` enforces the invariant by validation-and-rejection: it checks `spec.sample_rate == 16000` and `spec.channels == 1` immediately after opening the file (`src/audio.rs:51`), returning `AudioDecodeError::InvalidFormat` for any non-conforming input. A WAV with the right rate and channel count but non-16-bit integer samples (`bits_per_sample != 16`) also fails with `AudioDecodeError::UnsupportedSampleFormat` (`src/audio.rs:67`). An empty sample vector is caught after decoding (`src/audio.rs:92`), returning `AudioDecodeError::Empty`. No panics — every violation is a typed error variant that propagates to the caller. See [ADR 0014](../../decisions/0014-audio-input-invariant-float32-pcm-16khz-mono-via-hound.md) for the rationale.

### Source: the fetcher handoff

`src/audio.rs` accepts a path to a WAV file produced by the fetcher's ffmpeg post-processor. It does **not** resample, mix down channels, or convert sample rates — those operations happen at fetch time via yt-dlp's `--postprocessor-args` flags (see [data-input.md — Audio extraction handoff](data-input.md#audio-extraction-handoff)). By the time `decode_wav` is called, the file is already 16 kHz mono `pcm_s16le`.

The actual work `src/audio.rs` performs is:

1. **Open** the file via `hound::WavReader::open` (`src/audio.rs:45`).
2. **Validate** sample rate and channel count; reject with `InvalidFormat` if wrong (`src/audio.rs:51`).
3. **Decode** integer samples (`i16`) to float32 by dividing by `32768.0` (`src/audio.rs:74`): `i16::MIN` maps to exactly `-1.0`; `i16::MAX` maps to `~0.99997`. Float-format WAVs are passed through directly (`src/audio.rs:82`). The division constant and the choice between 32768 vs. 32767 are documented in the comment at `src/audio.rs:59`.
4. **Return** `Vec<f32>` in `[-1.0, 1.0]` — the format whisper.cpp's C API requires (see whisper-cpp-deepdive.md §2).

`hound` is the sole WAV I/O dependency (`Cargo.toml`: `hound = "3.5"`). No sample-rate conversion or channel mixdown library is used because the fetcher's ffmpeg post-processor already guarantees the correct format before `decode_wav` is called.

## Transcription

Transcription is a thin wrapper around `whisper-rs`, pinned to a specific upstream commit per [ADR 0009](../../decisions/0009-use-whisper-rs-for-whisper-cpp-embedding-with-version-pin-and-fallback-policy.md). The wrapper configures the context and per-inference parameters, wires a cancellation callback for cooperative shutdown ([ADR 0012](../../decisions/0012-cooperative-cancellation-via-per-request-arc-atomicbool-and-abort-callback.md)), declares the GPU backend intent at startup ([ADR 0013](../../decisions/0013-gpu-verification-at-startup-assert-backend-and-log-device-name.md)), and extracts the raw confidence signals whisper.cpp exposes ([ADR 0010](../../decisions/0010-json-artifact-raw-signals-schema-pass-through-with-schema-version.md)). For the upstream sampling and fallback loop these choices feed into, see [whisper-cpp-deepdive.md](../whisper-cpp-deepdive.md).

### whisper-rs integration

`whisper-rs` is pinned to an exact version: `=0.16.0` (`Cargo.toml`). A Cargo comment on the line above records the upstream correspondence:

```
# Tracks whisper.cpp v1.8.3 (commit 2eeeba56e9edd762b4b38467bab96c2517163158) via whisper-rs-sys 0.15.0.
```

The crate embeds whisper.cpp as a native library compiled from vendored C++ source — there is no separate subprocess, no server, and no IPC. The model is loaded in-process via `WhisperContext::new_with_params` (`src/transcribe.rs:406`), and inference runs against a `WhisperState` we own and reuse inside the worker thread. The fallback policy (when to unpin, when to patch) is specified in [ADR 0009](../../decisions/0009-use-whisper-rs-for-whisper-cpp-embedding-with-version-pin-and-fallback-policy.md).

### Context and full-params configuration

**`WhisperContextParameters`** (set once at worker-thread startup, `src/transcribe.rs:398–402`):

- `use_gpu(true)` — enables GPU acceleration; whisper.cpp falls back to CPU silently on a non-CUDA build. We declare GPU intent here; the backend-mismatch assertion (ADR 0013) is scaffolded (`WhisperInitError::BackendMismatch` exists at `src/transcribe.rs:303`) but not yet wired — see [GPU verification](#gpu-verification) below.
- `flash_attn(flash_attn)` — passed from `EngineConfig::flash_attn` (`src/transcribe.rs:401`). For the CUDA/Ampere case this enables FlashAttention kernels; see whisper-cpp-deepdive.md §3 and §10.
- `gpu_device(gpu_device)` — device index from `EngineConfig::gpu_device` (`src/transcribe.rs:402`); currently hardcoded to `0` in `src/main.rs:95`.

**`FullParams`** (built fresh per request, `src/transcribe.rs:533–543`):

- `SamplingStrategy::Greedy { best_of: 1 }` (`src/transcribe.rs:533`) — memory-conservative choice; the comment notes that `best_of=5` (Plan A's setting) takes ~7× the KV memory and may be worth revisiting after bake numbers confirm headroom on A10. Tracked in FOLLOWUPS.
- `set_print_progress(false)`, `set_print_realtime(false)`, `set_print_special(false)`, `set_print_timestamps(false)` (`src/transcribe.rs:534–537`) — suppress all console output; the pipeline captures results programmatically, not via printed output.
- `set_language(Some(lang))` (`src/transcribe.rs:543`) — pins to a BCP-47 code when `PerCallConfig::language` is `Some(...)`, or passes `"auto"` for auto-detection.

All other `FullParams` fields rely on upstream defaults. Notably, `no_context = true` is the whisper.cpp default, clearing prompt history at each `whisper_full` call so inference state does not carry over between videos. For the full option matrix, see whisper-cpp-deepdive.md §7.

### Cancellation wiring

Cancellation is cooperative and operates at two levels per [ADR 0012](../../decisions/0012-cooperative-cancellation-via-per-request-arc-atomicbool-and-abort-callback.md):

**Per-request `Arc<AtomicBool>`** — each `TranscribeRequest` carries its own cancel flag and deadline (`src/transcribe.rs:285–288`). The abort callback registered on `FullParams` polls both inside whisper.cpp's encoder/decoder loop (`src/transcribe.rs:574–581`). The callback fires `true` (abort) when `Instant::now() >= deadline` or `cancel.load() == true`. A separate `abort_fired: Arc<AtomicBool>` records whether the callback actually returned `true` during inference, so a post-deadline error from whisper isn't misattributed to cancellation (`src/transcribe.rs:570, 687`).

**Future-drop guard (`CancelOnDrop`)** — a `CancelOnDrop` struct wraps the cancel flag and sets it to `true` on `Drop` (`src/transcribe.rs:328–334`). When the orchestrator drops a `transcribe()` future (e.g., when `token.cancelled()` fires in a `tokio::select!` arm — `src/pipeline/pipelined.rs:344`), the guard fires immediately, signalling the abort callback at its next poll point inside whisper.cpp.

The abort callback is registered via the raw unsafe `set_abort_callback` / `set_abort_callback_user_data` pair (`src/transcribe.rs:583–587`), rather than the `set_abort_callback_safe` wrapper. A type-mismatch bug in whisper-rs 0.16.0's safe wrapper (`whisper_params.rs:645`) causes spurious early aborts; the workaround is documented at `src/transcribe.rs:550–569`. The closure box is reclaimed after `state.full` returns (`src/transcribe.rs:681`) to avoid leaking memory per request.

Cancellation is cooperative — whisper.cpp polls the abort callback at well-defined points in the encoder/decoder loop, not continuously. Ungraceful termination requires process kill. See whisper-cpp-deepdive.md §2 for the upstream polling points.

### GPU verification

The `WhisperInitError::BackendMismatch` error variant is scaffolded in `src/transcribe.rs:303` with the intent that startup would assert the GPU backend is active and exit rather than silently falling back to CPU (which would slow inference by ~50×, per [ADR 0013](../../decisions/0013-gpu-verification-at-startup-assert-backend-and-log-device-name.md)). The comment at `src/transcribe.rs:397` notes "0013 backend-mismatch assertion lands in T13" — this assertion is not yet wired. The current code logs the *configured* `gpu_device` and `flash_attn` values at model-load time (`src/transcribe.rs:409–414`) but does not query the active backend or device name. ADR 0013 documents the intended behavior; [ADR 0002](../../decisions/0002-dead-code-suppression-strategy-and-deferred-binary-library-restructuring.md) covers the `#[allow(dead_code)]` on the variant while the wiring is deferred.

### Engine-state model

A `WhisperContext` holds the model weights and vocabulary. It is loaded once at worker-thread startup via `WhisperContext::new_with_params` (`src/transcribe.rs:406`) and stays in memory for the worker's lifetime. It is read-only after load and never escapes the worker thread per [ADR 0016](../../decisions/0016-architecture-for-parallelism-engine-api-stable-across-single-and-multi-state.md).

Per-inference scratch (KV caches, decoder state, mel buffer) lives in `WhisperState`. The worker allocates two states:

1. **Primary `state`** — created once at init (`src/transcribe.rs:440`), reused for every inference call. `whisper_full_with_state` clears `result_all` on entry, so reuse across requests is safe.
2. **Secondary `lang_state`** — lazily allocated on the first `compute_lang_probs=true` request (`src/transcribe.rs:485–508`), then reused. It exists specifically for the `pcm_to_mel` + `lang_detect` pass, which must run on a separate state to avoid clobbering the primary state's logits.

Both states live exclusively inside the worker thread. The `lang_state_allocations` counter (`Arc<AtomicUsize>`) is the only piece of the lazy lifecycle exposed outside the thread — and only in `test-helpers` builds (`src/transcribe.rs:784–789`).

The orchestrator topology currently runs **one** transcribe worker thread (per [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md)), so one set of these states is active at a time. The engine API is designed to be stable across single-state and multi-state pools per [ADR 0016](../../decisions/0016-architecture-for-parallelism-engine-api-stable-across-single-and-multi-state.md) — a future multi-state configuration would allocate N primary states without changing the API surface. We explicitly do not use `whisper_full_parallel` per [ADR 0015](../../decisions/0015-explicit-non-use-of-whisper-full-parallel.md); parallelism is driven by engine states at the orchestrator layer, not by the upstream parallel function.

For the memory cost per state and the CUDA stream model, see whisper-cpp-deepdive.md §2 and §10.

### Raw signals extraction

After each successful `state.full(params, &samples)` call, we extract three kinds of signals per [ADR 0010](../../decisions/0010-json-artifact-raw-signals-schema-pass-through-with-schema-version.md):

- **`no_speech_prob` (per segment)** — `seg.no_speech_probability()` via `WhisperSegment` (`src/transcribe.rs:108`). Validated finite and in `[0.0, 1.0]`; out-of-range values surface as `TranscribeError::Bug`. For the upstream extraction point inside whisper.cpp, see whisper-cpp-deepdive.md §6.
- **Token `p` and `plog` (per token per segment)** — `tok.token_data().p` and `.plog` (`src/transcribe.rs:130–144`), validated for finiteness and plausible range.
- **Token `id` and `text` (per token per segment)** — `tok.token_id()` and `tok.to_str_lossy()` (`src/transcribe.rs:148–154`). Token identity is preserved so downstream consumers can filter special tokens (`[BEG]`, `[END]`, `<|en|>`, etc.).
- **`lang_probs` (per request, opt-in)** — populated by `lang_state.lang_detect()` when `PerCallConfig::compute_lang_probs` is `true` (`src/transcribe.rs:619–656`). The result is a probability vector paired with BCP-47 codes and sorted descending. A `pcm_to_mel` or `lang_detect` failure emits a warning and yields `lang_probs: None` rather than failing the inference — the transcript text is the contractual value; `lang_probs` is a research signal.

The `extract_segments` function collects these into `Vec<SegmentRaw>` (`src/transcribe.rs:91`). The output type `TranscribeOutput` (`src/transcribe.rs:27`) crosses the worker-thread boundary as owned data (no whisper-rs handles). The artifact schema and the `schema_version` field are specified in [ADR 0010](../../decisions/0010-json-artifact-raw-signals-schema-pass-through-with-schema-version.md); this section names the signals and their code locations only.

## Output

The output writer turns a completed transcription into a JSON artifact on disk. The artifact is written atomically and *before* the state row is flipped to succeeded (per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md)) so a crash mid-write leaves the row in `in_progress` for retry rather than orphaning the artifact.

### Artifact shape

Each video produces two files: a plain-text transcript (`{video_id}.txt`) and a JSON metadata artifact (`{video_id}.json`). The JSON top level is `TranscriptMetadata` (`src/output/artifacts.rs:47`), which carries provenance fields (`video_id`, `source_url`, `fetcher`, `transcript_source`, `model`, `transcribed_at`, `duration_s`, `language_detected`) plus an optional `raw_signals` sub-object (`src/output/artifacts.rs:61`). The `raw_signals` object carries a `schema_version` string (currently `"1"`, a string not an integer per ADR 0010's additive-revision rationale), `language`, `lang_probs` (always present as `null` when not opted-in — the field is never omitted), and `segments` (array of per-segment signals with per-token `id`, `text`, `p`, `plog`). The schema is specified fully in [ADR 0010](../../decisions/0010-json-artifact-raw-signals-schema-pass-through-with-schema-version.md); `src/output/artifacts.rs:100` is the projection (`RawSignals::from_transcribe_output`) that maps domain types to the artifact shape.

The JSON is serialized in compact form (`serde_json::to_vec`, not `to_vec_pretty`) to reduce on-disk size of the raw-signals payload.

### Sharding

Artifacts are stored under a two-level directory hierarchy: `{transcripts_root}/{shard}/{video_id}.{ext}`. The shard is the last two characters of the video ID (`src/output/mod.rs:9`), computed by `output::shard()`. TikTok video IDs are Snowflake-style integers whose low digits are essentially random, giving a uniform distribution across 100 buckets (`00/` through `99/`). The shard directory is created on demand by `write_artifacts_and_mark` before each write (`src/pipeline/mod.rs:299`). Redirect to [ADR 0004](../../decisions/0004-transcript-output-sharding-by-last-two-digits-of-video-id.md) for the rationale.

### Artifact-before-mark_succeeded

The write ordering is enforced in `write_artifacts_and_mark` (`src/pipeline/mod.rs:283`): the `.txt` artifact is written first, the `.json` artifact second, both via `artifacts::atomic_write` (`src/pipeline/mod.rs:308, 329`), and only then is `store.mark_succeeded(...)` called (`src/pipeline/mod.rs:332`). `atomic_write` writes to a `.tmp` file, calls `fsync`, renames to the final path, then fsyncs the parent directory (`src/output/artifacts.rs:128–159`), ensuring the artifact is durable before the rename completes. A crash between the two artifact writes (txt written, json absent) leaves the row in `in_progress`; a crash between json write and `mark_succeeded` also leaves it in `in_progress`. Either case is safe: `sweep_stale_claims` at next startup reclaims the row and re-drives the transcribe path; `atomic_write` is idempotent across retries. See [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md) for crash-recovery reasoning.

## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0004 | Transcript output sharding | `src/output/` shard-path computation. |
| 0008 | Artifact-before-mark_succeeded | The durability ordering between output writer and state mutation. Cross-cuts state machine. |
| 0009 | whisper-rs version pin + fallback | The pinned crate version and fallback discipline. |
| 0010 | JSON artifact schema with raw signals pass-through | Artifact schema and the raw-signals passthrough. |
| 0012 | Cooperative cancellation via per-request Arc\<AtomicBool\> | Abort callback wiring. |
| 0013 | GPU verification at startup | Backend assert + device-name log (scaffolded; assertion deferred). |
| 0014 | Audio input invariant: float32 PCM 16kHz mono via hound | Format invariant at the audio-prep boundary. |
| 0015 | Explicit non-use of `whisper_full_parallel` | Engine-state parallelism instead. |
| 0016 | Engine API stable across single- and multi-state | Engine-state concurrency model. |

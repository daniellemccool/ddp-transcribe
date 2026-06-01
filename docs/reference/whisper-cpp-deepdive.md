# whisper.cpp deepdive

Consolidated mental model of the whisper.cpp codebase. All citations are file:line.

## 1. Layout

- **`include/whisper.h`** (751 lines): the entire public C API. `extern "C"`, links via `libwhisper.so`. C++11 minimum.
- **`src/whisper.cpp`** (9003 lines): one giant translation unit holding all of it — model loader, encoder, decoder, sampling, fallback, VAD integration, DTW timestamps, grammars, mel spectrogram. There's no internal header beyond `src/whisper-arch.h` (tensor name maps).
- **`src/coreml/`, `src/openvino/`**: optional alternate encoder backends (Apple ANE, Intel OpenVINO).
- **`ggml/`**: bundled vendored math/backend library. `ggml/src/ggml-cuda/` is the CUDA kernels (`acc, argmax, argsort, conv2d, convert, ...` — full kernel zoo). Other backends: `ggml-metal`, `ggml-vulkan`, `ggml-hip`, `ggml-sycl`, `ggml-cann`, `ggml-musa`, `ggml-opencl`, `ggml-rpc`, `ggml-blas`, `ggml-cpu`.
- **`examples/`**: `cli`, `server`, `bench`, `vad-speech-segments`, `stream`, `command`, `talk-llama`, `quantize`, `lsp`, `wchess`, `whisper.android/objc/swiftui`, `whisper.wasm`, `addon.node`. The `cli` and `server` are the production-quality ones.
- **`bindings/`**: in-tree `go`, `java`, `javascript` (WASM), `ruby` (most actively touched per latest commit). Rust is **not** in-tree — `tazz4843/whisper-rs` is the community binding the README points to.
- **`models/`**: Python conversion scripts (`convert-pt-to-ggml.py`, `convert-h5-to-ggml.py`, `convert-silero-vad-to-ggml.py`, `convert-whisper-to-coreml.py`, `convert-whisper-to-openvino.py`), download scripts, plus zero-weight stub models for CI.

## 2. Public API contract

**Constants**: 16 kHz mono float32 audio. `WHISPER_SAMPLE_RATE=16000`, `WHISPER_N_FFT=400`, `WHISPER_HOP_LENGTH=160`, `WHISPER_CHUNK_SIZE=30`. The encoder always processes 30 s windows.

**Two-level handle model**:
- `whisper_context` = model weights + vocab + a default `whisper_state`. Read-only after load.
- `whisper_state` = per-inference scratch: KV self/cross/pad caches, mel buffer, batch, **N decoders** (`WHISPER_MAX_DECODERS`), 4 graph schedulers (conv/encode/cross/decode), encoder output cache, prompt history, no-speech prob, VAD state, DTW alignment buffers, and **its own `std::vector<ggml_backend_t>`**. Defined `whisper.cpp:834-935`.

**Concurrency contract**: API doc at `whisper.h:45-47`. The library is thread-safe iff you don't share a `whisper_state` across threads. Pattern for parallel inference on one model:
1. `whisper_init_from_file_with_params_no_state(path, cparams)` → context (no default state)
2. `whisper_init_state(ctx)` × N → N independent states; each allocates its own backends + KV caches
3. Run `whisper_full_with_state(ctx, states[i], …)` from N threads concurrently

Each extra state on a CUDA backend allocates its own GPU memory (KV self/cross/pad + compute buffers ≈ several hundred MB to ~1 GB depending on model). One physical GPU, multiple states = multiple CUDA streams sharing the device.

**Lifecycle functions**: `whisper_init_from_file_with_params`, `…_from_buffer_with_params`, `…_with_params` (custom loader callback). Each has a `_no_state` variant. Plus `whisper_init_state`, `whisper_free`, `whisper_free_state`, `whisper_free_params`, `whisper_free_context_params`.

**The pipeline** is `whisper_full(ctx, params, samples, n_samples)` (`whisper.cpp:7743`). Internally:
1. If `params.vad`, run `whisper_vad()` first (shrinks samples, builds time-mapping table)
2. Compute mel via `whisper_pcm_to_mel_with_state` (CPU, parallel)
3. If language unset/`"auto"`, call `whisper_lang_auto_detect_with_state` → fills `lang_probs[]`, sets `state->lang_id`
4. Outer loop in 30 s windows (`whisper_full_with_state`, `whisper.cpp:6792-7741`):
   - `whisper_encode_internal`
   - First decode pass with the prompt → captures `no_speech_prob` from the SOT logits *before* any logit filtering (`whisper.cpp:7158-7161`)
   - Inner sampling loop, up to `n_text_ctx/2 - 4` iterations, with `n_decoders_cur` parallel decoders (`whisper.cpp:7184`). Sampling and logit-processing are parallelized across decoders via `std::thread`
   - Score sequences, reject low-entropy ones (`entropy_thold`, repetition guard at `whisper.cpp:7527`)
   - **Temperature fallback** (`whisper.cpp:7548-7571`): if `failed || (avg_logprobs < logprob_thold && no_speech_prob < no_speech_thold)`, retry at next `temperature` from the schedule `[t0, t0+inc, t0+2·inc, … < 1.0]`
   - On success, walk tokens, split on timestamp tokens, emit `whisper_segment` records into `result_all`, fire `new_segment_callback`
5. Output via `whisper_full_get_segment_*` and `whisper_full_get_token_*` getters

**Callbacks** (set on `whisper_full_params`):
- `new_segment_callback(ctx, state, n_new, ud)` — fires per emitted segment (NOT in DTW mode where it fires per chunk)
- `progress_callback(ctx, state, %, ud)` — at start of each 30 s window
- `encoder_begin_callback(ctx, state, ud) → bool` — return false to abort before encoder runs
- `abort_callback(ud) → bool` (ggml-level) — checked frequently during graph compute
- `logits_filter_callback(ctx, state, tokens, n, logits, ud)` — modify logits after temperature scaling, before final logit suppression rules

**Logits processing** (`whisper.cpp:6164-6432`, `whisper_process_logits`) applies, in order: temperature scaling → suppress_blank (initial only) → suppress `<|notimestamps|>` and lang/task tokens → user `logits_filter_callback` → `suppress_regex` → `suppress_nst` → timestamp pairing constraints → `max_initial_ts` → monotonic timestamps → softmax → timestamp-vs-text decision (logsumexp comparison) → grammar suppression. This mirrors OpenAI `whisper/decoding.py`.

**Defaults that matter** (`whisper_full_default_params`, `whisper.cpp:5915-6021`):
- `language = "en"` — change to `"auto"` for multilingual
- `n_threads = min(4, hw_concurrency)`
- `no_context = true` — clears prompt history at start of each `whisper_full` call
- `temperature = 0`, `temperature_inc = 0.2`, `entropy_thold = 2.4`, `logprob_thold = -1.0`, `no_speech_thold = 0.6` — same as OpenAI Whisper
- `greedy.best_of = 5`, `beam_search.beam_size = 5` (when chosen)
- `print_progress = true` — set false when embedding
- `suppress_blank = true`, `suppress_nst = false`
- `vad = false`, `vad_params = whisper_vad_default_params()`

**Defaults for `whisper_context_params`** (`whisper.cpp:3606-3622`): `use_gpu = true`, `flash_attn = true`, `gpu_device = 0`. So GPU+FA out of the box.

## 3. Build system

- Root `CMakeLists.txt` is 258 lines. Whisper-level toggles: `BUILD_SHARED_LIBS`, `WHISPER_BUILD_TESTS/EXAMPLES/SERVER`, `WHISPER_USE_SYSTEM_GGML`, `WHISPER_CURL`, `WHISPER_SDL2`, `WHISPER_FFMPEG` (Linux only), `WHISPER_COREML`, `WHISPER_COREML_ALLOW_FALLBACK`, `WHISPER_OPENVINO`, sanitize knobs, `WHISPER_FATAL_WARNINGS`.
- All compute backends are GGML-level toggles, surfaced by the parent project. Old `WHISPER_CUDA`/`WHISPER_METAL`/etc. names auto-translate with deprecation warnings (`CMakeLists.txt:113-123`).
- **For NVIDIA**: `cmake -B build -DGGML_CUDA=1 -DCMAKE_CUDA_ARCHITECTURES=86` (A10/A100 = 86 Ampere). Sub-options under `ggml-cuda/CMakeLists.txt`: `GGML_CUDA_FA` (default ON, compiles FlashAttention kernels), `GGML_CUDA_FA_ALL_QUANTS`, `GGML_CUDA_FORCE_MMQ` / `_FORCE_CUBLAS`, `GGML_CUDA_NO_VMM`, `GGML_CUDA_NO_PEER_COPY`, `GGML_CUDA_GRAPHS` (CUDA graph capture — bench warmup loop at `bench.cpp:94` is required because of this), `GGML_CUDA_NCCL` (multi-GPU comm).
- Library output is `libwhisper.so` (or `.a`) + `libggml*.so` deps. Public header at `include/whisper.h`. CMake package (`whisper-config.cmake`) and pkg-config (`whisper.pc`) files installed.

## 4. Front-ends I read end-to-end

**`whisper-cli`** (`examples/cli/cli.cpp`, 1314 lines). Loads model **once** (`cli.cpp:1039`), iterates `params.fname_inp` (`cli.cpp:1072`), frees once (`cli.cpp:1311`). All flag categories:
- Decoding: `-bo/-bs/-tp/-tpi/-et/-lpt/-nth/-nf/-mc`
- Output: `-otxt/-ovtt/-osrt/-olrc/-ocsv/-oj/-ojf/-of/-owts`
- Display: `-pc/--print-confidence/-ps/-pp/-np`
- GPU: `-ng/-dev/-fa/-nfa`
- Audio: `-l/-dl/-ac` (audio_ctx override), `-d/-ot/-on` (offset/duration)
- Prompt: `--prompt`, `--carry-initial-prompt`
- VAD: `--vad/-vm/-vt/-vspd/-vsd/-vmsd/-vp/-vo`
- Other: `-tdrz/-di/-tr/-dtw/-ml/-sow/-ls/-sns/--suppress-regex/--grammar`

The `--output-json-full` schema (`cli.cpp:611-769`): `systeminfo`, `model{type,multilingual,vocab,audio,text,mels,ftype}`, `params`, `result.language`, `transcription[{timestamps,offsets,text,tokens[{text,id,p,t_dtw}]}]`. **It does not emit `no_speech_prob` or `avg_logprob`** — those only come through the C API or the server's `verbose_json`.

**`whisper-server`** (`examples/server/server.cpp`, 1262 lines). cpp-httplib + nlohmann/json. Endpoints:
- `POST /inference` — multipart form with `file` plus all wparam fields
- `POST /load` — hot-swap model
- `GET /health` — `{"status":"ok"}` 200 or `{"status":"loading model"}` 503
- `OPTIONS /inference` — CORS preflight

**Concurrency**: a single `std::mutex whisper_mutex` (`server.cpp:627, 807`) wraps every inference request. One inference at a time per server. Multi-GPU = multiple server processes on different ports. The server *does* accept `n_processors` and routes to `whisper_full_parallel` (`server.cpp:978`) — but that splits one audio across N states, with quality degradation at boundaries.

**Response formats** (`response_format` field): `json` (CLI-shaped), `text`, `srt`, `vtt`, `verbose_json`. The `verbose_json` form is OpenAI-compatible and is the only output that exposes `no_speech_prob` per segment, plus optional full language-probability distribution (`server.cpp:1054-1067, 1106`). Set `no_language_probabilities=false` to compute it (one extra encode+decode).

**HTTP abort**: server registers `wparams.abort_callback` to detect client disconnect (`server.cpp:971-976`); inference is cancellable.

**`whisper-bench`** (`examples/bench/bench.cpp`): microbenchmark of raw encoder + 3 decoder modes (single-token gen ×256, batched-5 ×64, prompt 256-tokens ×16). Doubles the workload as warmup (CUDA graph capture). Reports `Enc.`/`Dec.`/`Bch5`/`PP` as ms-per-call. Important: this is encoder/decoder primitives only — no mel, no sampling, no fallback. Real-world transcription is slower than these numbers suggest.

**`vad-speech-segments`** (`examples/vad-speech-segments/speech.cpp`): standalone Silero-VAD runner. Same VAD param surface as `whisper-cli`; outputs detected speech ranges.

## 5. VAD subsystem

Silero VAD is a separate small model (`ggml-silero-v6.2.0.bin`, ~865 KB). API (`whisper.h:678-729`):
- `whisper_vad_init_from_file_with_params(path, ctx_params)` → standalone `whisper_vad_context`
- `whisper_vad_segments_from_samples(vctx, params, samples, n)` → `whisper_vad_segments` (start/end pairs)
- Streaming: `whisper_vad_detect_speech` / `_no_reset` / `whisper_vad_reset_state`
- Free: `whisper_vad_free_segments`, `whisper_vad_free`

**`whisper_vad_params`** (`whisper.h:192-199`): `threshold`, `min_speech_duration_ms`, `min_silence_duration_ms`, `max_speech_duration_s`, `speech_pad_ms`, `samples_overlap`. Defaults: 0.5 / 250 ms / 100 ms / FLT_MAX / 30 ms / 0.1 s.

**Inline integration** when `params.vad = true` (`whisper.cpp:6630-6790`, `whisper_vad`):
- Lazily creates VAD context the first time, reuses across calls (lives on `state->vad_context`)
- Calls `whisper_vad_segments_from_samples`, builds a NEW audio buffer that concatenates speech segments separated by 0.1 s of zero-silence padding
- Builds `state->vad_mapping_table` of `(processed_time, original_time)` pairs
- Whisper runs over the trimmed audio; output timestamps are remapped via `map_processed_to_original_time` (`whisper.cpp:7912`)
- Logs reduction ratio (`Reduced audio from N to M samples (X% reduction)`)

VAD context exposes its own `whisper_vad_context_params { n_threads, use_gpu, gpu_device }` (`whisper.h:682-688`) — VAD can run on GPU separately from whisper.

## 6. Confidence / uncertainty signals

Everything you can extract per inference:

| Signal | Source | Granularity |
|---|---|---|
| `whisper_token_data.p` | `whisper_full_get_token_data` / `_get_token_p` | Per token |
| `whisper_token_data.plog` | same | Per token (log-prob) |
| `whisper_token_data.pt`, `.ptsum` | same | Per timestamp token |
| `no_speech_prob` | `whisper_full_get_segment_no_speech_prob` (`whisper.h:745`) | Per segment |
| Detected language | `whisper_full_lang_id(ctx)` | Per inference |
| Language probabilities | `whisper_lang_auto_detect`'s `lang_probs[]` buffer (size = `whisper_lang_max_id()+1`) | Per inference (re-encodes!) |
| `avg_logprob` | sum of `token.plog` / `n_tokens` over a segment | Per segment (you compute) |
| Compression ratio | not exposed; compute from segment text | Per segment (you compute) |
| Fallback failure counters | `state` fields `n_fail_p`, `n_fail_h` (`whisper.cpp:847-848`); shown in `whisper_print_timings` | Per inference (cumulative) |
| Per-decoder score | `decoder.sequence.score`, `.entropy`, `.avg_logprobs`; only logged at debug | Internal; surface via `-ls` CLI |

The **server's verbose_json** already aggregates `avg_logprob` and `no_speech_prob` per segment (`server.cpp:1102, 1106`); the **CLI's JSON-full** does not. The Python OpenAI Whisper triple of (no_speech_prob, avg_logprob, compression_ratio) maps cleanly except compression_ratio, which you'd compute yourself in a few lines.

## 7. Sampling / quality controls

**Strategy** (`whisper.h:455-458`):
- `WHISPER_SAMPLING_GREEDY` — uses `params.greedy.best_of` (default 5). At T=0, single greedy decode; at T>0 (fallback), `best_of` independent decodes.
- `WHISPER_SAMPLING_BEAM_SEARCH` — uses `params.beam_search.beam_size` (default 5) at T=0, switches to `best_of` at T>0. Patience param exists in struct but TODO-not-implemented.

**Decoders count** (`whisper.cpp:6862-6881`): `n_decoders = max(1, max(best_of, beam_size))`, capped at `WHISPER_MAX_DECODERS`. Memory cost grows with decoders because the KV self-cache is over-allocated by `n_decoders + 2` factor (`whisper.cpp:7128`).

**Fallback ladder**: temperatures `[temperature, temperature+inc, …, <1.0]`. Triggered by failed entropy check or `avg_logprobs < logprob_thold AND no_speech_prob < no_speech_thold`. `--no-fallback` (or `temperature_inc=0`) disables the ladder.

**Quality cliff**: `params.no_context = true` (default) keeps each window independent. Setting it to false enables prompt-history conditioning across windows but risks cascading hallucinations. The internal `WHISPER_HISTORY_CONDITIONING_TEMP_CUTOFF = 0.5f` (`whisper.cpp:145, 7090`) drops prompt history at high fallback temperatures.

**`audio_ctx`** (`-ac`) shrinks the encoder context — useful for short audio (< 30 s) to skip wasted encoder work. Speed-up but accuracy hit.

## 8. Models

Pre-built `ggml` models hosted at `huggingface.co/ggerganov/whisper.cpp`. Variants in `models/README.md`:

| Variant | Disk | RAM (typical) | Notes |
|---|---|---|---|
| `tiny` / `tiny.en` | 75 MB | ~273 MB | English-only `.en` is faster/cleaner for English |
| `base` / `base.en` | 142 MB | ~388 MB | |
| `small` / `small.en` / `small.en-tdrz` | 466 MB | ~852 MB | tdrz = tinydiarize (speaker turn) |
| `medium` / `medium.en` | 1.5 GB | ~2.1 GB | |
| `large-v1`, `-v2`, `-v3` | 2.9 GB | ~3.9 GB | |
| `large-v2-q5_0`, `large-v3-q5_0` | 1.1 GB | ~1.5 GB | Q5_0 quantized |
| `large-v3-turbo` | 1.5 GB | — | Faster decoder (4-layer instead of 32) |
| `large-v3-turbo-q5_0` | **547 MB** | ~750 MB | Sweet spot for multilingual + speed |

**Quantization**: bring the binary, run `./build/bin/quantize ggml-X.bin ggml-X-q5_0.bin q5_0`. Whisper.cpp supports `Q4_0/1, Q5_0/1, Q8_0` and the rest of ggml's quant types. CUDA backend supports them (CMake option `GGML_CUDA_FORCE_MMQ` controls whether MMQ kernels or cuBLAS handle them).

**Distilled models** (`distil-large-v2` etc.): chunk-based transcription not implemented; documented as sub-optimal.

**File format**: ggml binary, contains hparams, mel filters, vocab, weights. `whisper_model_load` (`whisper.cpp:1485`) reads in this order. Three loading paths: file path, in-memory buffer, custom `whisper_model_loader` (caller provides read/eof/close callbacks). Useful for embedded or signed-blob distribution.

## 9. Bindings landscape

In-tree:
- **Go** (`bindings/go/`): low-level CGO + high-level `pkg/whisper` API. Active.
- **Java** (`bindings/java/`): JNI.
- **JavaScript** (`bindings/javascript/`): WASM via emscripten, npm-published.
- **Ruby** (`bindings/ruby/`): full C extension (the recent commit at HEAD touched this). Has `Whisper::Context`, `::Params`, full VAD wrappers, segment/token models. Most comprehensive in-tree binding.

Out-of-tree:
- **Rust**: `tazz4843/whisper-rs` — referenced in main README at line 703 as the recommended Rust binding. Wraps the C API; tracks upstream actively.
- **Python**: three competing bindings (`whispercpp.py`, `pywhispercpp`, `whispercpp`). README points to all three.
- **.NET**, **Swift/ObjC**, **R**, **Unity**, **React Native** — all out-of-tree, listed in README.

## 10. Concurrency model — the actionable rules

1. **One context, many states** is the canonical pattern for in-process concurrency. Open with `_no_state`, then `whisper_init_state` per worker thread. Each state owns its own backends and KV caches.
2. **`whisper_full_parallel` is not a true concurrency tool** — it splits *one* audio across N states with documented quality loss at boundaries (`whisper.cpp:7891`). Useful only when the audio is much longer than one window and per-file latency matters. Don't use it for parallel transcription of independent files; spawn proper worker threads instead.
3. **GPU device selection** is per-context via `cparams.gpu_device` (an index into the GPU device list, `whisper.cpp:1304-1311`). To use 2 GPUs concurrently: open 2 contexts, one on `gpu_device=0` and one on `gpu_device=1`. Same model file, separately loaded in each.
4. **`flash_attn = true`** is the default and is supported by the CUDA backend on Ampere+ (`GGML_CUDA_FA = ON` by default). Keep it on.
5. **CUDA graphs** require a 2-loop warmup (see `examples/bench/bench.cpp:92-94` comment). The `whisper_full` path already reuses the captured graph after the first window, so this is mostly transparent — but it means the first 30 s of any inference is slower than steady-state.
6. **Inner threading** (`n_threads`) drives mel computation (`whisper.cpp:3212`), parallel sampling across decoders (`whisper.cpp:7242, 7494`), and CPU-backend ops via `ggml_backend_set_n_threads`. On a CUDA build it's mostly irrelevant for throughput.

## 11. Sharp edges I noted while reading

- **First-release distilled models force `no_timestamps=true`** with a warning — detected by `n_text_layer == 2 && n_vocab != 51866` (`whisper.cpp:6967-6974`).
- **Auto language detect inside `whisper_full`** reuses the existing mel and is cheap. Calling `whisper_lang_auto_detect` *before* `whisper_full` re-encodes the audio (`whisper.cpp:4040`) — wasteful. Just set `params.language = "auto"`.
- **`params.detect_language = true`** runs auto-detect and returns 0 immediately without transcribing (`whisper.cpp:6824-6826`). Good for routing.
- **`whisper_full_with_state` clears `result_all` on entry** (`whisper.cpp:6801`). State reuse across calls is safe; results from the previous call are gone.
- **Audio under 100 ms returns 0 with a warning** (`whisper.cpp:6846-6849`).
- **`prompt_past` clears at end-of-audio tail** (within last 5 s) to avoid the model trying to fit too little remaining audio into rolling context (`whisper.cpp:7027-7030`).
- **`whisper_kv_cache` over-allocates by `n_decoders + 2`** when n_decoders > 1 (`whisper.cpp:7128`) — fragmentation workaround. So beam_size=5 takes ~7× the KV memory of greedy.
- **The CLI's JSON output omits `no_speech_prob`**; only the server's `verbose_json` includes it. If you want it from CLI you patch the JSON writer (a few lines) or use the C API directly.
- **Compression ratio** (Whisper's classic third confidence indicator) is not computed by whisper.cpp — `entropy_thold` plays the same role internally (it's a cumulative entropy threshold over decoded tokens, used for fallback decisions). Marked TODO in server.cpp:1104.
- **`whisper_full(ctx, …)` operates on `ctx->state`**. If you opened with `_no_state`, you must use `whisper_full_with_state` — the no-state ctx has no default state to operate on.

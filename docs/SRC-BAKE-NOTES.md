# SRC A10 Bake Notes — Plan B Epic 1

**Date:** 2026-05-12
**Workspace:** `transcription.develop-data-do.src.surf-hosted.nl`
**Hardware:** 1× NVIDIA A10 (compute capability 8.6, VMM), driver 595.71.05
**OS:** Ubuntu 24.04.4 LTS, kernel 6.8.0-111-generic
**Operator:** Danielle McCool
**Branch:** `feat/plan-b-efficiency-first` @ `4171e00` (T12 follow-up) — 15 commits ahead of `main`
**Bake scope:** Pipeline-integration validation + per-model cold-start performance characterization. Full N=5 averaging not achieved due to yt-dlp format-selection issue (documented below — Epic 3 finding).

---

## Headline outcomes

1. **0013 gate passes on all four models.** No silent CPU fallback. `whisper_backend_init_gpu: using CUDA0 backend` confirmed in stderr for tiny.en, small, medium.en, and large-v3-turbo-q5_0. Binary linked against `libcudart.so.13`, `libcublas.so.13`, `libcuda.so.1`, `libcublasLt.so.13` from `/usr/local/cuda/targets/x86_64-linux/lib/`.
2. **CUDA 13.2 + whisper-rs 0.16.0 (whisper.cpp v1.8.3, commit `2eeeba5`) build cleanly.** 3 minutes 17 seconds release build with `--features cuda`. No flag adjustments, no patched headers, no fallback to older CUDA needed. This is a "tested-good" combination worth pinning in the playbook.
3. **End-to-end pipeline integration confirmed.** One video flowed through fetch → WAV decode → engine → text + JSON artifact with `raw_signals` (schema_version=`"1"`, per-segment `no_speech_prob`, per-token `{id, text, p, plog}`, language code, lang_probs slot) and provenance fields (`fetcher: "ytdlp"`, `transcript_source: "whisper-rs"`, model basename).
4. **Production model choice clarified by quality observation: `large-v3-turbo-q5_0` is the winner.** Better transcripts than `medium.en` (multilingual where medium.en hallucinates) and faster wall-clock at one-third the model size. See per-model table.
5. **Two material Plan B Epic 3 findings surfaced** during bake (yt-dlp format-selection + curl-cffi impersonation activation). Both have concrete fix paths documented in `docs/FOLLOWUPS.md`.

---

## Environment surprises (playbook material)

These caused friction during the bake and should be in the eventual operator playbook so future runs don't rediscover them.

1. **`SRC-BAKE-CHECKLIST.md` is Plan A; Phase 1 is stale.** Lists `whisper-cli` as required (Plan B removed it) and omits Plan B's actual build prereqs: `cmake`, `clang`, `libclang-dev` / `libclang-18-dev`, CUDA toolkit. Phase 5's `cargo build --release` should be `cargo build --release --features cuda` for A10 builds. Update the checklist (or supersede it) when the playbook lands.
2. **Ubuntu 24.04's `libclang-dev` package installs only C++ headers (`/usr/include/clang/`), NOT the C-API headers (`clang-c/Index.h`)** that bindgen needs. Need the versioned package `libclang-18-dev` explicitly. Headers land at `/usr/lib/llvm-18/include/clang-c/`, discovered by bindgen via `llvm-config-18`.
3. **`jq` is missing from the standard apt install set** but is needed for the 0017 done-contract check (inspect `.json` artifacts). Add to playbook's install list.
4. **`apt update` on the SRC base image emits ~60 W: warnings** about multi-configured sources (`fallback-mirrors.sources` + `ubuntu.sources` overlap). All cosmetic; "All packages are up to date" line at the top is the only success signal that matters.
5. **SSH from a modern terminal emulator (kitty, alacritty, wezterm) breaks ncurses tools** because the workspace doesn't ship those terminfo entries. `TERM=xterm-256color <cmd>` for one-offs or `export TERM=xterm-256color` to persist.
6. **`pipx inject yt-dlp curl-cffi` reports success but yt-dlp still shows `(unavailable)` for impersonation targets.** Diagnostic unfinished due to paste-friction; root cause likely a C-extension build / libcurl-dev gap. See FOLLOWUPS entry.

---

## Tools and versions

| Tool | Version |
|------|---------|
| Linux | 6.8.0-111-generic (Ubuntu 24.04.4 LTS) |
| NVIDIA driver | 595.71.05 |
| CUDA toolkit | release 13.2, V13.2.78 (built Mar 19 2026) |
| rustc / cargo | 1.95.0 (`59807616e` / `f2d3ce0bd`, 2026-04-14 / 2026-03-21) |
| cmake | 3.28.3 |
| clang / libclang | 18.1.3 (Ubuntu `1ubuntu1`) |
| yt-dlp | 2026.03.17 (via pipx) |
| ffmpeg | 6.1.1-3ubuntu5 |
| sqlite3 | 3.45.1 (2024-01-30) |
| whisper-rs crate | 0.16.0 (per `Cargo.toml`) |
| whisper.cpp (vendored via whisper-rs-sys) | v1.8.3 (commit `2eeeba56e9edd762b4b38467bab96c2517163158`) |

---

## GPU verification (0013 gate)

**Outcome: PASS** — backend init logs and `ldd` of the release binary both confirm GPU is active. No silent CPU fallback at any point.

```
whisper_backend_init_gpu: device 0: CUDA0 (type: 1)
whisper_backend_init_gpu: found GPU device 0: CUDA0 (type: 1, cnt: 0)
whisper_backend_init_gpu: using CUDA0 backend
```

Appears twice per inference (once for the primary state, once for the language-detection state per 0012's separate-state design from T8). Confirmed on every one of the four model runs.

`ldd target/release/uu-tiktok | grep -E "cuda|cublas"`:
```
libcublas.so.13 => /usr/local/cuda/targets/x86_64-linux/lib/libcublas.so.13
libcudart.so.13 => /usr/local/cuda/targets/x86_64-linux/lib/libcudart.so.13
libcuda.so.1 => /lib/x86_64-linux-gnu/libcuda.so.1
libcublasLt.so.13 => /usr/local/cuda/targets/x86_64-linux/lib/libcublasLt.so.13
```

`nvidia-smi` observation: GPU utilization spike during inference is sub-second on tiny.en's 35.6-second audio clip and difficult to capture with 0.5s `watch` polling. The CUDA backend init logs are the authoritative signal; nvidia-smi corroboration is best-effort.

---

## Per-model wallclock — n=1 each on video `7491984376423615766` (35.6440625s of French audio, "RTL FR" helicopter-crash news clip)

| Model | Size on disk | Model VRAM | State VRAM (×2 per 0012) | Total VRAM est | Transcribe wallclock† | Realtime ratio | `time -v` total | RSS peak |
|-------|--------------|------------|----------------------------|----------------|------------------------|-----------------|-----------------|----------|
| `tiny.en` | 75 MB | 77 MB | 148 MB | ~373 MB | **0.17 s** | **213× realtime** | 4.37 s | 493 MB |
| `small` | 466 MB | 487 MB | 242 MB | ~969 MB | **0.79 s** | **45× realtime** | 3.69 s | 514 MB |
| `medium.en` | 1.5 GB | 1533 MB | 389 MB | ~2310 MB | **1.17 s** | **30× realtime** | 4.45 s | 506 MB |
| **`large-v3-turbo-q5_0`** | 548 MB | 573 MB | 252 MB | ~1077 MB | **0.73 s** | **49× realtime** | 3.92 s | 503 MB |

† Transcribe wallclock = log-timestamp delta between `audio acquired` and `transcribed` log lines, isolating inference from fetch+decode. Note: cold-start (model load happens fresh each run because `process` exits after the single video). In a long-running daemon the per-call inference cost is what matters; the cold-start ~0.3-0.5s model-load tax amortizes across the batch.

**Quality observation (informative, not a perf metric):**

| Model | lang_detect | Probability | Transcript output |
|-------|-------------|-------------|--------------------|
| `tiny.en` | `hi` | **1.0%** (noise — English-only model has no real prior over non-English languages) | `[speaking Spanish]` placeholder — whisper.cpp's documented English-only fallback for non-English audio |
| `small` | `fr` | **98.6%** | Correct French transcript: "Voici ce que l'on sait sur les victimes du crash d'hélicoptère à New York..." |
| `medium.en` | `da` | **1.0%** (noise) | Hallucinated English "approximation" of French content — not a translation, not a transcript |
| `large-v3-turbo-q5_0` | `fr` | **99.9%** | Correct French transcript, marginally more polished phrasing than `small` |

The `lang_detect` probability (T8 on the separate `lang_state`) is a reliable confidence proxy: ≥98% → trust the language code and the transcript; ~1% → noise, English-only model encountered non-English audio.

**Production-model recommendation: `large-v3-turbo-q5_0`.** Best transcript quality (multilingual, accurate), competitive wall-clock (49× realtime), one-third the model size of `medium.en` (548 MB vs 1.5 GB), well under 2 GB total VRAM at single-state. Leaves headroom on the A10's 24 GB for multi-state parallelism if Plan C/production lights it up.

---

## --compute-lang-probs overhead

**Not measured in this bake.** Would require a separate run with `UU_TIKTOK_COMPUTE_LANG_PROBS=1`. Expected per spec/design: ~1.5–2× slower per video (one extra encoder pass over the lang_state). Carry into Epic 2's bake if compute-lang-probs is enabled in production.

---

## 1-state vs 2-state measurement

**Not measured in this bake** — fetch failures (Epic 3 finding below) prevented running the multi-instance test that would have produced 5 videos through two concurrent processes. The architectural future-proofing per 0016 is in place (Engine API is stable across single/multi-state); intra-process multi-state implementation is Plan C / production grant work regardless.

---

## Raw artifact size growth (per 0010 schema_version="1")

Single artifact for the 35.6-second French video:
- `.txt`: 730 bytes (transcript)
- `.json`: 2250 bytes (metadata + `raw_signals` with per-token `id`/`text`/`p`/`plog`)

**Per-token raw-signal growth roughly doubles JSON size** vs a hypothetical `{p, plog}`-only token shape. For 1M-video production at this artifact size: ~2.25 GB of metadata JSON. Acceptable at scale; gzip compression (Plan C consideration) could halve again. The `id` + `text` fields are load-bearing for downstream filtering of special tokens like `[_BEG_]`, `[_EOT_]`, `<|en|>` etc. (0010 pass-through rule).

---

## Cold-start vs steady-state

Not measured cleanly because each bake run invokes the binary fresh (one video then exit). Approximate per-run breakdown from log timestamps:

- Process startup + config + state DB init: ~0.2 s
- Ingest (already-ingested fixtures): ~0.05 s
- Model load (varies by size): ~0.3 s (tiny.en) to ~0.5 s (medium.en)
- Fetch + WAV decode: ~1.2-2.0 s (yt-dlp + ffmpeg, network-bound)
- Inference: see per-model table
- Artifact write + DB mark_succeeded + cleanup: ~0.2 s

In a long-running daemon (Plan B Epic 2 pipelined orchestrator), the model-load cost amortizes to zero across the batch; fetch becomes the dominant non-inference cost.

---

## Plan B Epic 3 findings surfaced during bake

Both findings are now well-supported with concrete fix paths; full FOLLOWUPS entries added in the same commit as these notes.

### Finding 1: yt-dlp's automatic format selection picks phantom video-only streams for some TikTok URLs

**Reproduction:** `--max-videos 5` on the `news_orgs` fixture failed at video #2 with `Postprocessing: WARNING: unable to obtain file audio codec with ffprobe`. yt-dlp downloaded `bytevc1_1080p_559269-1` — a format ID NOT present in `--list-formats` output. `ffprobe` confirmed the downloaded MP4 was HEVC video-only with no audio stream.

**Fix:** Pass explicit `-f "ba/b"` to yt-dlp invocation in `src/fetcher/ytdlp.rs`. Confirmed working: `-f "b"` downloaded the listed `bytevc1_1080p_583083-1` format with audio, ffmpeg extracted clean 4.4 MB WAV. Every listed format has `acodec=aac`; the issue is yt-dlp's `-x` auto-select walking off the listed format menu.

**Severity:** Material. The Escobar/French video succeeded (different TikTok account); the rtl.nl video failed reproducibly. Failure rate at scale is unknown until the patched fetcher is bake-tested. May be account-specific (newer accounts using DASH delivery).

### Finding 2: `pipx inject yt-dlp curl-cffi` silently fails to enable yt-dlp impersonation

**Reproduction:** `pipx inject yt-dlp curl-cffi 2>&1` reports `injected package curl-cffi into venv yt-dlp ✨ 🌟 ✨`. `yt-dlp --list-impersonate-targets` still shows all targets (`Chrome`, `Firefox`, `Edge`, `Safari`, `Tor`) as `(unavailable)`. Warning persists on every yt-dlp run: `The extractor is attempting impersonation, but no impersonate target is available`.

**Hypothesis:** curl-cffi's C extension may have built without proper libcurl linkage. `libcurl4-openssl-dev` may need to be apt-installed before curl-cffi for the C extension to find libcurl headers at build time. Diagnostic curtailed due to paste-friction during interactive bake.

**Production impact:** unknown. Initial assumption ("TikTok IP-blocks SURF datacenter") was withdrawn after the apparent block message turned out to be a URL typo. Whether impersonation is actually needed to fetch most TikTok content reliably from SURF workspaces is an open question — needs a focused mini-bake of N=20+ videos with and without working impersonation.

**2026-05-13 update — Finding 1 fix landed; Finding 2 resolved.** The yt-dlp format selector was switched to `download/b[vcodec=h264]/b` (commit `cb6f214`, PR #2); Finding 2's root cause turned out to be a curl_cffi version mismatch, not the libcurl-linkage hypothesis above. See `docs/FOLLOWUPS.md` for the corrected analysis and the operator runbook line (`pipx install 'yt-dlp[default,curl-cffi]'`). Both findings closed for the immediate bake retrospective; the residual Epic 3 retry work and the N=20+ impersonation-on-vs-off mini-bake remain open as separate concerns.

---

## Workspace metadata

The non-PII details below are reproducible by anyone provisioning an equivalent SRC workspace; recorded so the next bake operator can ground-truth their setup against this one.

- External storage volume: `~/data/transcription-pipeline-storage/` (separate from `~/data/datasets/`)
- Repo clone path during bake: `~/data/transcription-pipeline-storage/uu-tiktok/`
- Models downloaded to: `<repo>/models/` (75 MB + 466 MB + 1.5 GB + 548 MB = ~2.5 GB)
- State DB: `<repo>/state.sqlite` (local disk, WAL-capable)
- Transcripts: `<repo>/transcripts/<shard>/`

---

## What this bake did NOT do

For completeness and to clearly delegate to future epics:

- **N=5-video averaging per model.** Blocked by Epic 3 fetch-classification gap (no recovery from per-video fetch failure under Plan A's serial loop; Epic 2 introduces failure persistence).
- **--compute-lang-probs overhead.** Carry to Epic 2's bake addendum.
- **1-state vs 2-state throughput delta.** Multi-instance test deferred to Plan C / production grant work; architecture is future-proofed per 0016.
- **Long-running steady-state characterization.** Each bake run was single-shot; a 100-video continuous run would reveal CUDA-graph warmup effects, RSS growth/leak signatures, and amortized model-load cost. Becomes Epic 2 bake work once the pipelined orchestrator lands.
- **Donation-side TikTok URL reachability.** The single account-failure observed (`@rtl.nl`) reproduced consistently but no broader survey of donation-source URL fetchability was conducted.

---

## Operational habits documented (per 0011)

After capturing these notes, the workspace was paused per the 0011 spin-down practice. Grant-wallet bills $0 while paused; storage and workspace state persist for the next session.

Bake-completion checklist (informal — formalize in Epic 4's `status` subcommand per 0017):

- [x] `cargo build --release --features cuda` succeeded
- [x] CUDA backend log line confirmed on all measured models
- [x] At least one `.json` artifact inspected via `jq`; `raw_signals.schema_version == "1"` verified
- [x] Per-model wallclocks captured (n=1)
- [ ] N=5 per model (blocked by Epic 3 fetch-classification gap; deferred)
- [ ] --compute-lang-probs measurement (deferred)
- [ ] 1-state vs 2-state measurement (deferred to production grant)
- [x] Bake notes written + committed
- [x] Workspace paused

---

# SRC A10 Bake Notes — Plan B Epic 2

**Date:** 2026-05-20
**Workspace:** `transcription.develop-data-do.src.surf-hosted.nl`
**Hardware:** 1× NVIDIA A10 (same workspace as Epic 1)
**Operator:** Danielle McCool
**Branch:** `feat/plan-b-epic-2` @ `d928905` (Phase 2 + cleanup commits + bootstrap UX fixes)
**Build:** `cargo build --release --features cuda`
**Bake scope:** N=1/N=3/N=5 throughput comparison + coordinated-shutdown drill.

---

## Headline outcomes

1. **N=3 is the empirically validated default** per ADR 0027. N=5 adds essentially nothing (1.007× over N=3); the curve-flattening point is confirmed.
2. **1.44× absolute speedup at N=3 vs N=1** — below ADR 0027's ~3.5× prediction because this fixture's fetch is much faster than the modeled ratio (avg 1.5s/clip vs the 5.5s the prediction assumed). When fetch is fast, transcribe becomes the bottleneck sooner; the design ceiling depends on the avg_fetch / avg_transcribe ratio.
3. **User CPU constant at ~56s across all three topologies** — CPU work is fixed; wallclock varies via overlap. Design intent confirmed.
4. **Coordinated-shutdown drill validates ADR 0024 end-to-end.** Pre-kill `in_progress` count matched post-restart `recovered` count exactly; no half-written transcripts surfaced.
5. **Fixture:** `news_orgs` (20 pending videos after ingest). **Model:** `ggml-large-v3-turbo-q5_0.bin` (~573 MB, GPU/CUDA).

---

## Throughput comparison

| Topology | Wallclock | Per-clip | Speedup vs N=1 | User CPU | Sys |
|---|---|---|---|---|---|
| N=1 (serial) | 41.7s | 2.09s | 1.00× | 56.1s | 4.2s |
| **N=3 (default)** | **29.0s** | **1.45s** | **1.44×** | 56.3s | 4.1s |
| N=5 | 28.8s | 1.44s | 1.45× | 56.4s | 4.3s |

---

## Coordinated-shutdown drill

- N=3 default, started in background, `sleep 5`, `kill -KILL` the process tree.
- Pre-restart DB: 6 rows in `in_progress` with non-NULL `claimed_at` (5-second spread: timestamps 1779312832–1779312837).
- Restart with `--stale-claim-threshold 1s`: log line `sweep_stale_claims recovered=6 threshold_secs=1`. All 6 swept rows re-claimed and processed to `succeeded`.
- Final state: `claimed=20 succeeded=20 failed=0 stale_after_success=0 stale_after_failure=0`. No half-states.

---

## Findings

1. **N=3 is empirically validated as the right default** per ADR 0027. N=5 adds essentially zero (1.007× over N=3) — the curve-flattening point is confirmed.
2. **Absolute speedup (1.44×) is below ADR 0027's ~3.5× prediction** because this fixture's fetch is much faster than Epic 1's measurements (avg 1.5s/clip vs Epic 1's modeled 5.5s). When fetch is fast, transcribe becomes the bottleneck sooner — the design ceiling depends on the avg_fetch / avg_transcribe ratio. For fixtures with slower fetches (network-bound or pathological yt-dlp paths), the speedup would approach the predicted 3.5×.
3. **User CPU is constant at ~56s across all three topologies.** CPU work is fixed; wallclock varies via overlap. Design intent confirmed.
4. **All Phase 2 stale counters (`stale_after_success`, `stale_after_failure`) stayed at 0** across all happy-path runs. The symmetric race-detection design is in place but the happy path doesn't exercise it — expected.
5. **Coordinated-shutdown drill validates ADR 0024 (sweep semantics) end-to-end.** Pre-kill `in_progress` count matched post-restart `recovered` count exactly. No half-written transcripts surfaced because ADR 0008's artifact-before-`mark_succeeded` order prevented them.
6. **`attempt_count` accumulates across bake sweeps** (rows showed attempt=7-8 on the drill recovery). Expected behavior; the reset-between-runs SQL deliberately doesn't reset `attempt_count` per ADR 0024.

---

## Language detection (multilingual model exercised)

French ×1, Dutch ×8, English ×9, Tagalog ×2 — all detected with confidence > 0.987.

---

## What this bake did NOT do

- **Long-running steady-state characterization (>20 videos).** Each bake run was bounded to the 20-video `news_orgs` fixture.
- **Multi-fixture comparison.** e.g., longer videos to verify the 3.5× ceiling under network-bound fetch conditions.
- **Real-world concurrent claim contention.** The multi-process scenario `stale_after_failure` is designed to surface was not exercised.
- **`--compute-lang-probs` overhead measurement.** Carry to Epic 3 bake addendum (deferred since Epic 1).
- **ADR 0025's `engine.transcribe()` cancellation latency under in-flight load.** The T18 fixup at `a66d38b` wrapped the `select!` but cancel-mid-transcribe latency was not measured under load.

---

## Operational habits documented (per 0011)

Workspace paused per the 0011 spin-down practice after capturing these notes.

Bake-completion checklist (Epic 2 scope):

- [x] `cargo build --release --features cuda` succeeded
- [x] CUDA backend log line confirmed (inherited from Epic 1 baseline; `ggml-large-v3-turbo-q5_0.bin` confirmed GPU on A10)
- [x] N=1 / N=3 / N=5 throughput captured (20-video fixture)
- [x] Coordinated-shutdown drill completed (6-row recovery, final state clean)
- [x] Language detection confirmed on multilingual fixture (fr/nl/en/tl)
- [ ] --compute-lang-probs measurement (deferred to Epic 3)
- [ ] Multi-fixture speedup ceiling verification (deferred)

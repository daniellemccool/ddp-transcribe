---
status: accepted
date: "2026-05-12"
category: Audio
applies_to:
    - src/audio.rs
    - src/fetcher/ytdlp.rs
priority: invariant
---

# Validate 16k mono f32 at decode

## Decision

The transcription engine consumes exactly one audio format: float32 PCM in
`[-1.0, 1.0]`, 16 kHz, mono. `decode_wav` (`src/audio.rs`) validates the WAV
header on every load — `sample_rate == 16000`, `channels == 1`, f32 samples or
i16 converted — and rejects non-conforming input with a typed error.

## Guidance

- Every audio path into the engine goes through `decode_wav`; review rejects new fetch/decode paths that hand the engine samples that bypassed its header validation.
- The producing side is the yt-dlp ffmpeg postprocessor args (`-ar 16000 -ac 1 -c:a pcm_s16le`, `src/fetcher/ytdlp.rs`) — keep producer and validator in lockstep, and keep the "redundant" explicit flags as defence against upstream default changes.
- Decode to an owned `Vec<f32>` so samples cross the fetch→transcribe channel without borrowing; the i16 conversion is the `/32768.0` normalisation in `src/audio.rs`.

## Why

whisper.cpp's C API accepts any float buffer without complaint — wrong sample
rate or channel count doesn't error, it silently degrades transcription
quality across the whole batch, and fixture-based tests keep passing while it
does. The validator is the only thing standing between a producer-side drift
and a corrupted study corpus.

## Context

The pipeline embeds whisper.cpp in-process, so the WAV that yt-dlp's ffmpeg
postprocessor emits must be decoded to raw PCM in Rust. hound was chosen as
the decoder: small, PCM-WAV-focused, no general-codec weight, and the input
format is pinned by our own postprocessor flags so nothing broader is needed.

## Alternatives

- **symphonia** — general audio decoding (MP3/FLAC/Vorbis); dependency and compile-time weight for no benefit when upstream emits exactly one format.
- **Custom WAV parser** — error-prone; WAV corner cases (RIFX byte order, non-PCM subchunks, extension chunks) are not something a one-developer project should maintain.
- **ffmpeg subprocess for decode** — re-introduces the per-invocation subprocess overhead embedding was meant to remove, plus the binary-availability failure surface.

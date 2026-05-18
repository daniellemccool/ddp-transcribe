//! WAV → Vec<f32> decoder for whisper.cpp's float32 PCM 16kHz mono audio input.
//!
//! See 0014 (audio-input invariant) for the contract this module enforces.

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioDecodeError {
    #[error("opening WAV file {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: hound::Error,
    },

    #[error("invalid WAV format for {path}: expected 16 kHz mono, got sample_rate={sample_rate} channels={channels}")]
    InvalidFormat {
        path: String,
        sample_rate: u32,
        channels: u16,
    },

    #[error("unsupported sample format for {path}: {detail}")]
    UnsupportedSampleFormat { path: String, detail: String },

    #[error("reading samples from {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: hound::Error,
    },

    #[error("WAV file {path} contains no samples")]
    Empty { path: String },
}

/// Decode a 16 kHz mono WAV file into a Vec<f32> of PCM samples in [-1.0, 1.0].
///
/// Whisper.cpp's C API requires this exact format (api-and-pipeline.md:7).
/// Rejects non-conforming inputs with the appropriate [`AudioDecodeError`] variant.
pub fn decode_wav(path: &Path) -> Result<Vec<f32>, AudioDecodeError> {
    let path_str = path.display().to_string();
    let mut reader = hound::WavReader::open(path).map_err(|e| AudioDecodeError::Open {
        path: path_str.clone(),
        source: e,
    })?;

    let spec = reader.spec();
    if spec.sample_rate != 16000 || spec.channels != 1 {
        return Err(AudioDecodeError::InvalidFormat {
            path: path_str,
            sample_rate: spec.sample_rate,
            channels: spec.channels,
        });
    }

    // Whisper.cpp expects float32 PCM in [-1.0, 1.0]. hound exposes either
    // integer (PCM_S16LE, the yt-dlp postprocessor default) or float samples.
    // Divide i16 samples by 32768.0 (= |i16::MIN|) so that i16::MIN maps to
    // exactly -1.0 and i16::MAX maps to ~0.99997 — the standard PCM
    // normalization. Dividing by i16::MAX would push i16::MIN to -1.00003.
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            if spec.bits_per_sample != 16 {
                return Err(AudioDecodeError::UnsupportedSampleFormat {
                    path: path_str,
                    detail: format!("int {} bits per sample (expected 16)", spec.bits_per_sample),
                });
            }
            reader
                .samples::<i16>()
                .map(|r| r.map(|s| s as f32 / 32768.0))
                .collect::<Result<_, _>>()
                .map_err(|e| AudioDecodeError::Read {
                    path: path_str.clone(),
                    source: e,
                })?
        }
        hound::SampleFormat::Float => {
            reader
                .samples::<f32>()
                .collect::<Result<_, _>>()
                .map_err(|e| AudioDecodeError::Read {
                    path: path_str.clone(),
                    source: e,
                })?
        }
    };

    if samples.is_empty() {
        return Err(AudioDecodeError::Empty { path: path_str });
    }

    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use tempfile::NamedTempFile;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/audio")
            .join(name)
    }

    fn write_int_wav(sample_rate: u32, channels: u16, bits: u16, samples: &[i16]) -> NamedTempFile {
        let tmp = tempfile::Builder::new()
            .suffix(".wav")
            .tempfile()
            .expect("create tempfile");
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: bits,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(tmp.path(), spec).expect("create wav writer");
        for &s in samples {
            writer.write_sample(s).expect("write sample");
        }
        writer.finalize().expect("finalize");
        tmp
    }

    #[test]
    fn decodes_known_16khz_mono_wav_to_nonempty_vec() {
        let path = fixture_path("silence_16khz_mono.wav");
        let samples = decode_wav(&path).expect("fixture should decode");
        assert!(!samples.is_empty(), "expected non-empty samples");
        // 2 seconds at 16 kHz mono = 32000 samples
        assert!(
            samples.len() > 30000 && samples.len() < 35000,
            "expected ~32000 samples for 2-second fixture, got {}",
            samples.len()
        );
    }

    #[test]
    fn rejects_nonexistent_file() {
        let path = fixture_path("does-not-exist.wav");
        let err = decode_wav(&path).expect_err("missing file should error");
        assert!(matches!(err, AudioDecodeError::Open { .. }));
    }

    #[test]
    fn rejects_wrong_sample_rate() {
        let tmp = write_int_wav(8000, 1, 16, &[0i16; 100]);
        let err = decode_wav(tmp.path()).expect_err("8 kHz should be rejected");
        assert!(matches!(
            err,
            AudioDecodeError::InvalidFormat {
                sample_rate: 8000,
                channels: 1,
                ..
            }
        ));
    }

    #[test]
    fn rejects_stereo() {
        let tmp = write_int_wav(16000, 2, 16, &[0i16; 100]);
        let err = decode_wav(tmp.path()).expect_err("stereo should be rejected");
        assert!(matches!(
            err,
            AudioDecodeError::InvalidFormat {
                sample_rate: 16000,
                channels: 2,
                ..
            }
        ));
    }

    #[test]
    fn rejects_unsupported_int_bits() {
        let tmp = write_int_wav(16000, 1, 24, &[0i32 as i16; 100]);
        let err = decode_wav(tmp.path()).expect_err("24-bit int should be rejected");
        assert!(matches!(
            err,
            AudioDecodeError::UnsupportedSampleFormat { .. }
        ));
    }

    #[test]
    fn rejects_empty_wav() {
        let tmp = write_int_wav(16000, 1, 16, &[]);
        let err = decode_wav(tmp.path()).expect_err("empty WAV should be rejected");
        assert!(matches!(err, AudioDecodeError::Empty { .. }));
    }

    #[test]
    fn normalizes_i16_min_to_exactly_negative_one() {
        let tmp = write_int_wav(16000, 1, 16, &[i16::MIN, i16::MAX]);
        let samples = decode_wav(tmp.path()).expect("valid WAV should decode");
        assert_eq!(samples.len(), 2);
        // i16::MIN / 32768.0 = -1.0 exactly
        assert_eq!(samples[0], -1.0, "i16::MIN should map to exactly -1.0");
        // i16::MAX / 32768.0 ≈ 0.99997
        assert!(
            samples[1] > 0.999 && samples[1] < 1.0,
            "i16::MAX should map to ~0.99997, got {}",
            samples[1]
        );
    }
}

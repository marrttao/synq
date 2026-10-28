//! Decoding a track to what Whisper wants: mono f32 samples at 16 kHz.

use std::fs::File;
use std::path::Path;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use thiserror::Error;

/// Whisper's input sample rate.
pub const TARGET_RATE: u32 = 16_000;
/// FFT block size of the resampler.
const CHUNK: usize = 1024;

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("cannot open the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot decode the audio: {0}")]
    Symphonia(#[from] SymphoniaError),
    #[error("the file has no audio track")]
    NoTrack,
    #[error("resampling failed: {0}")]
    Resample(String),
}

/// Average interleaved channels into one.
pub fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32),
    );
}

/// Decode the file to mono samples at its own sample rate.
fn decode_mono(path: &Path) -> Result<(Vec<f32>, u32), DecodeError> {
    let file = File::open(path)?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        stream,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(DecodeError::NoTrack)?;
    let track_id = track.id;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut mono = Vec::new();
    let mut buffer: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // End of stream is reported as an I/O error.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A damaged packet: skip it and keep going.
            Err(SymphoniaError::DecodeError(e)) => {
                tracing::debug!("skipping undecodable packet: {e}");
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let spec = *decoded.spec();
        rate = spec.rate;
        let needed = decoded.capacity() as u64;
        let buf = match &mut buffer {
            Some(b) if b.capacity() as u64 >= needed * spec.channels.count() as u64 => b,
            slot => slot.insert(SampleBuffer::new(needed, spec)),
        };
        buf.copy_interleaved_ref(decoded);
        downmix(buf.samples(), spec.channels.count(), &mut mono);
    }
    if rate == 0 {
        return Err(DecodeError::NoTrack);
    }
    Ok((mono, rate))
}

/// Resample mono audio from `from` Hz to 16 kHz.
pub fn resample_to_16k(mono: Vec<f32>, from: u32) -> Result<Vec<f32>, DecodeError> {
    if from == TARGET_RATE || mono.is_empty() {
        return Ok(mono);
    }
    let fail = |e: &dyn std::fmt::Display| DecodeError::Resample(e.to_string());
    let mut resampler = Fft::<f32>::new(
        from as usize,
        TARGET_RATE as usize,
        CHUNK,
        1,
        FixedSync::Input,
    )
    .map_err(|e| fail(&e))?;
    let frames = mono.len();
    let input = InterleavedSlice::new(&mono, 1, frames).map_err(|e| fail(&e))?;
    let output = resampler
        .process_all(&input, frames, None)
        .map_err(|e| fail(&e))?;
    Ok(output.take_data())
}

/// Decode `path` to 16 kHz mono f32, ready for Whisper.
pub fn load_16k_mono(path: &Path) -> Result<Vec<f32>, DecodeError> {
    let (mono, rate) = decode_mono(path)?;
    resample_to_16k(mono, rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16-bit PCM WAV with the given interleaved samples.
    fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut v = Vec::new();
        v.extend(b"RIFF");
        v.extend((36 + data_len).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(channels.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * u32::from(channels) * 2).to_le_bytes());
        v.extend((channels * 2).to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend(data_len.to_le_bytes());
        for s in samples {
            v.extend(s.to_le_bytes());
        }
        v
    }

    fn sine(rate: u32, hz: f32, seconds: f32, amp: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0], 2, &mut out);
        assert_eq!(out, [0.5, 0.5, 0.0]);
        let mut out = Vec::new();
        downmix(&[0.1, 0.2], 1, &mut out);
        assert_eq!(out, [0.1, 0.2]);
    }

    #[test]
    fn resampling_changes_length_and_keeps_the_signal() {
        let input = sine(44_100, 440.0, 2.0, 0.5);
        let out = resample_to_16k(input, 44_100).unwrap();
        let expected = 32_000i64;
        assert!(
            (out.len() as i64 - expected).abs() < 64,
            "got {} samples",
            out.len()
        );
        // The middle of the signal keeps its energy (0.5 / sqrt 2).
        let middle = &out[8_000..24_000];
        assert!((rms(middle) - 0.3535).abs() < 0.02, "rms {}", rms(middle));
    }

    #[test]
    fn already_16k_audio_is_passed_through() {
        let input = vec![0.25; 100];
        assert_eq!(resample_to_16k(input.clone(), 16_000).unwrap(), input);
        assert!(resample_to_16k(Vec::new(), 44_100).unwrap().is_empty());
    }

    #[test]
    fn decodes_stereo_wav_to_16k_mono() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tone.wav");
        // 1 s of a 440 Hz tone, left channel only.
        let left = sine(44_100, 440.0, 1.0, 0.8);
        let mut samples = Vec::new();
        for s in &left {
            samples.push((s * f32::from(i16::MAX)) as i16);
            samples.push(0);
        }
        std::fs::write(&path, wav(44_100, 2, &samples)).unwrap();

        let out = load_16k_mono(&path).unwrap();
        assert!((out.len() as i64 - 16_000).abs() < 64, "{}", out.len());
        // Averaging with a silent channel halves the amplitude.
        let r = rms(&out[4_000..12_000]);
        assert!((r - 0.8 / 2.0 / 2f32.sqrt()).abs() < 0.03, "rms {r}");
    }

    #[test]
    fn missing_and_garbage_files_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            load_16k_mono(&tmp.path().join("nope.mp3")),
            Err(DecodeError::Io(_))
        ));
        let junk = tmp.path().join("junk.mp3");
        std::fs::write(&junk, b"this is not audio at all, just text").unwrap();
        assert!(load_16k_mono(&junk).is_err());
    }
}

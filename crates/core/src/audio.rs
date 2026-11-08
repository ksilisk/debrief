//! Audio as Whisper takes it: 16 kHz mono.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavIntoSamples, WavReader};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

pub const WHISPER_RATE: u32 = 16_000;

const CHUNK_FRAMES: usize = 4_800;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("could not read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },
    #[error("could not convert {} to 16 kHz: {reason}", path.display())]
    Resample { path: PathBuf, reason: String },
}

/// Reads a WAV file as 16 kHz mono. The 48 kHz file is read piece by piece, so
/// only the 16 kHz result is held in memory.
pub fn read_for_whisper(path: &Path) -> Result<Vec<f32>, AudioError> {
    let read_error = |source| AudioError::Read {
        path: path.to_path_buf(),
        source,
    };
    let reader = WavReader::open(path).map_err(read_error)?;
    let spec = reader.spec();
    let total = reader.duration() as usize;
    let mut frames = Frames::new(reader);
    let audio =
        resample(&mut frames, spec.sample_rate, total).map_err(|reason| AudioError::Resample {
            path: path.to_path_buf(),
            reason,
        })?;
    match frames.error {
        Some(source) => Err(read_error(source)),
        None => Ok(audio),
    }
}

/// Mono frames from a WAV file of any channel count and sample format. Stops at
/// the first read error and keeps it.
struct Frames {
    samples: Samples,
    channels: usize,
    error: Option<hound::Error>,
}

enum Samples {
    Float(WavIntoSamples<BufReader<File>, f32>),
    Int(WavIntoSamples<BufReader<File>, i32>, f32),
}

impl Frames {
    fn new(reader: WavReader<BufReader<File>>) -> Self {
        let spec = reader.spec();
        let samples = match spec.sample_format {
            SampleFormat::Float => Samples::Float(reader.into_samples()),
            SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample.max(1) - 1)) as f32;
                Samples::Int(reader.into_samples(), scale)
            }
        };
        Self {
            samples,
            channels: usize::from(spec.channels.max(1)),
            error: None,
        }
    }

    fn sample(&mut self) -> Option<f32> {
        let next = match &mut self.samples {
            Samples::Float(samples) => samples.next(),
            Samples::Int(samples, scale) => {
                let scale = *scale;
                samples.next().map(|s| s.map(|v| v as f32 * scale))
            }
        };
        match next? {
            Ok(value) => Some(value),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

impl Iterator for Frames {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let mut sum = self.sample()?;
        for _ in 1..self.channels {
            sum += self.sample()?;
        }
        Some(sum / self.channels as f32)
    }
}

/// `total` is the number of input frames, used to trim the result to length.
fn resample(
    frames: &mut dyn Iterator<Item = f32>,
    from: u32,
    total: usize,
) -> Result<Vec<f32>, String> {
    if from == WHISPER_RATE {
        return Ok(frames.collect());
    }
    let mut resampler = Fft::<f32>::new(
        from as usize,
        WHISPER_RATE as usize,
        CHUNK_FRAMES,
        1,
        FixedSync::Input,
    )
    .map_err(|e| e.to_string())?;
    let expected = (total as u64 * u64::from(WHISPER_RATE)).div_ceil(u64::from(from)) as usize;
    let delay = resampler.output_delay();
    let mut output = Vec::with_capacity(expected + delay + resampler.output_frames_max());
    let mut input = vec![0.0; resampler.input_frames_max()];
    let mut chunk = vec![0.0; resampler.output_frames_max()];
    let mut ended = false;

    while output.len() < delay + expected {
        let need = resampler.input_frames_next();
        let mut filled = 0;
        while !ended && filled < need {
            match frames.next() {
                Some(value) => {
                    input[filled] = value;
                    filled += 1;
                }
                None => ended = true,
            }
        }
        input[filled..need].fill(0.0);
        let indexing = (filled < need).then(|| Indexing {
            partial_len: Some(filled),
            ..Indexing::default()
        });
        let input_buffer =
            InterleavedSlice::new(&input[..need], 1, need).map_err(|e| e.to_string())?;
        let chunk_len = chunk.len();
        let mut output_buffer =
            InterleavedSlice::new_mut(&mut chunk, 1, chunk_len).map_err(|e| e.to_string())?;
        let (_, produced) = resampler
            .process_into_buffer(&input_buffer, &mut output_buffer, indexing.as_ref())
            .map_err(|e| e.to_string())?;
        output.extend_from_slice(&chunk[..produced]);
        if ended && produced == 0 {
            break;
        }
    }
    output.drain(..delay.min(output.len()));
    output.truncate(expected);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{SAMPLE_RATE, TrackWriter};
    use crate::testing::temp_folder;
    use std::f32::consts::TAU;

    fn tone(hz: f32, seconds: f32, rate: u32) -> Vec<f32> {
        let count = (seconds * rate as f32) as usize;
        (0..count)
            .map(|i| 0.5 * (TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn speech_frequencies_pass_at_the_right_length() {
        let input = tone(1_000.0, 2.0, SAMPLE_RATE);
        let output =
            resample(&mut input.iter().copied(), SAMPLE_RATE, input.len()).expect("resample");
        assert_eq!(output.len(), 32_000);
        let middle = &output[4_000..28_000];
        assert!((rms(middle) - 0.5 / 2f32.sqrt()).abs() < 0.01);
        let crossings = middle
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!(
            (1_480..=1_520).contains(&crossings),
            "{crossings} crossings"
        );
    }

    #[test]
    fn sound_above_8_khz_is_removed_not_folded_down() {
        let input = tone(12_000.0, 1.0, SAMPLE_RATE);
        let output =
            resample(&mut input.iter().copied(), SAMPLE_RATE, input.len()).expect("resample");
        assert!(rms(&output[2_000..14_000]) < 0.005);
    }

    #[test]
    fn reads_a_recorded_track() {
        let folder = temp_folder("audio-read");
        let path = folder.join("me.wav");
        let mut writer = TrackWriter::create(&path).expect("create");
        writer
            .write(0.0, SAMPLE_RATE, &tone(440.0, 1.5, SAMPLE_RATE))
            .expect("write");
        writer.finish().expect("finish");
        let audio = read_for_whisper(&path).expect("read");
        assert_eq!(audio.len(), 24_000);
        assert!(rms(&audio[2_000..22_000]) > 0.3);
    }
}

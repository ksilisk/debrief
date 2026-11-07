use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavSpec, WavWriter};
use serde::Serialize;

pub const SAMPLE_RATE: u32 = 48_000;

/// Smaller timing errors are normal jitter and are left alone.
const TOLERANCE_SECONDS: f64 = 0.010;
/// The WAV header is rewritten this often, so a crash loses at most this much audio.
const FLUSH_INTERVAL_SECONDS: u32 = 5;

#[derive(Debug, thiserror::Error)]
#[error("could not write {}: {source}", path.display())]
pub struct RecordingError {
    path: PathBuf,
    #[source]
    source: hound::Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Me,
    Them,
}

impl Track {
    pub fn file_name(self) -> &'static str {
        match self {
            Track::Me => "me.wav",
            Track::Them => "them.wav",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TrackStats {
    pub chunks: u64,
    pub frames: u64,
    pub source_rates: Vec<u32>,
    pub resampled_chunks: u64,
    /// Seconds between the start of the recording and this track's first audio.
    pub first_chunk_at: Option<f64>,
    pub max_timing_error_ms: f64,
    pub gaps_filled: u64,
    pub silence_added_ms: f64,
    pub overlaps_trimmed: u64,
    pub audio_trimmed_ms: f64,
    pub peak: f32,
}

impl TrackStats {
    pub fn duration_seconds(&self) -> f64 {
        self.frames as f64 / f64::from(SAMPLE_RATE)
    }

    pub fn bytes(&self) -> u64 {
        44 + self.frames * 2
    }
}

/// Writes one track as 48 kHz mono 16-bit WAV and keeps it on the recording's clock:
/// each chunk lands where its timestamp says, so a gap becomes silence and an
/// overlap is trimmed.
pub struct TrackWriter {
    path: PathBuf,
    wav: WavWriter<BufWriter<File>>,
    resampler: Option<LinearResampler>,
    unflushed: u32,
    stats: TrackStats,
}

impl TrackWriter {
    pub fn create(path: &Path) -> Result<Self, RecordingError> {
        let spec = WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let wav = WavWriter::create(path, spec).map_err(|source| RecordingError {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            wav,
            resampler: None,
            unflushed: 0,
            stats: TrackStats::default(),
        })
    }

    /// `offset` is the chunk's start in seconds since the recording started.
    pub fn write(
        &mut self,
        offset: f64,
        sample_rate: u32,
        samples: &[f32],
    ) -> Result<(), RecordingError> {
        if samples.is_empty() || sample_rate == 0 {
            return Ok(());
        }
        if !self.stats.source_rates.contains(&sample_rate) {
            self.stats.source_rates.push(sample_rate);
        }

        let resampled;
        let samples = if sample_rate == SAMPLE_RATE {
            samples
        } else {
            let resampler = match &mut self.resampler {
                Some(r) if r.from == sample_rate => r,
                slot => slot.insert(LinearResampler::new(sample_rate, SAMPLE_RATE)),
            };
            resampled = resampler.process(samples);
            self.stats.resampled_chunks += 1;
            &resampled
        };

        let expected = (offset * f64::from(SAMPLE_RATE)).round() as i64;
        let error = expected - self.stats.frames as i64;
        let tolerance = (TOLERANCE_SECONDS * f64::from(SAMPLE_RATE)) as i64;
        let mut skip = 0;
        if self.stats.chunks == 0 {
            self.stats.first_chunk_at = Some(offset);
            if error > 0 {
                self.push_silence(error)?;
            } else {
                skip = (error.unsigned_abs() as usize).min(samples.len());
            }
        } else {
            self.stats.max_timing_error_ms = self
                .stats
                .max_timing_error_ms
                .max(frames_to_ms(error.unsigned_abs()));
            if error > tolerance {
                self.push_silence(error)?;
                self.stats.gaps_filled += 1;
                self.stats.silence_added_ms += frames_to_ms(error.unsigned_abs());
            } else if error < -tolerance {
                skip = (error.unsigned_abs() as usize).min(samples.len());
                self.stats.overlaps_trimmed += 1;
                self.stats.audio_trimmed_ms += frames_to_ms(skip as u64);
            }
        }
        self.stats.chunks += 1;
        for &sample in &samples[skip..] {
            self.push(sample)?;
        }
        Ok(())
    }

    pub fn stats(&self) -> &TrackStats {
        &self.stats
    }

    pub fn finish(self) -> Result<TrackStats, RecordingError> {
        let path = self.path;
        self.wav
            .finalize()
            .map_err(|source| RecordingError { path, source })?;
        Ok(self.stats)
    }

    fn push_silence(&mut self, frames: i64) -> Result<(), RecordingError> {
        for _ in 0..frames {
            self.push(0.0)?;
        }
        Ok(())
    }

    fn push(&mut self, sample: f32) -> Result<(), RecordingError> {
        let sample = sample.clamp(-1.0, 1.0);
        self.stats.peak = self.stats.peak.max(sample.abs());
        self.wav
            .write_sample((sample * f32::from(i16::MAX)) as i16)
            .map_err(|source| self.error(source))?;
        self.stats.frames += 1;
        self.unflushed += 1;
        if self.unflushed >= FLUSH_INTERVAL_SECONDS * SAMPLE_RATE {
            self.wav.flush().map_err(|source| self.error(source))?;
            self.unflushed = 0;
        }
        Ok(())
    }

    fn error(&self, source: hound::Error) -> RecordingError {
        RecordingError {
            path: self.path.clone(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordedTracks {
    pub me: TrackStats,
    pub them: TrackStats,
}

/// The Me and Them tracks of one recording, sharing one origin: the timestamp
/// of the first chunk from either track.
pub struct Tracks {
    origin: Option<f64>,
    me: TrackWriter,
    them: TrackWriter,
}

impl Tracks {
    pub fn create(folder: &Path) -> Result<Self, RecordingError> {
        Ok(Self {
            origin: None,
            me: TrackWriter::create(&folder.join(Track::Me.file_name()))?,
            them: TrackWriter::create(&folder.join(Track::Them.file_name()))?,
        })
    }

    /// `time` is the chunk's timestamp in seconds on the capture clock.
    pub fn write(
        &mut self,
        track: Track,
        time: f64,
        sample_rate: u32,
        samples: &[f32],
    ) -> Result<(), RecordingError> {
        let origin = *self.origin.get_or_insert(time);
        let writer = match track {
            Track::Me => &mut self.me,
            Track::Them => &mut self.them,
        };
        writer.write(time - origin, sample_rate, samples)
    }

    pub fn stats(&self, track: Track) -> &TrackStats {
        match track {
            Track::Me => self.me.stats(),
            Track::Them => self.them.stats(),
        }
    }

    pub fn finish(self) -> Result<RecordedTracks, RecordingError> {
        Ok(RecordedTracks {
            me: self.me.finish()?,
            them: self.them.finish()?,
        })
    }
}

fn frames_to_ms(frames: u64) -> f64 {
    frames as f64 * 1000.0 / f64::from(SAMPLE_RATE)
}

/// Good enough for speech. It carries the fractional output length between
/// chunks so the track does not slowly drift.
struct LinearResampler {
    from: u32,
    to: u32,
    carry: f64,
}

impl LinearResampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            from,
            to,
            carry: 0.0,
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let ratio = f64::from(self.to) / f64::from(self.from);
        let exact = input.len() as f64 * ratio + self.carry;
        let len = exact.floor() as usize;
        self.carry = exact - len as f64;
        let step = 1.0 / ratio;
        (0..len)
            .map(|i| {
                let position = i as f64 * step;
                let index = position.floor() as usize;
                let fraction = (position - index as f64) as f32;
                let a = input[index.min(input.len() - 1)];
                let b = input[(index + 1).min(input.len() - 1)];
                a + (b - a) * fraction
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("debrief-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("create temp folder");
        folder
    }

    fn read(path: &Path) -> Vec<i16> {
        hound::WavReader::open(path)
            .expect("open wav")
            .samples::<i16>()
            .map(|s| s.expect("read sample"))
            .collect()
    }

    const CHUNK: usize = 480;

    #[test]
    fn continuous_chunks_are_written_as_they_are() {
        let folder = temp_folder("continuous");
        let path = folder.join("track.wav");
        let mut writer = TrackWriter::create(&path).expect("create");
        for i in 0..10 {
            let offset = (i * CHUNK) as f64 / f64::from(SAMPLE_RATE);
            writer
                .write(offset, SAMPLE_RATE, &[0.5; CHUNK])
                .expect("write");
        }
        let stats = writer.finish().expect("finish");
        assert_eq!(stats.frames, 10 * CHUNK as u64);
        assert_eq!(stats.gaps_filled, 0);
        assert_eq!(stats.overlaps_trimmed, 0);
        assert_eq!(read(&path).len(), 10 * CHUNK);
    }

    #[test]
    fn a_gap_becomes_silence() {
        let folder = temp_folder("gap");
        let path = folder.join("track.wav");
        let mut writer = TrackWriter::create(&path).expect("create");
        writer
            .write(0.0, SAMPLE_RATE, &[0.5; CHUNK])
            .expect("write");
        writer
            .write(0.5, SAMPLE_RATE, &[0.5; CHUNK])
            .expect("write");
        let stats = writer.finish().expect("finish");
        assert_eq!(stats.gaps_filled, 1);
        assert_eq!(stats.frames, 24_000 + CHUNK as u64);
        let samples = read(&path);
        assert_eq!(samples[CHUNK], 0);
        assert_ne!(samples[24_000], 0);
    }

    #[test]
    fn an_overlap_is_trimmed() {
        let folder = temp_folder("overlap");
        let path = folder.join("track.wav");
        let mut writer = TrackWriter::create(&path).expect("create");
        writer.write(0.0, SAMPLE_RATE, &[0.5; 4800]).expect("write");
        writer
            .write(0.05, SAMPLE_RATE, &[0.5; 4800])
            .expect("write");
        let stats = writer.finish().expect("finish");
        assert_eq!(stats.overlaps_trimmed, 1);
        assert_eq!(stats.frames, 7200);
    }

    #[test]
    fn small_jitter_is_left_alone() {
        let folder = temp_folder("jitter");
        let path = folder.join("track.wav");
        let mut writer = TrackWriter::create(&path).expect("create");
        writer
            .write(0.0, SAMPLE_RATE, &[0.5; CHUNK])
            .expect("write");
        writer
            .write(0.013, SAMPLE_RATE, &[0.5; CHUNK])
            .expect("write");
        let stats = writer.finish().expect("finish");
        assert_eq!(stats.gaps_filled, 0);
        assert_eq!(stats.frames, 2 * CHUNK as u64);
        assert!((stats.max_timing_error_ms - 3.0).abs() < 0.1);
    }

    #[test]
    fn other_rates_are_resampled_without_drift() {
        let folder = temp_folder("resample");
        let path = folder.join("track.wav");
        let mut writer = TrackWriter::create(&path).expect("create");
        let chunk = 441;
        for i in 0..1000 {
            let offset = (i * chunk) as f64 / 44_100.0;
            writer.write(offset, 44_100, &[0.25; 441]).expect("write");
        }
        let stats = writer.finish().expect("finish");
        assert_eq!(stats.gaps_filled + stats.overlaps_trimmed, 0);
        assert!(stats.frames.abs_diff(480_000) <= 1);
        assert_eq!(stats.source_rates, vec![44_100]);
    }

    #[test]
    fn tracks_share_one_origin() {
        let folder = temp_folder("tracks");
        let mut tracks = Tracks::create(&folder).expect("create");
        tracks
            .write(Track::Them, 100.0, SAMPLE_RATE, &[0.5; CHUNK])
            .expect("write");
        tracks
            .write(Track::Me, 100.25, SAMPLE_RATE, &[0.5; CHUNK])
            .expect("write");
        let recorded = tracks.finish().expect("finish");
        assert_eq!(recorded.me.first_chunk_at, Some(0.25));
        assert_eq!(recorded.me.gaps_filled, 0);
        assert_eq!(recorded.me.frames, 12_000 + CHUNK as u64);
        assert_eq!(recorded.them.frames, CHUNK as u64);
    }
}

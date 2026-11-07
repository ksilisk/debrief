//! Measures how Me and Them line up, using the echo of Them in the microphone.
//! Needs sound on speakers: with headphones there is no echo to measure.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::recording::Track;

const ANALYSIS_RATE: u32 = 2_000;
const WINDOW_SECONDS: f64 = 5.0;
const MAX_LAG_SECONDS: f64 = 0.5;
const MIN_CORRELATION: f64 = 0.25;
const SILENCE_DBFS: f64 = -50.0;

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("could not read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: hound::Error,
    },
    #[error("{} is not a mono 16-bit WAV file", path.display())]
    Format { path: PathBuf },
    #[error("me.wav and them.wav have different sample rates")]
    RateMismatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct Level {
    pub duration_seconds: f64,
    pub rms_dbfs: f64,
    pub peak_dbfs: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Window {
    pub at_seconds: f64,
    /// Positive when Me hears the sound later than Them.
    pub lag_ms: f64,
    pub correlation: f64,
    /// Echo level in Me relative to the same sound in Them.
    pub echo_db: f64,
    pub echo_dbfs: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    pub me: Level,
    pub them: Level,
    pub windows_checked: usize,
    pub windows: Vec<Window>,
    pub lag_spread_ms: Option<f64>,
    pub median_echo_db: Option<f64>,
}

pub fn analyze_folder(folder: &Path, step_seconds: f64) -> Result<SyncReport, AnalysisError> {
    let (me, me_signal, me_rate) = read_track(&folder.join(Track::Me.file_name()))?;
    let (them, them_signal, them_rate) = read_track(&folder.join(Track::Them.file_name()))?;
    if (me_rate - them_rate).abs() > f64::EPSILON {
        return Err(AnalysisError::RateMismatch);
    }
    let (windows_checked, windows) = compare(&me_signal, &them_signal, me_rate, step_seconds);
    Ok(SyncReport {
        me,
        them,
        windows_checked,
        lag_spread_ms: spread(windows.iter().map(|w| w.lag_ms)),
        median_echo_db: median(windows.iter().map(|w| w.echo_db).collect()),
        windows,
    })
}

/// Reads a track, measures its level, and returns it averaged down to about
/// `ANALYSIS_RATE`, which is plenty for lining up speech.
fn read_track(path: &Path) -> Result<(Level, Vec<f32>, f64), AnalysisError> {
    let read_error = |source| AnalysisError::Read {
        path: path.to_path_buf(),
        source,
    };
    let mut reader = hound::WavReader::open(path).map_err(read_error)?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err(AnalysisError::Format {
            path: path.to_path_buf(),
        });
    }
    let factor = (spec.sample_rate / ANALYSIS_RATE).max(1) as usize;
    let mut signal = Vec::with_capacity(reader.len() as usize / factor + 1);
    let (mut sum, mut count, mut energy, mut peak, mut frames) = (0.0f32, 0, 0.0f64, 0.0f32, 0u64);
    for sample in reader.samples::<i16>() {
        let value = f32::from(sample.map_err(read_error)?) / 32_768.0;
        energy += f64::from(value * value);
        peak = peak.max(value.abs());
        frames += 1;
        sum += value;
        count += 1;
        if count == factor {
            signal.push(sum / factor as f32);
            sum = 0.0;
            count = 0;
        }
    }
    let level = Level {
        duration_seconds: frames as f64 / f64::from(spec.sample_rate),
        rms_dbfs: power_db(energy / frames.max(1) as f64),
        peak_dbfs: amplitude_db(f64::from(peak)),
    };
    Ok((level, signal, f64::from(spec.sample_rate) / factor as f64))
}

/// Returns how many windows were checked and the ones with a clear echo.
fn compare(me: &[f32], them: &[f32], rate: f64, step_seconds: f64) -> (usize, Vec<Window>) {
    let window = (WINDOW_SECONDS * rate) as usize;
    let max_lag = (MAX_LAG_SECONDS * rate) as usize;
    let step = ((step_seconds * rate) as usize).max(1);
    let len = me.len().min(them.len());

    let mut checked = 0;
    let mut found = Vec::new();
    let mut start = max_lag;
    while start + window + max_lag <= len {
        checked += 1;
        if let Some(result) = compare_window(me, them, start, window, max_lag, rate) {
            found.push(result);
        }
        start += step;
    }
    (checked, found)
}

fn compare_window(
    me: &[f32],
    them: &[f32],
    start: usize,
    window: usize,
    max_lag: usize,
    rate: f64,
) -> Option<Window> {
    let reference = &them[start..start + window];
    let reference_energy: f64 = reference.iter().map(|&v| f64::from(v * v)).sum();
    if power_db(reference_energy / window as f64) < SILENCE_DBFS {
        return None;
    }

    let search = &me[start - max_lag..start + window + max_lag];
    let mut prefix = Vec::with_capacity(search.len() + 1);
    prefix.push(0.0f64);
    for &v in search {
        prefix.push(prefix[prefix.len() - 1] + f64::from(v * v));
    }

    let mut best: Option<(usize, f64, f64)> = None;
    for shift in 0..=2 * max_lag {
        let candidate = &search[shift..shift + window];
        let energy = prefix[shift + window] - prefix[shift];
        if energy <= 0.0 {
            continue;
        }
        let dot = f64::from(
            reference
                .iter()
                .zip(candidate)
                .map(|(a, b)| a * b)
                .sum::<f32>(),
        );
        let correlation = dot / (reference_energy * energy).sqrt();
        if best.is_none_or(|(_, r, _)| correlation.abs() > r.abs()) {
            best = Some((shift, correlation, dot));
        }
    }

    let (shift, correlation, dot) = best?;
    if correlation.abs() < MIN_CORRELATION {
        return None;
    }
    let gain = (dot / reference_energy).abs();
    Some(Window {
        at_seconds: start as f64 / rate,
        lag_ms: (shift as f64 - max_lag as f64) / rate * 1000.0,
        correlation,
        echo_db: amplitude_db(gain),
        echo_dbfs: amplitude_db(gain * (reference_energy / window as f64).sqrt()),
    })
}

fn spread(values: impl Iterator<Item = f64>) -> Option<f64> {
    values
        .fold(None, |range: Option<(f64, f64)>, v| match range {
            None => Some((v, v)),
            Some((low, high)) => Some((low.min(v), high.max(v))),
        })
        .map(|(low, high)| high - low)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

fn amplitude_db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-10).log10()
}

fn power_db(power: f64) -> f64 {
    10.0 * power.max(1e-20).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 20_000) as f32 / 20_000.0 - 0.5
            })
            .collect()
    }

    #[test]
    fn finds_the_echo_delay_and_level() {
        let rate = f64::from(ANALYSIS_RATE);
        let len = (60.0 * rate) as usize;
        let delay = (0.040 * rate) as usize;
        let them = noise(len, 1);
        let room = noise(len, 2);
        let me: Vec<f32> = (0..len)
            .map(|i| {
                let echo = if i >= delay {
                    them[i - delay] * 0.1
                } else {
                    0.0
                };
                echo + room[i] * 0.01
            })
            .collect();

        let (checked, windows) = compare(&me, &them, rate, 10.0);
        assert!(checked >= 5);
        assert_eq!(windows.len(), checked);
        for window in &windows {
            assert!((window.lag_ms - 40.0).abs() < 1.0, "lag {}", window.lag_ms);
            assert!(
                (window.echo_db + 20.0).abs() < 1.0,
                "echo {}",
                window.echo_db
            );
        }
        assert!(spread(windows.iter().map(|w| w.lag_ms)).is_some_and(|s| s < 1.0));
    }

    #[test]
    fn no_echo_means_no_windows() {
        let rate = f64::from(ANALYSIS_RATE);
        let len = (30.0 * rate) as usize;
        let (checked, windows) = compare(&noise(len, 3), &noise(len, 4), rate, 5.0);
        assert!(checked > 0);
        assert!(windows.is_empty());
    }

    #[test]
    fn silent_them_is_skipped() {
        let rate = f64::from(ANALYSIS_RATE);
        let len = (30.0 * rate) as usize;
        let (_, windows) = compare(&noise(len, 5), &vec![0.0; len], rate, 5.0);
        assert!(windows.is_empty());
    }
}

//! Silero VAD, and the audio Whisper gets: only the speech, each segment at the
//! same level, joined the way whisper.cpp's own VAD joins it, with Whisper's
//! times mapped back to the track.

use std::ffi::{c_int, c_void};
use std::ptr::NonNull;

use debrief_core::audio::WHISPER_RATE;
use whisper_rs::whisper_rs_sys as sys;

/// Silero VAD 5.1.2 in whisper.cpp's format (MIT, see `assets/LICENSE-silero-vad`).
const MODEL: &[u8] = include_bytes!("../assets/ggml-silero-v5.1.2.bin");

/// whisper.cpp keeps 0.1 s of audio after each speech segment but the last and
/// puts 0.1 s of silence between segments; doing the same gives Whisper exactly
/// the audio it was tested with.
const OVERLAP_SAMPLES: usize = 1_600;
const GAP_SAMPLES: usize = 1_600;

/// Each speech segment is brought to this level. When one microphone records a
/// room, a voice far from it otherwise reaches Whisper much quieter than the
/// near one, and Whisper guesses its words or leaves them out.
const LEVEL_DBFS: f32 = -20.0;
/// Near-silent segments are raised by no more than this.
const MAX_GAIN_DB: f32 = 30.0;

/// A line edge this close to the start or end of speech is taken as right.
const NEAR_SECONDS: f64 = 0.3;
/// A line edge this close to the far side of a pause is on the wrong side of it.
const EDGE_SECONDS: f64 = 0.5;

/// Seconds in the track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

pub struct Vad(NonNull<sys::whisper_vad_context>);

// SAFETY: like the Whisper context, it moves between threads but is used by one at a time.
unsafe impl Send for Vad {}

impl Vad {
    pub fn load() -> Option<Self> {
        let mut reader = Reader {
            data: MODEL,
            position: 0,
        };
        let mut loader = sys::whisper_model_loader {
            context: &mut reader as *mut Reader as *mut c_void,
            read: Some(read),
            eof: Some(eof),
            close: Some(close),
        };
        let context = unsafe {
            sys::whisper_vad_init_with_params(
                &mut loader,
                sys::whisper_vad_default_context_params(),
            )
        };
        NonNull::new(context).map(Self)
    }

    /// Speech in 16 kHz audio, with whisper.cpp's default settings.
    pub fn speech(&mut self, audio: &[f32]) -> Option<Vec<Span>> {
        let rate = f64::from(WHISPER_RATE);
        let length = c_int::try_from(audio.len()).ok()?;
        unsafe {
            let segments = NonNull::new(sys::whisper_vad_segments_from_samples(
                self.0.as_ptr(),
                sys::whisper_vad_default_params(),
                audio.as_ptr(),
                length,
            ))?;
            let count = sys::whisper_vad_segments_n_segments(segments.as_ptr());
            let spans = (0..count)
                .map(|i| Span {
                    start: centiseconds(sys::whisper_vad_segments_get_segment_t0(
                        segments.as_ptr(),
                        i,
                    )) as f64
                        / 100.0,
                    end: centiseconds(sys::whisper_vad_segments_get_segment_t1(
                        segments.as_ptr(),
                        i,
                    )) as f64
                        / 100.0,
                })
                .filter(|span| span.end > span.start && span.start * rate < audio.len() as f64)
                .collect();
            sys::whisper_vad_free_segments(segments.as_ptr());
            Some(spans)
        }
    }
}

impl Drop for Vad {
    fn drop(&mut self) {
        unsafe { sys::whisper_vad_free(self.0.as_ptr()) }
    }
}

fn centiseconds(value: f32) -> i64 {
    value.round() as i64
}

struct Reader {
    data: &'static [u8],
    position: usize,
}

/// SAFETY: `context` is the `Reader` that `Vad::load` keeps alive while loading.
unsafe extern "C" fn read(context: *mut c_void, output: *mut c_void, size: usize) -> usize {
    let reader = unsafe { &mut *(context as *mut Reader) };
    let count = size.min(reader.data.len() - reader.position);
    unsafe {
        std::ptr::copy_nonoverlapping(
            reader.data[reader.position..].as_ptr(),
            output as *mut u8,
            count,
        );
    }
    reader.position += count;
    count
}

/// SAFETY: as for `read`.
unsafe extern "C" fn eof(context: *mut c_void) -> bool {
    let reader = unsafe { &*(context as *const Reader) };
    reader.position >= reader.data.len()
}

unsafe extern "C" fn close(_context: *mut c_void) {}

/// The speech of a track joined into one stretch of audio.
pub struct Joined {
    pub audio: Vec<f32>,
    parts: Vec<Part>,
}

/// One speech segment in the joined audio, in samples.
struct Part {
    joined_start: usize,
    start: usize,
    length: usize,
}

impl Joined {
    pub fn new(audio: &[f32], speech: &[Span]) -> Self {
        let last = audio.len().saturating_sub(1);
        let mut joined = Vec::new();
        let mut parts = Vec::new();
        for (i, span) in speech.iter().enumerate() {
            let more = i + 1 < speech.len();
            let start = to_samples(span.start).min(last);
            let end = (to_samples(span.end) + if more { OVERLAP_SAMPLES } else { 0 }).min(last);
            if end <= start {
                continue;
            }
            parts.push(Part {
                joined_start: joined.len(),
                start,
                length: end - start,
            });
            let segment = &audio[start..end];
            let gain = gain(segment);
            joined.extend(segment.iter().map(|value| (value * gain).clamp(-1.0, 1.0)));
            if more {
                joined.resize(joined.len() + GAP_SAMPLES, 0.0);
            }
        }
        Self {
            audio: joined,
            parts,
        }
    }

    /// Where a line's start or end in the joined audio lies in the track. A
    /// start in a gap belongs to the speech after it, an end to the speech before.
    pub fn in_track(&self, seconds: f64, is_start: bool) -> f64 {
        let rate = f64::from(WHISPER_RATE);
        let sample = (seconds * rate).max(0.0);
        let mut before = None;
        for part in &self.parts {
            let start = part.joined_start as f64;
            if sample < start {
                return match (is_start, before) {
                    (false, Some(end)) => end,
                    _ => part.start as f64 / rate,
                };
            }
            if sample <= start + part.length as f64 {
                return (part.start as f64 + sample - start) / rate;
            }
            before = Some((part.start + part.length) as f64 / rate);
        }
        before.unwrap_or(seconds)
    }
}

fn gain(segment: &[f32]) -> f32 {
    let power = segment.iter().map(|value| value * value).sum::<f32>() / segment.len() as f32;
    (from_db(LEVEL_DBFS) / power.sqrt().max(f32::MIN_POSITIVE)).min(from_db(MAX_GAIN_DB))
}

fn from_db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// whisper.cpp's `cs_to_samples`: segment times are whole centiseconds.
fn to_samples(seconds: f64) -> usize {
    ((seconds * f64::from(WHISPER_RATE)) + 0.5) as usize
}

/// Whisper often starts a line at the end of the speech before a pause instead
/// of after it, or ends one just past the next pause. Such edges move to the
/// speech the line holds; edges at the start or end of speech stay.
pub fn snap(start: f64, end: f64, speech: &[Span]) -> (f64, f64) {
    let containing = |time: f64| {
        speech
            .iter()
            .find(|span| span.start <= time && time <= span.end)
    };
    let mut start_at = start;
    let start_is_off = containing(start)
        .is_none_or(|span| start - span.start > NEAR_SECONDS && span.end - start < EDGE_SECONDS);
    if start_is_off
        && let Some(next) = speech
            .iter()
            .find(|span| start < span.start && span.start < end)
    {
        start_at = next.start;
    }
    let mut end_at = end;
    let end_is_off = containing(end)
        .is_none_or(|span| span.end - end > NEAR_SECONDS && end - span.start < EDGE_SECONDS);
    if end_is_off
        && let Some(last) = speech
            .iter()
            .rev()
            .find(|span| start_at < span.end && span.end < end)
    {
        end_at = last.end;
    }
    (start_at, end_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, end: f64) -> Span {
        Span { start, end }
    }

    #[test]
    fn the_built_in_model_loads_and_hears_no_speech_in_silence() {
        whisper_rs::install_logging_hooks();
        let mut vad = Vad::load().expect("Silero VAD");
        let silence = vec![0.0; WHISPER_RATE as usize * 3];
        assert_eq!(vad.speech(&silence), Some(Vec::new()));
    }

    fn tone(seconds: f64, amplitude: f32) -> Vec<f32> {
        (0..(seconds * f64::from(WHISPER_RATE)) as usize)
            .map(|i| amplitude * (i as f32 * 0.09).sin())
            .collect()
    }

    fn level_db(samples: &[f32]) -> f32 {
        let power = samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32;
        10.0 * power.log10()
    }

    #[test]
    fn speech_is_joined_like_whisper_cpp_joins_it() {
        let audio: Vec<f32> = (0..16_000 * 10).map(|i| i as f32 / 1e6).collect();
        let joined = Joined::new(&audio, &[span(1.0, 2.0), span(5.0, 6.5)]);
        let first = gain(&audio[16_000..33_600]);
        let second = gain(&audio[80_000..104_000]);
        // 1 s of speech + 0.1 s after it + 0.1 s gap + 1.5 s of speech.
        assert_eq!(joined.audio.len(), 16_000 + 1_600 + 1_600 + 24_000);
        assert_eq!(joined.audio[0], audio[16_000] * first);
        assert_eq!(joined.audio[17_599], audio[33_599] * first);
        assert_eq!(joined.audio[17_600..19_200], [0.0; 1_600]);
        assert_eq!(joined.audio[19_200], audio[80_000] * second);
    }

    #[test]
    fn far_and_near_voices_reach_whisper_at_the_same_level() {
        let mut audio = tone(2.0, 0.01);
        audio.extend(tone(2.0, 0.3));
        let joined = Joined::new(&audio, &[span(0.5, 1.5), span(2.5, 3.5)]);
        assert!((level_db(&joined.audio[..17_600]) - LEVEL_DBFS).abs() < 0.1);
        assert!((level_db(&joined.audio[19_200..]) - LEVEL_DBFS).abs() < 0.1);
    }

    #[test]
    fn near_silence_is_raised_by_30_db_at_most() {
        let audio = tone(2.0, 0.000_1);
        let joined = Joined::new(&audio, &[span(0.5, 1.5)]);
        let raised = level_db(&joined.audio) - level_db(&audio[8_000..24_000]);
        assert!((raised - MAX_GAIN_DB).abs() < 0.1, "raised by {raised} dB");
    }

    #[test]
    fn joined_times_map_back_to_the_track() {
        let audio = vec![0.0; 16_000 * 10];
        let joined = Joined::new(&audio, &[span(1.0, 2.0), span(5.0, 6.5)]);
        assert!((joined.in_track(0.5, true) - 1.5).abs() < 1e-9);
        // Inside the gap: a start belongs to the next speech, an end to the one before.
        assert!((joined.in_track(1.15, true) - 5.0).abs() < 1e-9);
        assert!((joined.in_track(1.15, false) - 2.1).abs() < 1e-9);
        assert!((joined.in_track(1.7, false) - 5.5).abs() < 1e-9);
        assert!((joined.in_track(9.0, false) - 6.5).abs() < 1e-9);
    }

    #[test]
    fn a_start_at_the_end_of_earlier_speech_moves_past_the_pause() {
        let speech = [span(140.40, 141.50), span(193.10, 195.90)];
        assert_eq!(snap(141.23, 195.49, &speech), (193.10, 195.49));
    }

    #[test]
    fn a_short_word_at_the_start_of_speech_stays() {
        let speech = [span(8.51, 8.89), span(15.59, 16.64), span(22.66, 23.29)];
        assert_eq!(snap(8.51, 23.08, &speech), (8.51, 23.08));
    }

    #[test]
    fn edges_in_silence_move_to_the_speech() {
        let speech = [span(10.0, 12.0), span(30.0, 31.0)];
        assert_eq!(snap(8.0, 20.0, &speech), (10.0, 12.0));
        assert_eq!(snap(13.0, 31.0, &speech), (30.0, 31.0));
    }

    #[test]
    fn an_end_just_past_a_pause_moves_back() {
        let speech = [span(10.0, 12.0), span(30.0, 34.0)];
        assert_eq!(snap(10.0, 30.2, &speech), (10.0, 12.0));
    }
}

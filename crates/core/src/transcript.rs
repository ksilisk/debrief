//! `transcript.json`: every line from both tracks, sorted by time.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::files::write_atomically;
use crate::recording::Track;

pub const TRANSCRIPT_FILE: &str = "transcript.json";
pub const FORMAT: u64 = 1;

/// Whisper's line breaks differ between the tracks, so lines this close count
/// as the same moment.
const ECHO_SLACK_SECONDS: f64 = 1.0;
/// Shorter lines ("да", "okay") may really have been said by both people.
const ECHO_MIN_CHARS: usize = 8;
/// Share of the line's three-letter chunks that must also be in the Them lines.
const ECHO_MIN_SHARE: f64 = 0.6;

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    #[error("could not read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is not a valid transcript: {source}", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// What Whisper made of one track.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackText {
    /// The language Whisper used, such as `ru`.
    pub language: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    format: u64,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// Seconds in the recorded audio, the same timeline as the marks.
    pub start: f64,
    pub end: f64,
    pub speaker: Track,
    pub language: String,
    pub text: String,
    /// A Me line that repeats a Them line: the microphone heard the speakers.
    pub echo: bool,
}

impl Transcript {
    pub fn merge(me: TrackText, them: TrackText) -> Self {
        let mut segments = Vec::with_capacity(me.lines.len() + them.lines.len());
        for (speaker, track) in [(Track::Them, them), (Track::Me, me)] {
            segments.extend(track.lines.into_iter().map(|line| Segment {
                start: line.start,
                end: line.end,
                speaker,
                language: track.language.clone(),
                text: line.text,
                echo: false,
            }));
        }
        segments.sort_by(|a, b| a.start.total_cmp(&b.start));
        mark_echo(&mut segments);
        Self {
            format: FORMAT,
            segments,
        }
    }

    pub fn load(folder: &Path) -> Result<Self, TranscriptError> {
        let path = folder.join(TRANSCRIPT_FILE);
        let text = fs::read_to_string(&path).map_err(|source| TranscriptError::Read {
            path: path.clone(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| TranscriptError::Invalid { path, source })
    }

    pub fn save(&self, folder: &Path) -> Result<(), TranscriptError> {
        let path = folder.join(TRANSCRIPT_FILE);
        let write_error = |source| TranscriptError::Write {
            path: path.clone(),
            source,
        };
        let mut json = serde_json::to_vec_pretty(self).map_err(|e| write_error(e.into()))?;
        json.push(b'\n');
        write_atomically(&path, &json).map_err(write_error)
    }

    pub fn echo_lines(&self) -> usize {
        self.segments.iter().filter(|segment| segment.echo).count()
    }
}

fn mark_echo(segments: &mut [Segment]) {
    let them: Vec<(f64, f64, String)> = segments
        .iter()
        .filter(|segment| segment.speaker == Track::Them)
        .map(|segment| (segment.start, segment.end, normalize(&segment.text)))
        .collect();
    for segment in segments
        .iter_mut()
        .filter(|segment| segment.speaker == Track::Me)
    {
        let text = normalize(&segment.text);
        if text.chars().filter(|c| *c != ' ').count() < ECHO_MIN_CHARS {
            continue;
        }
        let nearby: Vec<&str> = them
            .iter()
            .filter(|(start, end, _)| {
                *start <= segment.end + ECHO_SLACK_SECONDS
                    && *end >= segment.start - ECHO_SLACK_SECONDS
            })
            .map(|(_, _, text)| text.as_str())
            .collect();
        if !nearby.is_empty() {
            segment.echo = shared_chunks(&text, &nearby.join(" ")) >= ECHO_MIN_SHARE;
        }
    }
}

/// Lowercase letters and digits, single spaces between words.
fn normalize(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'ё' => 'е',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn chunks(text: &str) -> HashSet<[char; 3]> {
    let padded: Vec<char> = format!(" {text} ").chars().collect();
    padded.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
}

/// The share of `text`'s three-letter chunks that also appear in `other`.
fn shared_chunks(text: &str, other: &str) -> f64 {
    let own = chunks(text);
    if own.is_empty() {
        return 0.0;
    }
    let theirs = chunks(other);
    own.intersection(&theirs).count() as f64 / own.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_folder;

    fn track(language: &str, lines: &[(f64, f64, &str)]) -> TrackText {
        TrackText {
            language: language.into(),
            lines: lines
                .iter()
                .map(|&(start, end, text)| Line {
                    start,
                    end,
                    text: text.into(),
                })
                .collect(),
        }
    }

    fn echo_flags(transcript: &Transcript) -> Vec<(Track, bool)> {
        transcript
            .segments
            .iter()
            .map(|segment| (segment.speaker, segment.echo))
            .collect()
    }

    #[test]
    fn lines_from_both_tracks_are_sorted_by_time() {
        let transcript = Transcript::merge(
            track("ru", &[(0.0, 2.0, "Привет"), (5.0, 7.0, "Да, давай")]),
            track("en", &[(2.5, 4.5, "Hi, shall we start?")]),
        );
        let order: Vec<(f64, Track)> = transcript
            .segments
            .iter()
            .map(|segment| (segment.start, segment.speaker))
            .collect();
        assert_eq!(
            order,
            vec![(0.0, Track::Me), (2.5, Track::Them), (5.0, Track::Me)]
        );
        assert_eq!(transcript.segments[1].language, "en");
    }

    #[test]
    fn a_me_line_repeating_them_at_the_same_moment_is_echo() {
        let transcript = Transcript::merge(
            track("en", &[(10.3, 13.8, "Can you share the relise plan")]),
            track("en", &[(10.0, 13.5, "Can you share the release plan?")]),
        );
        assert_eq!(
            echo_flags(&transcript),
            vec![(Track::Them, false), (Track::Me, true)]
        );
        assert_eq!(transcript.echo_lines(), 1);
    }

    #[test]
    fn echo_can_span_two_them_lines() {
        let transcript = Transcript::merge(
            track("ru", &[(20.0, 26.0, "План в Confluence, деплой в четверг")]),
            track(
                "ru",
                &[
                    (19.8, 22.5, "План в Confluence,"),
                    (22.6, 25.9, "деплой в четверг."),
                ],
            ),
        );
        assert_eq!(transcript.echo_lines(), 1);
    }

    #[test]
    fn the_same_words_at_another_time_are_not_echo() {
        let transcript = Transcript::merge(
            track("en", &[(40.0, 43.0, "Can you share the release plan?")]),
            track("en", &[(10.0, 13.5, "Can you share the release plan?")]),
        );
        assert_eq!(transcript.echo_lines(), 0);
    }

    #[test]
    fn short_answers_and_own_words_are_not_echo() {
        let transcript = Transcript::merge(
            track(
                "ru",
                &[
                    (10.2, 10.8, "Да."),
                    (12.0, 16.0, "Я думаю, нам нужно ещё раз проверить тесты"),
                ],
            ),
            track(
                "ru",
                &[(10.0, 10.6, "Да."), (12.1, 15.0, "Релиз будет в пятницу")],
            ),
        );
        assert_eq!(transcript.echo_lines(), 0);
    }

    #[test]
    fn saves_and_loads() {
        let folder = temp_folder("transcript-roundtrip");
        let transcript = Transcript::merge(
            track("ru", &[(0.0, 1.5, "Привет, это тест")]),
            TrackText::default(),
        );
        transcript.save(&folder).expect("save");
        assert_eq!(Transcript::load(&folder).expect("load"), transcript);
    }
}

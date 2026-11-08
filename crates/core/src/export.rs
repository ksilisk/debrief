//! The transcript files people read: `transcript.md`, `transcript.txt` and
//! `transcript.srt`. Echo lines are left out.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::files::write_atomically;
use crate::meeting::Meeting;
use crate::recording::Track;
use crate::transcript::{Segment, Transcript};

pub const MARKDOWN_FILE: &str = "transcript.md";
pub const TEXT_FILE: &str = "transcript.txt";
pub const SUBTITLES_FILE: &str = "transcript.srt";

const SUBTITLE_LINE_CHARS: usize = 42;
const SUBTITLE_LINES: usize = 2;
/// A zero-length line would never show.
const MIN_SUBTITLE_SECONDS: f64 = 1.0;

#[derive(Debug, thiserror::Error)]
#[error("could not write {}: {source}", path.display())]
pub struct ExportError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

pub fn export(
    folder: &Path,
    meeting: &Meeting,
    transcript: &Transcript,
) -> Result<(), ExportError> {
    for (file, content) in [
        (MARKDOWN_FILE, markdown(meeting, transcript)),
        (TEXT_FILE, plain_text(meeting, transcript)),
        (SUBTITLES_FILE, subtitles(transcript)),
    ] {
        let path = folder.join(file);
        write_atomically(&path, content.as_bytes())
            .map_err(|source| ExportError { path, source })?;
    }
    Ok(())
}

enum Item<'a> {
    Pause { at: f64, seconds: f64 },
    Mark { at: f64 },
    Line(&'a Segment),
}

impl Item<'_> {
    fn sort_key(&self) -> (f64, u8) {
        match self {
            Item::Pause { at, .. } => (*at, 0),
            Item::Mark { at } => (*at, 1),
            Item::Line(segment) => (segment.start, 2),
        }
    }
}

/// Lines, marks and pauses in time order; a mark or pause comes before a line
/// that starts at the same moment.
fn timeline<'a>(meeting: &Meeting, transcript: &'a Transcript) -> Vec<Item<'a>> {
    let mut items: Vec<Item<'a>> = transcript
        .segments
        .iter()
        .filter(|segment| !segment.echo)
        .map(Item::Line)
        .collect();
    items.extend(meeting.marks.iter().map(|mark| Item::Mark {
        at: mark.at_seconds,
    }));
    items.extend(meeting.pauses.iter().map(|pause| Item::Pause {
        at: pause.at_seconds,
        seconds: pause.duration_seconds,
    }));
    items.sort_by(|a, b| {
        let (a, b) = (a.sort_key(), b.sort_key());
        a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))
    });
    items
}

fn markdown(meeting: &Meeting, transcript: &Transcript) -> String {
    let mut out = format!(
        "# {}\n\n{}\n\n",
        title(meeting),
        details(meeting, transcript)
    );
    let items = timeline(meeting, transcript);
    for item in &items {
        match item {
            Item::Line(segment) => out.push_str(&format!(
                "**{} · {}** {}\n\n",
                clock(segment.start),
                speaker(segment.speaker),
                segment.text
            )),
            Item::Mark { at } => out.push_str(&format!("★ **{} · Marked moment**\n\n", clock(*at))),
            Item::Pause { at, seconds } => out.push_str(&format!(
                "⏸ **{} · Paused for {}**\n\n",
                clock(*at),
                length(*seconds)
            )),
        }
    }
    out.push_str(&ending(transcript));
    out
}

fn plain_text(meeting: &Meeting, transcript: &Transcript) -> String {
    let mut out = text_for_summary(meeting, transcript);
    out.push('\n');
    out.push_str(&ending(transcript));
    out
}

/// The plain text transcript without the note about echo lines.
pub(crate) fn text_for_summary(meeting: &Meeting, transcript: &Transcript) -> String {
    let mut out = format!("{}\n{}\n\n", title(meeting), details(meeting, transcript));
    let items = timeline(meeting, transcript);
    for item in &items {
        match item {
            Item::Line(segment) => out.push_str(&format!(
                "{}  {:<4}  {}\n",
                clock(segment.start),
                speaker(segment.speaker),
                segment.text
            )),
            Item::Mark { at } => out.push_str(&format!("★ {}  Marked moment\n", clock(*at))),
            Item::Pause { at, seconds } => out.push_str(&format!(
                "⏸ {}  Paused for {}\n",
                clock(*at),
                length(*seconds)
            )),
        }
    }
    out
}

/// One or more subtitles per line, each at most two short lines. A long line's
/// time is shared between its subtitles by length.
fn subtitles(transcript: &Transcript) -> String {
    let mut out = String::new();
    let mut number = 0;
    for segment in transcript.segments.iter().filter(|segment| !segment.echo) {
        let label = speaker(segment.speaker);
        let rows = wrap(&format!("{label}: {}", segment.text), SUBTITLE_LINE_CHARS);
        let pieces: Vec<Vec<String>> = rows
            .chunks(SUBTITLE_LINES)
            .enumerate()
            .map(|(index, rows)| {
                let mut rows = rows.to_vec();
                if index > 0 {
                    rows[0] = format!("{label}: {}", rows[0]);
                }
                rows
            })
            .collect();
        let weights: Vec<usize> = pieces
            .iter()
            .map(|rows| {
                rows.iter()
                    .map(|row| row.chars().count())
                    .sum::<usize>()
                    .max(1)
            })
            .collect();
        let total: usize = weights.iter().sum();
        let end = segment.end.max(segment.start + MIN_SUBTITLE_SECONDS);
        let mut start = segment.start;
        for (rows, weight) in pieces.iter().zip(&weights) {
            let stop = start + (end - segment.start) * *weight as f64 / total as f64;
            number += 1;
            out.push_str(&format!(
                "{number}\n{} --> {}\n{}\n\n",
                srt_time(start),
                srt_time(stop),
                rows.join("\n")
            ));
            start = stop;
        }
    }
    out
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in text.split_whitespace() {
        if !row.is_empty() && row.chars().count() + 1 + word.chars().count() > width {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push(' ');
        }
        row.push_str(word);
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

pub(crate) fn title(meeting: &Meeting) -> String {
    match meeting.title.trim() {
        "" => format!("Meeting · {}", when(meeting)),
        title => title.to_string(),
    }
}

fn when(meeting: &Meeting) -> String {
    meeting
        .recording
        .started_at
        .format("%a %-d %b %Y, %H:%M")
        .to_string()
}

/// For example `Wed 4 Nov 2026, 14:30 · 47 min · Russian + English`.
pub(crate) fn details(meeting: &Meeting, transcript: &Transcript) -> String {
    let mut parts = vec![when(meeting), length(meeting.recording.duration_seconds)];
    if let Some(languages) = languages(transcript) {
        parts.push(languages);
    }
    parts.join(" · ")
}

fn languages(transcript: &Transcript) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for segment in transcript.segments.iter().filter(|segment| !segment.echo) {
        if !segment.language.is_empty() {
            *counts.entry(segment.language.as_str()).or_default() += 1;
        }
    }
    let mut languages: Vec<(&str, usize)> = counts.into_iter().collect();
    languages.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let names: Vec<String> = languages
        .into_iter()
        .map(|(code, _)| match code {
            "ru" => "Russian".to_string(),
            "en" => "English".to_string(),
            other => other.to_string(),
        })
        .collect();
    (!names.is_empty()).then(|| names.join(" + "))
}

fn ending(transcript: &Transcript) -> String {
    let lines = transcript
        .segments
        .iter()
        .filter(|segment| !segment.echo)
        .count();
    let echo = transcript.echo_lines();
    let mut out = String::new();
    if lines == 0 {
        out.push_str("No speech was found.\n");
    }
    match echo {
        0 => {}
        1 => out.push_str("1 line that both tracks heard was left out of Me.\n"),
        n => out.push_str(&format!(
            "{n} lines that both tracks heard were left out of Me.\n"
        )),
    }
    out
}

fn speaker(track: Track) -> &'static str {
    match track {
        Track::Me => "Me",
        Track::Them => "Them",
    }
}

fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        total / 60 % 60,
        total % 60
    )
}

fn srt_time(seconds: f64) -> String {
    let millis = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        millis / 3_600_000,
        millis / 60_000 % 60,
        millis / 1000 % 60,
        millis % 1000
    )
}

/// `45 s`, `47 min` or `1 h 12 min`.
fn length(seconds: f64) -> String {
    if seconds < 59.5 {
        return format!("{} s", seconds.max(0.0).round() as u64);
    }
    let minutes = (seconds / 60.0).round() as u64;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::{Mark, Pause, Settings};
    use crate::testing::temp_folder;
    use crate::transcript::{Line, TrackText};

    fn meeting(title: &str) -> Meeting {
        let started_at =
            chrono::DateTime::parse_from_rfc3339("2026-11-04T14:30:05+03:00").expect("valid time");
        let mut meeting = Meeting::new(title.into(), started_at, Settings::default());
        meeting.recording.duration_seconds = 47.0 * 60.0 + 10.0;
        meeting.marks.push(Mark { at_seconds: 870.0 });
        meeting.pauses.push(Pause {
            at_seconds: 600.0,
            duration_seconds: 300.0,
        });
        meeting
    }

    fn line(start: f64, end: f64, text: &str) -> Line {
        Line {
            start,
            end,
            text: text.into(),
        }
    }

    fn transcript() -> Transcript {
        Transcript::merge(
            TrackText {
                language: "ru".into(),
                lines: vec![
                    line(6.0, 9.0, "Да, слышу. Let's start with the release."),
                    line(4.2, 6.5, "Hi, can you hear me?"),
                    line(871.0, 875.0, "Не уверен, давай решим после demo."),
                ],
            },
            TrackText {
                language: "en".into(),
                lines: vec![line(4.0, 6.3, "Hi, can you hear me?")],
            },
        )
    }

    #[test]
    fn markdown_has_the_header_lines_marks_and_pauses_in_time_order() {
        let expected = "\
# Product sync

Wed 4 Nov 2026, 14:30 · 47 min · Russian + English

**00:00:04 · Them** Hi, can you hear me?

**00:00:06 · Me** Да, слышу. Let's start with the release.

⏸ **00:10:00 · Paused for 5 min**

★ **00:14:30 · Marked moment**

**00:14:31 · Me** Не уверен, давай решим после demo.

1 line that both tracks heard was left out of Me.
";
        assert_eq!(markdown(&meeting("Product sync"), &transcript()), expected);
    }

    #[test]
    fn plain_text_is_the_same_without_markdown() {
        let expected = "\
Meeting · Wed 4 Nov 2026, 14:30
Wed 4 Nov 2026, 14:30 · 47 min · Russian + English

00:00:04  Them  Hi, can you hear me?
00:00:06  Me    Да, слышу. Let's start with the release.
⏸ 00:10:00  Paused for 5 min
★ 00:14:30  Marked moment
00:14:31  Me    Не уверен, давай решим после demo.

1 line that both tracks heard was left out of Me.
";
        assert_eq!(plain_text(&meeting("  "), &transcript()), expected);
    }

    #[test]
    fn subtitles_leave_out_echo() {
        let expected = "\
1
00:00:04,000 --> 00:00:06,300
Them: Hi, can you hear me?

2
00:00:06,000 --> 00:00:09,000
Me: Да, слышу. Let's start with the
release.

3
00:14:31,000 --> 00:14:35,000
Me: Не уверен, давай решим после demo.

";
        assert_eq!(subtitles(&transcript()), expected);
    }

    #[test]
    fn a_long_line_becomes_several_subtitles_sharing_its_time() {
        let text = "раз ".repeat(60);
        let transcript = Transcript::merge(
            TrackText {
                language: "ru".into(),
                lines: vec![line(10.0, 40.0, text.trim())],
            },
            TrackText::default(),
        );
        let srt = subtitles(&transcript);
        let cues: Vec<&str> = srt.split("\n\n").filter(|cue| !cue.is_empty()).collect();
        assert!(cues.len() > 1);
        for cue in &cues {
            let rows: Vec<&str> = cue.lines().skip(2).collect();
            assert!(rows.len() <= SUBTITLE_LINES);
            assert!(rows[0].starts_with("Me: "));
        }
        assert!(cues[0].contains("00:00:10,000 --> "));
        assert!(
            cues.last()
                .is_some_and(|cue| cue.contains(" --> 00:00:40,000"))
        );
    }

    #[test]
    fn an_empty_transcript_says_so() {
        let empty = Transcript::merge(TrackText::default(), TrackText::default());
        let text = plain_text(&meeting("Sync"), &empty);
        assert!(text.ends_with("No speech was found.\n"));
        assert_eq!(subtitles(&empty), "");
    }

    #[test]
    fn lengths_read_naturally() {
        assert_eq!(length(45.0), "45 s");
        assert_eq!(length(47.0 * 60.0), "47 min");
        assert_eq!(length(3600.0), "1 h");
        assert_eq!(length(72.0 * 60.0 + 20.0), "1 h 12 min");
    }

    #[test]
    fn writes_all_three_files() {
        let folder = temp_folder("export-files");
        export(&folder, &meeting("Sync"), &transcript()).expect("export");
        for file in [MARKDOWN_FILE, TEXT_FILE, SUBTITLES_FILE] {
            assert!(folder.join(file).exists(), "{file}");
        }
    }
}

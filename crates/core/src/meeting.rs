//! The meeting folder and its `meeting.json`.
//!
//! `format` changes only for changes old readers would get wrong (a renamed or
//! removed field, a new meaning). New optional fields keep the same format.
//! Until v0.1 ships, format 1 may still change freely.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::files::write_atomically;
use crate::recording::{Track, TrackStats};

pub const FORMAT: u64 = 1;
pub const MEETING_FILE: &str = "meeting.json";
pub const VIDEO_FILE: &str = "screen.mp4";

const JOINED_TEMP_FILE: &str = ".screen-joined.mp4";
/// Keeps the folder name well under the 255-byte limit, even with the date and a " (2)".
const MAX_TITLE_BYTES: usize = 120;
const MAX_SAME_NAME: u32 = 999;

#[derive(Debug, thiserror::Error)]
pub enum MeetingError {
    #[error("could not read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not a valid meeting file: {source}", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "{} needs a newer Debrief (meeting format {found}; this version reads up to {FORMAT})",
        path.display()
    )]
    Newer { path: PathBuf, found: u64 },
    #[error("could not join the video parts: {0}")]
    JoinVideo(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meeting {
    format: u64,
    pub app_version: String,
    /// Can be empty; the folder name and the window then use the date and time.
    pub title: String,
    pub recording: Recording,
    pub settings: Settings,
    pub tracks: AudioTracks,
    pub video: Option<VideoFile>,
    #[serde(default)]
    pub marks: Vec<Mark>,
    #[serde(default)]
    pub pauses: Vec<Pause>,
    /// A step that is not listed is pending.
    #[serde(default)]
    pub steps: BTreeMap<Step, StepState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub started_at: DateTime<FixedOffset>,
    /// `None` while recording, or when the recording was cut off by a crash.
    pub stopped_at: Option<DateTime<FixedOffset>>,
    pub duration_seconds: f64,
    pub error: Option<String>,
    /// When the storage cleanup deleted the audio and video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_deleted_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub language: Language,
    /// Write a summary after the transcript.
    #[serde(default)]
    pub summary: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    /// Whisper detects one language per track from its first 30 seconds.
    #[serde(rename = "auto")]
    Auto,
    #[default]
    #[serde(rename = "ru")]
    Russian,
    #[serde(rename = "en")]
    English,
}

impl Language {
    /// The code Whisper uses.
    pub fn code(self) -> &'static str {
        match self {
            Language::Auto => "auto",
            Language::Russian => "ru",
            Language::English => "en",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown language `{0}`; use auto, ru or en")]
pub struct UnknownLanguage(String);

impl FromStr for Language {
    type Err = UnknownLanguage;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "auto" => Ok(Language::Auto),
            "ru" => Ok(Language::Russian),
            "en" => Ok(Language::English),
            other => Err(UnknownLanguage(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioTracks {
    pub me: AudioTrack,
    pub them: AudioTrack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioTrack {
    /// The WAV file now; a compressed file may replace it later.
    pub file: String,
    /// The microphone, or the app or apps the system audio came from.
    pub source: String,
    /// Diagnostics from the recorder; not part of the format.
    #[serde(default)]
    pub stats: Option<TrackStats>,
}

impl AudioTrack {
    fn new(track: Track) -> Self {
        Self {
            file: track.file_name().to_string(),
            source: String::new(),
            stats: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoFile {
    pub file: String,
    /// The screen or window that was recorded.
    pub source: String,
    pub fps: u32,
    pub bytes: Option<u64>,
    /// One per stretch between pauses, until they are joined into `file`. Still
    /// listed after a failed join or a crash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<VideoPart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoPart {
    pub file: String,
    /// Where the part starts in the recorded audio.
    pub at_seconds: f64,
}

pub fn video_part_name(number: usize) -> String {
    format!("screen-{number}.mp4")
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    /// Position in the recorded audio.
    pub at_seconds: f64,
}

/// Paused time is not in the audio; this keeps the real clock time recoverable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pause {
    /// Position in the recorded audio where the pause happened.
    pub at_seconds: f64,
    pub duration_seconds: f64,
}

/// Processing steps in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Each track on its own, converted to 16 kHz in memory.
    Transcribe,
    /// Merge Me and Them by time and find echo lines.
    Merge,
    /// The transcript files.
    Export,
    Summary,
    /// The WAV tracks to m4a.
    Compress,
}

impl Step {
    pub const ALL: [Step; 5] = [
        Step::Transcribe,
        Step::Merge,
        Step::Export,
        Step::Summary,
        Step::Compress,
    ];
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StepState {
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<FixedOffset>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<FixedOffset>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The model the step used, such as `large-v3-turbo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Pending,
    Running,
    Done,
    Skipped,
    Failed,
}

impl Meeting {
    pub fn new(title: String, started_at: DateTime<FixedOffset>, settings: Settings) -> Self {
        Self {
            format: FORMAT,
            app_version: crate::VERSION.to_string(),
            title,
            recording: Recording {
                started_at,
                stopped_at: None,
                duration_seconds: 0.0,
                error: None,
                files_deleted_at: None,
            },
            settings,
            tracks: AudioTracks {
                me: AudioTrack::new(Track::Me),
                them: AudioTrack::new(Track::Them),
            },
            video: None,
            marks: Vec::new(),
            pauses: Vec::new(),
            steps: BTreeMap::new(),
        }
    }

    pub fn load(folder: &Path) -> Result<Self, MeetingError> {
        let path = folder.join(MEETING_FILE);
        let text = fs::read_to_string(&path).map_err(|source| MeetingError::Read {
            path: path.clone(),
            source,
        })?;
        let invalid = |source| MeetingError::Invalid {
            path: path.clone(),
            source,
        };
        let value: serde_json::Value = serde_json::from_str(&text).map_err(invalid)?;
        let format = value
            .get("format")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| invalid(serde::de::Error::missing_field("format")))?;
        if format > FORMAT {
            return Err(MeetingError::Newer {
                path,
                found: format,
            });
        }
        serde_json::from_value(value).map_err(invalid)
    }

    pub fn save(&self, folder: &Path) -> Result<(), MeetingError> {
        let path = folder.join(MEETING_FILE);
        let write_error = |source| MeetingError::Write {
            path: path.clone(),
            source,
        };
        let mut json = serde_json::to_vec_pretty(self).map_err(|e| write_error(e.into()))?;
        json.push(b'\n');
        write_atomically(&path, &json).map_err(write_error)
    }

    pub fn status(&self, step: Step) -> StepStatus {
        self.steps
            .get(&step)
            .map_or(StepStatus::Pending, |state| state.status)
    }

    /// Where processing starts or resumes. A step left running by a crash or quit
    /// counts as not done.
    pub fn next_step(&self) -> Option<Step> {
        Step::ALL
            .into_iter()
            .find(|&step| !matches!(self.status(step), StepStatus::Done | StepStatus::Skipped))
    }
}

/// Turns the video parts into `video.file`: renames a single part, or joins
/// several with `join(parts, output)`. When the join fails, the parts stay.
pub fn finish_video<E>(
    folder: &Path,
    meeting: &mut Meeting,
    join: impl FnOnce(&[(PathBuf, f64)], &Path) -> Result<(), E>,
) -> Result<(), MeetingError>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let Some(video) = &mut meeting.video else {
        return Ok(());
    };
    if video.parts.is_empty() {
        return Ok(());
    }
    let target = folder.join(&video.file);
    let write_error = |path: &Path, source| MeetingError::Write {
        path: path.to_path_buf(),
        source,
    };
    let parts: Vec<(PathBuf, f64)> = video
        .parts
        .iter()
        .map(|part| (folder.join(&part.file), part.at_seconds))
        .filter(|(path, _)| path.exists())
        .collect();
    match parts.as_slice() {
        [] => {}
        [(only, _)] => fs::rename(only, &target).map_err(|source| write_error(only, source))?,
        _ => {
            let temp = folder.join(JOINED_TEMP_FILE);
            let _ = fs::remove_file(&temp);
            if let Err(error) = join(&parts, &temp) {
                let _ = fs::remove_file(&temp);
                return Err(MeetingError::JoinVideo(Box::new(error)));
            }
            fs::rename(&temp, &target).map_err(|source| write_error(&target, source))?;
            for (path, _) in &parts {
                let _ = fs::remove_file(path);
            }
        }
    }
    video.parts.clear();
    video.bytes = fs::metadata(&target).map(|meta| meta.len()).ok();
    Ok(())
}

/// `~/Documents/Debrief`, where new meeting folders go.
pub fn default_library() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Documents").join(crate::APP_NAME))
}

/// For example `2026-11-04 14-30 Product sync`.
pub fn folder_name(started_at: &DateTime<FixedOffset>, title: &str) -> String {
    let stamp = started_at.format("%Y-%m-%d %H-%M").to_string();
    let title = clean_title(title);
    if title.is_empty() {
        stamp
    } else {
        format!("{stamp} {title}")
    }
}

/// Creates `library/name`, or `library/name (2)` and so on when it is taken.
pub fn create_folder(library: &Path, name: &str) -> Result<PathBuf, MeetingError> {
    let write_error = |path: &Path, source| MeetingError::Write {
        path: path.to_path_buf(),
        source,
    };
    fs::create_dir_all(library).map_err(|source| write_error(library, source))?;
    for n in 1..=MAX_SAME_NAME {
        let folder = match n {
            1 => library.join(name),
            n => library.join(format!("{name} ({n})")),
        };
        match fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(write_error(&folder, error)),
        }
    }
    Err(write_error(
        &library.join(name),
        io::ErrorKind::AlreadyExists.into(),
    ))
}

/// Characters that macOS, Windows or Linux don't allow in names, or that Finder
/// shows differently, become spaces. A leading dot would hide the folder.
fn clean_title(title: &str) -> String {
    let replaced: String = title
        .chars()
        .map(|c| {
            if c.is_control() || r#"/\:*?"<>|"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let words = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = words.trim_start_matches(['.', ' ']).to_string();
    if title.len() > MAX_TITLE_BYTES {
        let mut end = MAX_TITLE_BYTES;
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        title.truncate(end);
    }
    title.trim_end_matches(['.', ' ']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_folder;

    fn time(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).expect("valid time")
    }

    fn sample() -> Meeting {
        let mut meeting = Meeting::new(
            "Product sync".into(),
            time("2026-11-04T14:30:05+03:00"),
            Settings {
                language: Language::Russian,
                summary: true,
            },
        );
        meeting.tracks.me.source = "MacBook Pro Microphone".into();
        meeting.tracks.them.source = "Zoom (us.zoom.xos)".into();
        meeting.marks.push(Mark { at_seconds: 870.0 });
        meeting
    }

    #[test]
    fn folder_name_has_date_time_and_title() {
        let at = time("2026-11-04T14:30:59+03:00");
        assert_eq!(
            folder_name(&at, "Product sync"),
            "2026-11-04 14-30 Product sync"
        );
        assert_eq!(folder_name(&at, "  "), "2026-11-04 14-30");
    }

    #[test]
    fn folder_name_drops_characters_file_systems_reject() {
        let at = time("2026-11-04T14:30:00+03:00");
        assert_eq!(
            folder_name(&at, "Q&A: roadmap / plan?\n"),
            "2026-11-04 14-30 Q&A roadmap plan"
        );
        assert_eq!(folder_name(&at, "..hidden."), "2026-11-04 14-30 hidden");
    }

    #[test]
    fn long_titles_are_cut_between_characters() {
        let at = time("2026-11-04T14:30:00+03:00");
        let name = folder_name(&at, &"планёрка ".repeat(30));
        let title = name.trim_start_matches("2026-11-04 14-30 ");
        assert!(title.len() <= MAX_TITLE_BYTES);
        assert!(title.starts_with("планёрка планёрка"));
        assert!(!title.ends_with(' '));
    }

    #[test]
    fn taken_folder_names_get_a_number() {
        let library = temp_folder("meeting-names");
        let first = create_folder(&library, "2026-11-04 14-30 Sync").expect("create");
        let second = create_folder(&library, "2026-11-04 14-30 Sync").expect("create");
        assert_eq!(first, library.join("2026-11-04 14-30 Sync"));
        assert_eq!(second, library.join("2026-11-04 14-30 Sync (2)"));
    }

    #[test]
    fn saves_and_loads_the_same_meeting() {
        let folder = temp_folder("meeting-roundtrip");
        let mut meeting = sample();
        meeting.steps.insert(
            Step::Transcribe,
            StepState {
                status: StepStatus::Done,
                model: Some("large-v3-turbo".into()),
                ..StepState::default()
            },
        );
        meeting.save(&folder).expect("save");
        assert!(!folder.join(".meeting.json.tmp").exists());
        assert_eq!(Meeting::load(&folder).expect("load"), meeting);
    }

    #[test]
    fn loads_a_format_1_file_written_by_hand() {
        let folder = temp_folder("meeting-v1");
        let json = r#"{
          "format": 1,
          "app_version": "0.1.0",
          "title": "Product sync",
          "recording": {
            "started_at": "2026-11-04T14:30:05+03:00",
            "stopped_at": "2026-11-04T15:17:12+03:00",
            "duration_seconds": 2827.0,
            "error": null
          },
          "settings": { "language": "ru" },
          "tracks": {
            "me": { "file": "me.wav", "source": "MacBook Pro Microphone" },
            "them": { "file": "them.m4a", "source": "Zoom (us.zoom.xos)", "stats": null }
          },
          "video": { "file": "screen.mp4", "source": "Display 1", "fps": 5, "bytes": 157000000 },
          "marks": [{ "at_seconds": 870.0 }],
          "steps": {
            "transcribe": { "status": "done", "finished_at": "2026-11-04T15:17:20+03:00" },
            "merge": { "status": "running" }
          },
          "added_later": true
        }"#;
        fs::write(folder.join(MEETING_FILE), json).expect("write");
        let meeting = Meeting::load(&folder).expect("load");
        assert_eq!(meeting.settings.language, Language::Russian);
        assert_eq!(meeting.tracks.them.file, "them.m4a");
        assert_eq!(meeting.marks, vec![Mark { at_seconds: 870.0 }]);
        assert_eq!(meeting.status(Step::Transcribe), StepStatus::Done);
        assert_eq!(meeting.next_step(), Some(Step::Merge));
    }

    #[test]
    fn refuses_files_from_a_newer_version() {
        let folder = temp_folder("meeting-newer");
        fs::write(folder.join(MEETING_FILE), r#"{"format": 2}"#).expect("write");
        assert!(matches!(
            Meeting::load(&folder),
            Err(MeetingError::Newer { found: 2, .. })
        ));
        fs::write(folder.join(MEETING_FILE), r#"{"title": "x"}"#).expect("write");
        assert!(matches!(
            Meeting::load(&folder),
            Err(MeetingError::Invalid { .. })
        ));
    }

    #[test]
    fn processing_resumes_at_the_first_unfinished_step() {
        let mut meeting = sample();
        assert_eq!(meeting.next_step(), Some(Step::Transcribe));
        for (step, status) in [
            (Step::Transcribe, StepStatus::Done),
            (Step::Merge, StepStatus::Running),
        ] {
            meeting.steps.insert(
                step,
                StepState {
                    status,
                    ..StepState::default()
                },
            );
        }
        assert_eq!(meeting.next_step(), Some(Step::Merge));
        for (step, status) in [
            (Step::Merge, StepStatus::Done),
            (Step::Export, StepStatus::Done),
            (Step::Summary, StepStatus::Skipped),
            (Step::Compress, StepStatus::Done),
        ] {
            meeting.steps.insert(
                step,
                StepState {
                    status,
                    ..StepState::default()
                },
            );
        }
        assert_eq!(meeting.next_step(), None);
    }

    #[test]
    fn steps_are_saved_in_processing_order() {
        let mut meeting = sample();
        for step in [Step::Compress, Step::Transcribe] {
            meeting.steps.insert(step, StepState::default());
        }
        let json = serde_json::to_string(&meeting).expect("serialize");
        let transcribe = json.find("\"transcribe\"").expect("transcribe");
        let compress = json.find("\"compress\"").expect("compress");
        assert!(transcribe < compress);
    }

    fn with_parts(folder: &Path, count: usize) -> Meeting {
        let mut meeting = sample();
        let parts = (1..=count)
            .map(|n| {
                fs::write(folder.join(video_part_name(n)), format!("part {n}")).expect("write");
                VideoPart {
                    file: video_part_name(n),
                    at_seconds: (n - 1) as f64 * 60.0,
                }
            })
            .collect();
        meeting.video = Some(VideoFile {
            file: VIDEO_FILE.into(),
            source: "Display 1".into(),
            fps: 5,
            bytes: None,
            parts,
        });
        meeting
    }

    #[test]
    fn a_single_video_part_is_renamed() {
        let folder = temp_folder("video-single");
        let mut meeting = with_parts(&folder, 1);
        finish_video(&folder, &mut meeting, |_, _| -> Result<(), io::Error> {
            panic!("nothing to join")
        })
        .expect("finish");
        let video = meeting.video.expect("video");
        assert!(video.parts.is_empty());
        assert_eq!(video.bytes, Some(6));
        assert_eq!(
            fs::read_to_string(folder.join(VIDEO_FILE)).expect("read"),
            "part 1"
        );
    }

    #[test]
    fn several_video_parts_are_joined_in_order() {
        let folder = temp_folder("video-join");
        let mut meeting = with_parts(&folder, 3);
        finish_video(&folder, &mut meeting, |parts, output| {
            let starts: Vec<f64> = parts.iter().map(|(_, at)| *at).collect();
            assert_eq!(starts, vec![0.0, 60.0, 120.0]);
            fs::write(output, "joined")
        })
        .expect("finish");
        assert!(meeting.video.expect("video").parts.is_empty());
        assert_eq!(
            fs::read_to_string(folder.join(VIDEO_FILE)).expect("read"),
            "joined"
        );
        assert!(!folder.join(video_part_name(2)).exists());
    }

    #[test]
    fn video_parts_stay_when_the_join_fails() {
        let folder = temp_folder("video-join-fails");
        let mut meeting = with_parts(&folder, 2);
        let result = finish_video(&folder, &mut meeting, |_, _| {
            Err(io::Error::other("export failed"))
        });
        assert!(matches!(result, Err(MeetingError::JoinVideo(_))));
        assert_eq!(meeting.video.expect("video").parts.len(), 2);
        assert!(folder.join(video_part_name(1)).exists());
        assert!(!folder.join(VIDEO_FILE).exists());
    }

    #[test]
    fn parses_language_codes() {
        assert_eq!("ru".parse::<Language>().ok(), Some(Language::Russian));
        assert_eq!("auto".parse::<Language>().ok(), Some(Language::Auto));
        assert!("russian".parse::<Language>().is_err());
    }
}

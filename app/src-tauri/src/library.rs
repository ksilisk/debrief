//! The meetings in the meetings folder (`~/Documents/Debrief` unless Settings
//! says otherwise): the list for the sidebar and the menu bar, and one
//! meeting's details for the Meeting screen.

use std::fs;
use std::path::{Path, PathBuf};

use debrief_core::export::{MARKDOWN_FILE, SUBTITLES_FILE, TEXT_FILE};
use debrief_core::meeting::{self, MEETING_FILE, Meeting, Step, StepStatus};
use debrief_core::recording::Track;
use debrief_core::session::is_recording;
use debrief_core::summary;
use debrief_core::transcript::{TRANSCRIPT_FILE, Transcript};
use debrief_summarize::config;
use serde::Serialize;

/// Where a meeting stands, for the chips in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Recording,
    Processing,
    Waiting,
    Ready,
    TranscriptOnly,
    NotProcessed,
    /// The app quit or crashed while recording; processing repairs it first.
    CutOff,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub folder: String,
    pub title: String,
    /// RFC 3339 with the recording's own offset.
    pub started_at: String,
    pub duration_seconds: f64,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    #[serde(flatten)]
    pub item: Item,
    /// The recorded screen or window with its frame rate; `None` for audio only.
    pub video: Option<String>,
    /// The meeting's language setting: `ru`, `en` or `auto`.
    pub language: String,
    /// The languages Whisper heard, such as `["ru", "en"]`.
    pub heard: Vec<String>,
    pub steps: Vec<StepView>,
    pub summaries_on: bool,
    /// The summary in Markdown, when there is one.
    pub summary: Option<String>,
    pub segments: Vec<SegmentView>,
    pub marks: Vec<f64>,
    pub files: Vec<FileView>,
    /// RFC 3339; when the storage cleanup deleted the audio and video.
    pub files_deleted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    pub step: Step,
    pub status: StepStatus,
    pub error: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SegmentView {
    pub start: f64,
    pub speaker: Track,
    pub text: String,
    pub echo: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileView {
    pub name: String,
    pub bytes: u64,
}

/// The steps the screens show, in the order they run.
pub const SHOWN_STEPS: [Step; 4] = [Step::Transcribe, Step::Merge, Step::Export, Step::Summary];

/// Files in the order the Files tab lists them; anything else comes after.
const FILE_ORDER: [&str; 10] = [
    summary::MARKDOWN_FILE,
    summary::TEXT_FILE,
    MARKDOWN_FILE,
    TEXT_FILE,
    SUBTITLES_FILE,
    "me.wav",
    "them.wav",
    meeting::VIDEO_FILE,
    MEETING_FILE,
    TRANSCRIPT_FILE,
];

/// Newest first. `queued` tells which folders are being processed or waiting.
pub fn list(queued: &dyn Fn(&Path) -> Option<Status>) -> Vec<Item> {
    let Ok(library) = config::library() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&library) else {
        return Vec::new();
    };
    let mut items: Vec<(Meeting, Item)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|folder| folder.join(MEETING_FILE).is_file())
        .filter_map(|folder| {
            let meeting = Meeting::load(&folder).ok()?;
            let item = item(&folder, &meeting, queued);
            Some((meeting, item))
        })
        .collect();
    items.sort_by_key(|(meeting, _)| std::cmp::Reverse(meeting.recording.started_at));
    items.into_iter().map(|(_, item)| item).collect()
}

pub fn details(folder: &Path, queued: &dyn Fn(&Path) -> Option<Status>) -> Result<Details, String> {
    let meeting = Meeting::load(folder).map_err(|error| error.to_string())?;
    let transcript = Transcript::load(folder).ok();
    let mut heard: Vec<String> = Vec::new();
    for segment in transcript.iter().flat_map(|t| &t.segments) {
        if !segment.language.is_empty() && !heard.contains(&segment.language) {
            heard.push(segment.language.clone());
        }
    }
    Ok(Details {
        item: item(folder, &meeting, queued),
        video: meeting.video.as_ref().map(|video| video.source.clone()),
        language: meeting.settings.language.code().to_string(),
        heard,
        steps: SHOWN_STEPS
            .iter()
            .map(|&step| {
                let state = meeting.steps.get(&step).cloned().unwrap_or_default();
                StepView {
                    step,
                    status: state.status,
                    error: state.error,
                    model: state.model,
                }
            })
            .collect(),
        summaries_on: meeting.settings.summary,
        summary: fs::read_to_string(folder.join(summary::MARKDOWN_FILE)).ok(),
        segments: transcript
            .map(|transcript| {
                transcript
                    .segments
                    .into_iter()
                    .map(|segment| SegmentView {
                        start: segment.start,
                        speaker: segment.speaker,
                        text: segment.text,
                        echo: segment.echo,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        marks: meeting.marks.iter().map(|mark| mark.at_seconds).collect(),
        files: files(folder),
        files_deleted_at: meeting.recording.files_deleted_at.map(|at| at.to_rfc3339()),
    })
}

/// The title the app shows: the meeting's own, or the folder name.
pub fn title(folder: &Path, meeting: &Meeting) -> String {
    if meeting.title.trim().is_empty() {
        folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        meeting.title.clone()
    }
}

fn item(folder: &Path, meeting: &Meeting, queued: &dyn Fn(&Path) -> Option<Status>) -> Item {
    Item {
        folder: folder.display().to_string(),
        title: title(folder, meeting),
        started_at: meeting.recording.started_at.to_rfc3339(),
        duration_seconds: meeting.recording.duration_seconds,
        status: status(folder, meeting, queued),
    }
}

fn status(folder: &Path, meeting: &Meeting, queued: &dyn Fn(&Path) -> Option<Status>) -> Status {
    if is_recording(folder) {
        return Status::Recording;
    }
    if let Some(status) = queued(folder) {
        return status;
    }
    if meeting.recording.stopped_at.is_none() {
        return Status::CutOff;
    }
    if meeting.status(Step::Export) == StepStatus::Done {
        return if meeting.status(Step::Summary) == StepStatus::Done {
            Status::Ready
        } else {
            Status::TranscriptOnly
        };
    }
    if meeting
        .steps
        .values()
        .any(|state| state.status == StepStatus::Failed)
    {
        Status::Failed
    } else {
        Status::NotProcessed
    }
}

fn files(folder: &Path) -> Vec<FileView> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<FileView> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = entry.metadata().ok()?.len();
            (!name.starts_with('.')).then_some(FileView { name, bytes })
        })
        .collect();
    let rank = |name: &str| {
        FILE_ORDER
            .iter()
            .position(|&known| known == name)
            .unwrap_or(FILE_ORDER.len())
    };
    files.sort_by(|a, b| {
        rank(&a.name)
            .cmp(&rank(&b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    files
}

/// A folder the window may ask about: inside the meetings folder, with a meeting in it.
pub fn checked_folder(folder: &str) -> Result<PathBuf, String> {
    let library = config::library().map_err(|error| error.to_string())?;
    let library = library.canonicalize().unwrap_or(library);
    let folder = Path::new(folder)
        .canonicalize()
        .map_err(|error| format!("Could not find {folder}: {error}"))?;
    if !folder.starts_with(&library) || !folder.join(MEETING_FILE).is_file() {
        return Err(format!("{} is not a meeting folder", folder.display()));
    }
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use debrief_core::meeting::{Settings, StepState};

    use super::*;

    fn stopped() -> Meeting {
        let started_at =
            chrono::DateTime::parse_from_rfc3339("2026-11-08T22:48:00+03:00").expect("valid time");
        let mut meeting = Meeting::new(String::new(), started_at, Settings::default());
        meeting.recording.stopped_at = Some(started_at);
        meeting
    }

    fn with(mut meeting: Meeting, steps: &[(Step, StepStatus)]) -> Meeting {
        for &(step, status) in steps {
            meeting.steps.insert(
                step,
                StepState {
                    status,
                    ..StepState::default()
                },
            );
        }
        meeting
    }

    fn status_of(meeting: &Meeting) -> Status {
        status(Path::new("/nonexistent/meeting"), meeting, &|_| None)
    }

    #[test]
    fn status_follows_the_steps() {
        assert_eq!(status_of(&stopped()), Status::NotProcessed);
        let mut cut_off = stopped();
        cut_off.recording.stopped_at = None;
        assert_eq!(status_of(&cut_off), Status::CutOff);
        let failed = with(stopped(), &[(Step::Transcribe, StepStatus::Failed)]);
        assert_eq!(status_of(&failed), Status::Failed);
        let transcribed = [
            (Step::Transcribe, StepStatus::Done),
            (Step::Merge, StepStatus::Done),
            (Step::Export, StepStatus::Done),
        ];
        let no_summary = with(
            with(stopped(), &transcribed),
            &[(Step::Summary, StepStatus::Skipped)],
        );
        assert_eq!(status_of(&no_summary), Status::TranscriptOnly);
        let summary_failed = with(
            with(stopped(), &transcribed),
            &[(Step::Summary, StepStatus::Failed)],
        );
        assert_eq!(status_of(&summary_failed), Status::TranscriptOnly);
        let ready = with(
            with(stopped(), &transcribed),
            &[(Step::Summary, StepStatus::Done)],
        );
        assert_eq!(status_of(&ready), Status::Ready);
    }

    #[test]
    fn the_queue_comes_first() {
        let meeting = stopped();
        let waiting = status(Path::new("/nonexistent/meeting"), &meeting, &|_| {
            Some(Status::Waiting)
        });
        assert_eq!(waiting, Status::Waiting);
    }

    #[test]
    fn an_empty_title_shows_the_folder_name() {
        let folder = Path::new("/meetings/2026-11-08 22-48");
        assert_eq!(title(folder, &stopped()), "2026-11-08 22-48");
        let mut named = stopped();
        named.title = "Sprint planning".into();
        assert_eq!(title(folder, &named), "Sprint planning");
    }
}

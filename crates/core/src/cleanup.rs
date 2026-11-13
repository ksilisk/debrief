//! Storage cleanup: after a number of days, the audio and video of old meetings,
//! or whole meetings, are deleted. A meeting without a transcript is never
//! touched. The caller does the deleting (the app moves things to the Trash).

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, FixedOffset, TimeDelta};
use serde::{Deserialize, Serialize};

use crate::meeting::{MEETING_FILE, Meeting, MeetingError, Step, StepStatus};
use crate::session::is_recording;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delete {
    /// The audio and video; the transcript and summary stay.
    #[default]
    Recordings,
    /// The meeting folder, transcript and summary included.
    Meeting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub after_days: u32,
    pub delete: Delete,
}

/// A meeting the rule applies to now.
#[derive(Debug, Clone, PartialEq)]
pub struct Due {
    pub folder: PathBuf,
    pub meeting: Meeting,
    pub delete: Delete,
    /// The audio and video files that are still there, or the folder itself.
    pub paths: Vec<PathBuf>,
    pub bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CleanupError {
    #[error("could not delete {}: {source}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Meeting(#[from] MeetingError),
}

/// How many meetings the library holds and how much space they take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub meetings: usize,
    pub bytes: u64,
}

/// The meetings in `library` that the rule applies to, oldest first. Age counts
/// from the start of the recording. Skips meetings without a transcript, those
/// being recorded, and those `busy` says are being processed or waiting.
pub fn due(
    library: &Path,
    rule: Rule,
    now: DateTime<FixedOffset>,
    busy: &dyn Fn(&Path) -> bool,
) -> Vec<Due> {
    let oldest = now - TimeDelta::days(i64::from(rule.after_days));
    let mut due: Vec<Due> = meeting_folders(library)
        .filter_map(|folder| {
            let meeting = Meeting::load(&folder).ok()?;
            let skip = meeting.recording.started_at > oldest
                || meeting.status(Step::Export) != StepStatus::Done
                || is_recording(&folder)
                || busy(&folder);
            if skip {
                return None;
            }
            let paths = match rule.delete {
                Delete::Recordings => recordings(&folder, &meeting),
                Delete::Meeting => vec![folder.clone()],
            };
            if paths.is_empty() {
                return None;
            }
            let bytes = paths.iter().map(|path| size(path)).sum();
            Some(Due {
                folder,
                meeting,
                delete: rule.delete,
                paths,
                bytes,
            })
        })
        .collect();
    due.sort_by_key(|due| due.meeting.recording.started_at);
    due
}

/// Deletes what is due with `remove`. When some of the audio and video went,
/// `meeting.json` notes when; the rest is tried again next time.
pub fn clean(
    due: &Due,
    now: DateTime<FixedOffset>,
    remove: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Result<(), CleanupError> {
    let mut failed = None;
    let mut removed = false;
    for path in &due.paths {
        match remove(path) {
            Ok(()) => removed = true,
            Err(source) => {
                failed.get_or_insert(CleanupError::Remove {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    if removed && due.delete == Delete::Recordings {
        let mut meeting = Meeting::load(&due.folder)?;
        meeting.recording.files_deleted_at = Some(now);
        meeting.save(&due.folder)?;
    }
    failed.map_or(Ok(()), Err)
}

pub fn usage(library: &Path) -> Usage {
    meeting_folders(library).fold(Usage::default(), |usage, folder| Usage {
        meetings: usage.meetings + 1,
        bytes: usage.bytes + size(&folder),
    })
}

/// Bytes in a file, or in a folder and everything in it. Links are not followed.
pub fn size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|entry| size(&entry.path())).sum())
        .unwrap_or(0)
}

fn meeting_folders(library: &Path) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(library)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|folder| folder.join(MEETING_FILE).is_file())
}

fn recordings(folder: &Path, meeting: &Meeting) -> Vec<PathBuf> {
    let video = meeting.video.iter().flat_map(|video| {
        std::iter::once(video.file.as_str())
            .chain(video.parts.iter().map(|part| part.file.as_str()))
    });
    let mut paths: Vec<PathBuf> = meeting
        .tracks
        .recorded()
        .map(|(_, audio)| audio.file.as_str())
        .chain(video)
        .filter(|name| is_plain_name(name))
        .map(|name| folder.join(name))
        .filter(|path| path.is_file())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// `meeting.json` can be edited by hand; a name like `../x` must not lead out of the folder.
fn is_plain_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::{Settings, StepState, VIDEO_FILE, VideoFile, VideoPart};
    use crate::testing::temp_folder;

    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-12-20T12:00:00+03:00").expect("valid time")
    }

    const RECORDINGS: [&str; 4] = ["me.wav", "them.wav", VIDEO_FILE, "screen-2.mp4"];

    /// A meeting folder with audio, video, a leftover video part and a transcript.
    fn meeting_in(library: &Path, name: &str, days_old: i64, transcribed: bool) -> PathBuf {
        let folder = library.join(name);
        fs::create_dir_all(&folder).expect("create");
        let started_at = now() - TimeDelta::days(days_old);
        let mut meeting = Meeting::new(String::new(), started_at, Settings::default());
        meeting.recording.stopped_at = Some(started_at);
        meeting.video = Some(VideoFile {
            file: VIDEO_FILE.into(),
            source: "Display 1".into(),
            fps: 5,
            bytes: None,
            parts: vec![VideoPart {
                file: "screen-2.mp4".into(),
                at_seconds: 60.0,
            }],
        });
        if transcribed {
            meeting.steps.insert(
                Step::Export,
                StepState {
                    status: StepStatus::Done,
                    ..StepState::default()
                },
            );
        }
        meeting.save(&folder).expect("save");
        for file in RECORDINGS.iter().chain(&["transcript.md"]) {
            fs::write(folder.join(file), "12345").expect("write");
        }
        folder
    }

    fn rule(delete: Delete) -> Rule {
        Rule {
            after_days: 30,
            delete,
        }
    }

    fn folders(due: &[Due]) -> Vec<&Path> {
        due.iter().map(|due| due.folder.as_path()).collect()
    }

    #[test]
    fn only_old_transcribed_meetings_are_due() {
        let library = temp_folder("cleanup-due");
        let old = meeting_in(&library, "old", 40, true);
        let older = meeting_in(&library, "older", 90, true);
        meeting_in(&library, "recent", 10, true);
        meeting_in(&library, "not transcribed", 40, false);
        let busy = meeting_in(&library, "busy", 40, true);
        fs::create_dir_all(library.join("not a meeting")).expect("create");

        let due = due(&library, rule(Delete::Recordings), now(), &|folder| {
            folder == busy
        });
        assert_eq!(folders(&due), vec![older.as_path(), old.as_path()]);
        let mut paths = due[1].paths.clone();
        paths.sort();
        let mut expected: Vec<PathBuf> = RECORDINGS.iter().map(|file| old.join(file)).collect();
        expected.sort();
        assert_eq!(paths, expected);
        assert_eq!(due[1].bytes, 20);
    }

    #[test]
    fn whole_meetings_are_due_as_folders() {
        let library = temp_folder("cleanup-meeting");
        let old = meeting_in(&library, "old", 31, true);
        let due = due(&library, rule(Delete::Meeting), now(), &|_| false);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].paths, vec![old.clone()]);
        assert_eq!(due[0].bytes, size(&old));
        clean(&due[0], now(), &mut |path| fs::remove_dir_all(path)).expect("clean");
        assert!(!old.exists());
    }

    #[test]
    fn cleaning_recordings_keeps_the_text_and_notes_when() {
        let library = temp_folder("cleanup-recordings");
        let old = meeting_in(&library, "old", 40, true);
        let first = due(&library, rule(Delete::Recordings), now(), &|_| false);
        clean(&first[0], now(), &mut |path| fs::remove_file(path)).expect("clean");
        for file in RECORDINGS {
            assert!(!old.join(file).exists(), "{file} is still there");
        }
        assert!(old.join("transcript.md").exists());
        let meeting = Meeting::load(&old).expect("load");
        assert_eq!(meeting.recording.files_deleted_at, Some(now()));
        assert!(meeting.video.is_some());
        assert!(due(&library, rule(Delete::Recordings), now(), &|_| false).is_empty());
    }

    #[test]
    fn a_failed_delete_is_reported_and_the_rest_still_goes() {
        let library = temp_folder("cleanup-fails");
        let old = meeting_in(&library, "old", 40, true);
        let due = due(&library, rule(Delete::Recordings), now(), &|_| false);
        let result = clean(&due[0], now(), &mut |path| {
            if path.ends_with("me.wav") {
                Err(io::Error::other("locked"))
            } else {
                fs::remove_file(path)
            }
        });
        assert!(matches!(result, Err(CleanupError::Remove { .. })));
        assert!(old.join("me.wav").exists());
        assert!(!old.join("them.wav").exists());
        let meeting = Meeting::load(&old).expect("load");
        assert_eq!(meeting.recording.files_deleted_at, Some(now()));
    }

    #[test]
    fn file_names_cannot_lead_out_of_the_folder() {
        let library = temp_folder("cleanup-names");
        let old = meeting_in(&library, "old", 40, true);
        fs::write(library.join("outside.wav"), "keep").expect("write");
        let mut meeting = Meeting::load(&old).expect("load");
        if let Some(me) = &mut meeting.tracks.me {
            me.file = "../outside.wav".into();
        }
        if let Some(them) = &mut meeting.tracks.them {
            them.file = "/tmp/them.wav".into();
        }
        meeting.save(&old).expect("save");
        let due = due(&library, rule(Delete::Recordings), now(), &|_| false);
        assert!(
            due[0]
                .paths
                .iter()
                .all(|path| path.parent() == Some(old.as_path()))
        );
        assert_eq!(due[0].paths.len(), 2);
    }

    #[test]
    fn usage_counts_meeting_folders_only() {
        let library = temp_folder("cleanup-usage");
        let one = meeting_in(&library, "one", 1, true);
        let two = meeting_in(&library, "two", 2, false);
        fs::create_dir_all(library.join("tests")).expect("create");
        fs::write(library.join("tests/big.wav"), "0123456789").expect("write");
        assert_eq!(
            usage(&library),
            Usage {
                meetings: 2,
                bytes: size(&one) + size(&two),
            }
        );
    }
}

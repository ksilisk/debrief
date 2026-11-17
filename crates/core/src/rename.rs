//! A new title for a meeting: in `meeting.json`, on the first line of the
//! transcript and summary files, and in the folder name.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::files::write_atomically;
use crate::meeting::{self, Meeting, MeetingError};
use crate::{export, summary};

/// The files that start with the title, and what comes before it.
const TITLED_FILES: [(&str, &str); 4] = [
    (export::MARKDOWN_FILE, "# "),
    (export::TEXT_FILE, ""),
    (summary::MARKDOWN_FILE, "# "),
    (summary::TEXT_FILE, ""),
];

/// Saves the new title and returns the meeting's folder, which keeps its date
/// and time and gets the new title after them. A folder renamed in Finder, or a
/// first line changed by hand, stays as it is.
pub fn rename(folder: &Path, title: &str) -> Result<PathBuf, MeetingError> {
    let mut meeting = Meeting::load(folder)?;
    let title = title.trim();
    if meeting.title.trim() == title {
        return Ok(folder.to_path_buf());
    }
    let old = export::title(&meeting);
    meeting.title = title.to_string();
    let new = export::title(&meeting);
    for (file, before) in TITLED_FILES {
        retitle(
            &folder.join(file),
            &format!("{before}{old}\n"),
            &format!("{before}{new}\n"),
        )?;
    }
    meeting.save(folder)?;
    match meeting::retitled_name(folder, title) {
        Some(name) => meeting::move_folder(folder, &name),
        None => Ok(folder.to_path_buf()),
    }
}

fn retitle(path: &Path, old: &str, new: &str) -> Result<(), MeetingError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(MeetingError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let Some(rest) = text.strip_prefix(old) else {
        return Ok(());
    };
    write_atomically(path, format!("{new}{rest}").as_bytes()).map_err(|source| {
        MeetingError::Write {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::Settings;
    use crate::testing::temp_folder;
    use crate::transcript::{Line, TrackText, Transcript};

    fn transcript() -> Transcript {
        let text = |language: &str, start: f64, text: &str| TrackText {
            language: language.into(),
            lines: vec![Line {
                start,
                end: start + 2.0,
                text: text.into(),
            }],
        };
        Transcript::merge(text("ru", 1.0, "Привет."), text("en", 4.0, "Hi."))
    }

    fn write_files(folder: &Path, meeting: &Meeting) {
        export::export(folder, meeting, &transcript()).expect("export");
        summary::write_files(folder, meeting, &transcript(), "## Decisions\n\n- Ship it.")
            .expect("summary");
    }

    /// A meeting with its transcript and summary, as processing leaves it.
    fn processed(library: &Path, title: &str) -> PathBuf {
        let started_at =
            chrono::DateTime::parse_from_rfc3339("2026-11-04T14:30:05+03:00").expect("valid time");
        let folder = meeting::create_folder(library, &meeting::folder_name(&started_at, title))
            .expect("create");
        let meeting = Meeting::new(title.into(), started_at, Settings::default());
        meeting.save(&folder).expect("save");
        write_files(&folder, &meeting);
        folder
    }

    fn titled_files(folder: &Path) -> Vec<String> {
        TITLED_FILES
            .iter()
            .map(|(file, _)| fs::read_to_string(folder.join(file)).expect("read"))
            .collect()
    }

    fn names(library: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(library)
            .expect("list")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn renames_the_folder_and_the_title_in_every_file() {
        let library = temp_folder("rename-all");
        let folder = processed(&library, "");
        let renamed = rename(&folder, "  Sprint planning ").expect("rename");
        assert_eq!(renamed, library.join("2026-11-04 14-30 Sprint planning"));
        assert_eq!(names(&library), ["2026-11-04 14-30 Sprint planning"]);

        let meeting = Meeting::load(&renamed).expect("load");
        assert_eq!(meeting.title, "Sprint planning");
        let expected = temp_folder("rename-all-expected");
        write_files(&expected, &meeting);
        assert_eq!(titled_files(&renamed), titled_files(&expected));
    }

    #[test]
    fn an_empty_title_goes_back_to_the_date_and_time() {
        let library = temp_folder("rename-empty");
        let folder = processed(&library, "Sync");
        let renamed = rename(&folder, " ").expect("rename");
        assert_eq!(renamed, library.join("2026-11-04 14-30"));
        let markdown = fs::read_to_string(renamed.join(export::MARKDOWN_FILE)).expect("read");
        assert!(markdown.starts_with("# Meeting · Wed 4 Nov 2026, 14:30\n\n"));
    }

    #[test]
    fn the_same_title_changes_nothing() {
        let library = temp_folder("rename-same");
        let folder = processed(&library, "Sync");
        assert_eq!(rename(&folder, "Sync ").expect("rename"), folder);
        assert_eq!(names(&library), ["2026-11-04 14-30 Sync"]);
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let library = temp_folder("rename-taken");
        processed(&library, "Sync");
        let second = processed(&library, "Sync");
        let untitled = processed(&library, "");
        assert_eq!(
            rename(&untitled, "Sync").expect("rename"),
            library.join("2026-11-04 14-30 Sync (3)")
        );
        assert_eq!(
            rename(&second, "Retro").expect("rename"),
            library.join("2026-11-04 14-30 Retro")
        );
    }

    #[test]
    fn a_change_of_case_keeps_the_folder_number_free() {
        let library = temp_folder("rename-case");
        let folder = processed(&library, "sync");
        let renamed = rename(&folder, "Sync").expect("rename");
        assert_eq!(renamed, library.join("2026-11-04 14-30 Sync"));
        assert_eq!(names(&library), ["2026-11-04 14-30 Sync"]);
    }

    #[test]
    fn keeps_the_date_and_time_the_folder_has() {
        let library = temp_folder("rename-own-minute");
        let folder = processed(&library, "Sync");
        let moved = library.join("2026-11-04 14-29 meet (2)");
        fs::rename(&folder, &moved).expect("rename by hand");
        assert_eq!(
            rename(&moved, "Retro").expect("rename"),
            library.join("2026-11-04 14-29 Retro")
        );
    }

    #[test]
    fn keeps_a_folder_name_and_a_first_line_changed_by_hand() {
        let library = temp_folder("rename-by-hand");
        let folder = processed(&library, "Sync");
        let moved = library.join("Notes from Sync");
        fs::rename(&folder, &moved).expect("rename by hand");
        let own = "# My heading\n\nText.\n";
        fs::write(moved.join(summary::MARKDOWN_FILE), own).expect("write");

        assert_eq!(rename(&moved, "Retro").expect("rename"), moved);
        assert_eq!(Meeting::load(&moved).expect("load").title, "Retro");
        let read = |file| fs::read_to_string(moved.join(file)).expect("read");
        assert_eq!(read(summary::MARKDOWN_FILE), own);
        assert!(read(export::TEXT_FILE).starts_with("Retro\n"));
    }
}

use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// The error's text, or a plain sentence when the disk is full.
pub fn error_text(error: &(dyn Error + 'static)) -> String {
    let mut cause = Some(error);
    while let Some(current) = cause {
        if current
            .downcast_ref::<io::Error>()
            .is_some_and(|io| io.kind() == io::ErrorKind::StorageFull)
        {
            return "the disk is full; free up some space and try again".into();
        }
        cause = current.source();
    }
    error.to_string()
}

/// Writes `.name.tmp` next to `path` and renames it over `path`, so a crash
/// never leaves half a file.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("no file name"))?
        .to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    let mut file = File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::meeting::MeetingError;
    use crate::process::ProcessError;

    fn write_failed(kind: io::ErrorKind) -> ProcessError {
        ProcessError::Meeting(MeetingError::Write {
            path: PathBuf::from("/m/transcript.json"),
            source: kind.into(),
        })
    }

    #[test]
    fn a_full_disk_reads_plainly_through_wrapped_errors() {
        assert_eq!(
            error_text(&write_failed(io::ErrorKind::StorageFull)),
            "the disk is full; free up some space and try again"
        );
        let denied = write_failed(io::ErrorKind::PermissionDenied);
        assert_eq!(error_text(&denied), denied.to_string());
    }
}

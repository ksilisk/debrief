//! Recovers a recording that a crash or a forced quit cut off.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

use crate::meeting::{Meeting, MeetingError};
use crate::session::{LOCK_FILE, is_recording};

#[derive(Debug, thiserror::Error)]
pub enum RepairError {
    #[error("{} is still being recorded", path.display())]
    StillRecording { path: PathBuf },
    #[error(transparent)]
    Meeting(#[from] MeetingError),
    #[error("could not repair {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not a WAV file", path.display())]
    NotWav { path: PathBuf },
}

#[derive(Debug)]
pub struct Repaired {
    pub meeting: Meeting,
    /// Video parts that were never finished. Their files are kept but can't be played.
    pub lost_video: Vec<String>,
}

/// Fixes the WAV headers and fills in `meeting.json`. Returns `None` when the
/// recording was stopped normally. The video parts that survived are left for
/// [`crate::meeting::finish_video`].
pub fn repair(folder: &Path) -> Result<Option<Repaired>, RepairError> {
    if is_recording(folder) {
        return Err(RepairError::StillRecording {
            path: folder.to_path_buf(),
        });
    }
    let mut meeting = Meeting::load(folder)?;
    if meeting.recording.stopped_at.is_some() {
        return Ok(None);
    }

    let mut duration: f64 = 0.0;
    let mut stopped_at = meeting.recording.started_at;
    for track in [&meeting.tracks.me, &meeting.tracks.them] {
        let path = folder.join(&track.file);
        if !path.exists() {
            continue;
        }
        duration = duration.max(repair_wav(&path)?);
        if let Ok(modified) = fs::metadata(&path).and_then(|meta| meta.modified()) {
            stopped_at = stopped_at.max(DateTime::<Local>::from(modified).fixed_offset());
        }
    }

    let mut lost_video = Vec::new();
    if let Some(video) = &mut meeting.video {
        video.parts.retain(|part| {
            let path = folder.join(&part.file);
            let complete = mp4_is_complete(&path);
            if !complete && path.exists() {
                lost_video.push(part.file.clone());
            }
            complete
        });
    }

    let mut error =
        "Debrief stopped before the recording ended; the audio was repaired".to_string();
    if !lost_video.is_empty() {
        error.push_str(&format!(
            ". The video in {} was not finished and can't be played",
            lost_video.join(", ")
        ));
    }
    meeting.recording.stopped_at = Some(stopped_at);
    meeting.recording.duration_seconds = duration;
    meeting.recording.error = Some(error);
    meeting.save(folder)?;
    let _ = fs::remove_file(folder.join(LOCK_FILE));
    Ok(Some(Repaired {
        meeting,
        lost_video,
    }))
}

/// Sets the header's sizes to the audio actually in the file and drops a
/// partial last frame. Returns the duration in seconds.
fn repair_wav(path: &Path) -> Result<f64, RepairError> {
    let io_error = |source| RepairError::Io {
        path: path.to_path_buf(),
        source,
    };
    let not_wav = || RepairError::NotWav {
        path: path.to_path_buf(),
    };
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    let len = file.metadata().map_err(io_error)?.len();

    let mut riff = [0u8; 12];
    file.read_exact(&mut riff).map_err(|_| not_wav())?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Err(not_wav());
    }

    let mut offset = 12u64;
    let mut format = None;
    let data_start = loop {
        if offset + 8 > len {
            return Err(not_wav());
        }
        let mut header = [0u8; 8];
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        file.read_exact(&mut header).map_err(io_error)?;
        let size = u64::from(u32::from_le_bytes([
            header[4], header[5], header[6], header[7],
        ]));
        match &header[0..4] {
            b"data" => break offset + 8,
            b"fmt " => {
                let mut fmt = [0u8; 16];
                file.read_exact(&mut fmt).map_err(|_| not_wav())?;
                let sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                let block_align = u64::from(u16::from_le_bytes([fmt[12], fmt[13]]));
                format = Some((sample_rate, block_align));
            }
            _ => {}
        }
        offset += 8 + size + (size & 1);
    };
    let (sample_rate, block_align) = format
        .filter(|&(rate, align)| rate > 0 && align > 0)
        .ok_or_else(not_wav)?;

    let data_len = (len - data_start) / block_align * block_align;
    let data_size = u32::try_from(data_len).map_err(|_| not_wav())?;
    let riff_size = u32::try_from(data_start + data_len - 8).map_err(|_| not_wav())?;
    file.set_len(data_start + data_len).map_err(io_error)?;
    file.seek(SeekFrom::Start(4)).map_err(io_error)?;
    file.write_all(&riff_size.to_le_bytes()).map_err(io_error)?;
    file.seek(SeekFrom::Start(data_start - 4))
        .map_err(io_error)?;
    file.write_all(&data_size.to_le_bytes()).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;

    let frames = data_len / block_align;
    Ok(frames as f64 / f64::from(sample_rate))
}

/// An MP4 can be played only once its `moov` box, the index written last, is there.
fn mp4_is_complete(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return false;
    };
    let mut offset = 0u64;
    while offset + 8 <= len {
        let mut header = [0u8; 8];
        if file.seek(SeekFrom::Start(offset)).is_err() || file.read_exact(&mut header).is_err() {
            return false;
        }
        if &header[4..8] == b"moov" {
            return true;
        }
        let size = match u32::from_be_bytes([header[0], header[1], header[2], header[3]]) {
            0 => return false,
            1 => {
                let mut large = [0u8; 8];
                if file.read_exact(&mut large).is_err() {
                    return false;
                }
                u64::from_be_bytes(large)
            }
            size => u64::from(size),
        };
        if size < 8 {
            return false;
        }
        offset += size;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::{Settings, VIDEO_FILE, VideoFile, VideoPart};
    use crate::recording::{SAMPLE_RATE, Track, TrackWriter};
    use crate::testing::temp_folder;

    fn mp4_box(kind: &[u8; 4], body: usize) -> Vec<u8> {
        let mut bytes = ((8 + body) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.resize(8 + body, 0);
        bytes
    }

    /// A track as a crash leaves it: the header counts only the first second.
    fn cut_off_wav(path: &Path) {
        let mut writer = TrackWriter::create(path).expect("create");
        writer
            .write(0.0, SAMPLE_RATE, &vec![0.25; SAMPLE_RATE as usize])
            .expect("write");
        writer.finish().expect("finish");
        let mut file = OpenOptions::new().append(true).open(path).expect("open");
        file.write_all(&vec![1u8; SAMPLE_RATE as usize * 2 + 1])
            .expect("append");
    }

    #[test]
    fn a_wav_header_is_fixed_to_the_audio_in_the_file() {
        let folder = temp_folder("repair-wav");
        let path = folder.join("track.wav");
        cut_off_wav(&path);
        assert_eq!(
            hound::WavReader::open(&path).expect("open").duration(),
            SAMPLE_RATE
        );
        let seconds = repair_wav(&path).expect("repair");
        assert_eq!(seconds, 2.0);
        let reader = hound::WavReader::open(&path).expect("open");
        assert_eq!(reader.duration(), 2 * SAMPLE_RATE);
        assert_eq!(
            fs::metadata(&path).expect("meta").len(),
            44 + 4 * SAMPLE_RATE as u64
        );
    }

    #[test]
    fn other_files_are_not_touched() {
        let folder = temp_folder("repair-not-wav");
        let path = folder.join("notes.wav");
        fs::write(&path, b"hello, this is not audio").expect("write");
        assert!(matches!(repair_wav(&path), Err(RepairError::NotWav { .. })));
        assert_eq!(fs::read(&path).expect("read"), b"hello, this is not audio");
    }

    #[test]
    fn an_mp4_without_its_index_is_incomplete() {
        let folder = temp_folder("repair-mp4");
        let finished = folder.join("finished.mp4");
        let cut_off = folder.join("cut-off.mp4");
        let mut bytes = mp4_box(b"ftyp", 16);
        bytes.extend(mp4_box(b"mdat", 1000));
        fs::write(&cut_off, &bytes).expect("write");
        bytes.extend(mp4_box(b"moov", 100));
        fs::write(&finished, &bytes).expect("write");
        assert!(mp4_is_complete(&finished));
        assert!(!mp4_is_complete(&cut_off));
    }

    #[test]
    fn repairs_a_meeting_that_never_stopped() {
        let folder = temp_folder("repair-meeting");
        let started_at =
            chrono::DateTime::parse_from_rfc3339("2026-11-04T14:30:00+03:00").expect("valid time");
        let mut meeting = Meeting::new("Sync".into(), started_at, Settings::default());
        let mut finished_part = mp4_box(b"ftyp", 16);
        finished_part.extend(mp4_box(b"moov", 16));
        fs::write(folder.join("screen-1.mp4"), finished_part).expect("write");
        fs::write(folder.join("screen-2.mp4"), mp4_box(b"mdat", 64)).expect("write");
        meeting.video = Some(VideoFile {
            file: VIDEO_FILE.into(),
            source: "Display 1".into(),
            fps: 5,
            bytes: None,
            parts: vec![
                VideoPart {
                    file: "screen-1.mp4".into(),
                    at_seconds: 0.0,
                },
                VideoPart {
                    file: "screen-2.mp4".into(),
                    at_seconds: 1.0,
                },
            ],
        });
        meeting.save(&folder).expect("save");
        cut_off_wav(&folder.join(Track::Me.file_name()));
        cut_off_wav(&folder.join(Track::Them.file_name()));

        let repaired = repair(&folder).expect("repair").expect("needed repair");
        assert_eq!(repaired.lost_video, vec!["screen-2.mp4".to_string()]);
        assert_eq!(repaired.meeting.recording.duration_seconds, 2.0);
        assert!(repaired.meeting.recording.stopped_at.is_some());
        assert!(repaired.meeting.recording.error.is_some());
        let parts = &repaired.meeting.video.as_ref().expect("video").parts;
        assert_eq!(parts.len(), 1);
        assert!(repair(&folder).expect("repair").is_none());
    }
}

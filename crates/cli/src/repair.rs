use std::path::{Path, PathBuf};

use debrief_core::meeting::{self, MEETING_FILE};
use debrief_core::repair::repair;

use crate::format::{clock, size};

#[derive(clap::Args)]
pub struct Args {
    /// Meeting folder.
    folder: PathBuf,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    if !repair_if_needed(&args.folder)? {
        println!(
            "Nothing to repair: the recording in {} is complete.",
            args.folder.display()
        );
    }
    Ok(())
}

/// Returns whether the recording needed a repair.
pub fn repair_if_needed(folder: &Path) -> anyhow::Result<bool> {
    let Some(mut repaired) = repair(folder)? else {
        return Ok(false);
    };
    let video = meeting::finish_video(folder, &mut repaired.meeting, debrief_capture::join_videos);
    repaired.meeting.save(folder)?;

    let meeting = &repaired.meeting;
    println!("Repaired {}", folder.display());
    println!(
        "  audio        {}",
        clock(meeting.recording.duration_seconds)
    );
    if let Some(video) = &meeting.video {
        if !video.parts.is_empty() {
            println!(
                "  {:<12} not joined, parts listed in {MEETING_FILE}",
                video.file
            );
        } else if let Some(bytes) = video.bytes {
            println!("  {:<12} {}", video.file, size(bytes));
        } else {
            println!("  {:<12} none could be saved", video.file);
        }
    }
    for file in &repaired.lost_video {
        println!("  {file:<12} not finished, can't be played");
    }
    if let Err(error) = video {
        println!("{error}");
    }
    Ok(true)
}

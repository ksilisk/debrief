use std::path::PathBuf;

use debrief_core::meeting::{self, MEETING_FILE};
use debrief_core::repair::repair;

use crate::format::{clock, size};

#[derive(clap::Args)]
pub struct Args {
    /// Meeting folder.
    folder: PathBuf,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let Some(mut repaired) = repair(&args.folder)? else {
        println!(
            "Nothing to repair: the recording in {} is complete.",
            args.folder.display()
        );
        return Ok(());
    };
    let video = meeting::finish_video(
        &args.folder,
        &mut repaired.meeting,
        debrief_capture::join_videos,
    );
    repaired.meeting.save(&args.folder)?;

    let meeting = &repaired.meeting;
    println!("Repaired {}", args.folder.display());
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
    Ok(())
}

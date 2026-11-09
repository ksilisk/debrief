use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, bail};
use debrief_core::export::{MARKDOWN_FILE, SUBTITLES_FILE, TEXT_FILE};
use debrief_core::meeting::Step;
use debrief_core::process::{Event, ProcessError, process};
use debrief_core::recording::Track;
use debrief_core::summary::{self, Summarizer};
use debrief_core::transcript::{TRANSCRIPT_FILE, Transcript};
use debrief_summarize::{RemoteModel, config};
use debrief_transcribe::Whisper;
use debrief_transcribe::models::LARGE_V3_TURBO;

use crate::format::{clock, size};
use crate::repair::repair_if_needed;

#[derive(clap::Args)]
pub struct Args {
    /// Meeting folder.
    folder: PathBuf,
    /// A Whisper model file. Default: the one `debrief model download` saves.
    #[arg(long)]
    model: Option<PathBuf>,
    /// Names and terms Whisper should spell right, separated by commas.
    /// Default: the ones from Settings.
    #[arg(long)]
    hints: Option<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    repair_if_needed(&args.folder)?;

    let (path, name) = match &args.model {
        Some(path) => {
            let name = path
                .file_stem()
                .map(|stem| {
                    stem.to_string_lossy()
                        .trim_start_matches("ggml-")
                        .to_string()
                })
                .unwrap_or_else(|| "custom".into());
            (path.clone(), name)
        }
        None => {
            let model = LARGE_V3_TURBO;
            if !model.is_downloaded()? {
                bail!(
                    "the Whisper model is not downloaded yet. Run `debrief model download` ({})",
                    size(model.bytes)
                );
            }
            (model.path()?, model.name.to_string())
        }
    };
    println!("Loading Whisper {name}");
    let mut whisper = Whisper::load(&path, &name)?;
    let hints = match args.hints {
        Some(hints) => hints,
        None => {
            config::Settings::load(&config::folder()?)?
                .transcription
                .hints
        }
    };
    whisper.set_hints(&hints);

    let (mut summarizer, setup_problem) = match RemoteModel::from_settings() {
        Ok(model) => (model, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let summary_label = summarizer
        .as_ref()
        .map(|model| model.model().to_string())
        .unwrap_or_default();

    let cancelled = Arc::new(AtomicBool::new(false));
    let on_ctrl_c = Arc::clone(&cancelled);
    ctrlc::set_handler(move || on_ctrl_c.store(true, Ordering::SeqCst))
        .context("could not catch Ctrl+C")?;

    let started = Instant::now();
    let mut track_started = started;
    let mut ran = false;
    let result = process(
        &args.folder,
        &mut whisper,
        summarizer
            .as_mut()
            .map(|model| model as &mut dyn Summarizer),
        &cancelled,
        &mut |event| {
            match event {
                Event::Started(Step::Transcribe) => {
                    println!("Transcribing (Ctrl+C stops; run again to continue)")
                }
                Event::Started(Step::Merge) => println!("Merging Me and Them, marking echo lines"),
                Event::Started(Step::Export) => println!("Writing the transcript files"),
                Event::Started(Step::Summary) => {
                    println!("Writing the summary with {summary_label}")
                }
                Event::Finished(Step::Summary) => println!(),
                Event::Started(_) | Event::Finished(_) => {}
                Event::Summarizing { .. } => crate::summarize::show_progress(event),
                Event::Transcribing { track, done } => {
                    if done == 0.0 {
                        track_started = Instant::now();
                    }
                    let label = match track {
                        Track::Me => "Me",
                        Track::Them => "Them",
                    };
                    print!(
                        "\r  {label:<5} {:3.0}%  {}   ",
                        done * 100.0,
                        clock(track_started.elapsed().as_secs_f64())
                    );
                    if done >= 1.0 {
                        println!();
                    }
                    let _ = io::stdout().flush();
                }
            }
            ran = true;
        },
    );
    let meeting = match result {
        Ok(meeting) => meeting,
        Err(ProcessError::Cancelled) => {
            println!("\nStopped. Run the same command to continue where it stopped.");
            return Ok(());
        }
        Err(ProcessError::NoSummaryProvider) => {
            let reason = setup_problem.unwrap_or_else(|| "they are not set up".into());
            bail!(
                "the transcript is saved, but no summary was written: summaries are on for this \
                 meeting and {reason}. Run `debrief config summaries`, then `debrief summarize {}`",
                args.folder.display()
            );
        }
        Err(error) => return Err(error.into()),
    };
    if !ran {
        println!(
            "Nothing to do: {} is already transcribed.",
            args.folder.display()
        );
        return Ok(());
    }

    let transcript = Transcript::load(&args.folder)?;
    let count = |speaker: Track| {
        transcript
            .segments
            .iter()
            .filter(|segment| segment.speaker == speaker)
            .count()
    };
    let language = |speaker: Track| {
        transcript
            .segments
            .iter()
            .find(|segment| segment.speaker == speaker)
            .map_or("-", |segment| segment.language.as_str())
            .to_string()
    };
    println!(
        "Transcribed a {} meeting in {}",
        clock(meeting.recording.duration_seconds),
        clock(started.elapsed().as_secs_f64())
    );
    println!(
        "  Me: {} lines ({}), Them: {} lines ({}), {} echo lines marked",
        count(Track::Me),
        language(Track::Me),
        count(Track::Them),
        language(Track::Them),
        transcript.echo_lines()
    );
    println!("Saved to {}", args.folder.display());
    for file in [MARKDOWN_FILE, TEXT_FILE, SUBTITLES_FILE, TRANSCRIPT_FILE] {
        println!("  {file}");
    }
    for file in [summary::MARKDOWN_FILE, summary::TEXT_FILE] {
        if args.folder.join(file).exists() {
            println!("  {file}");
        }
    }
    Ok(())
}

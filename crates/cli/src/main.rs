//! `debrief`: record and process meetings from the terminal.

mod analyze;
mod format;
mod model;
mod process;
mod record;
mod repair;
mod sources;

use std::path::PathBuf;

use anyhow::bail;
use clap::{Parser, Subcommand};

/// Record meetings and turn them into transcripts and summaries.
#[derive(Parser)]
#[command(name = "debrief", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the microphones, apps, screens and windows you can record.
    Sources,
    /// Record the microphone and system audio as separate tracks, with optional screen video.
    Record(record::Args),
    /// Transcribe a recorded meeting: both tracks, merged by time, echo lines marked.
    Process(process::Args),
    /// Show or download the Whisper model.
    Model(model::Args),
    /// Write a summary for a processed meeting folder (M1.5).
    Summarize {
        /// Meeting folder to summarize.
        folder: PathBuf,
    },
    /// Measure sync and echo between me.wav and them.wav (M0.2 test).
    #[command(hide = true)]
    Analyze(analyze::Args),
    /// Fix a recording that a crash or forced quit cut off.
    #[command(hide = true)]
    Repair(repair::Args),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sources => sources::run(),
        Command::Record(args) => record::run(args),
        Command::Process(args) => process::run(args),
        Command::Model(args) => model::run(args),
        Command::Summarize { folder } => {
            not_yet(&format!("summarize {}", folder.display()), "M1.5")
        }
        Command::Analyze(args) => analyze::run(args),
        Command::Repair(args) => repair::run(args),
    }
}

fn not_yet(command: &str, milestone: &str) -> anyhow::Result<()> {
    bail!(
        "`debrief {command}` is not implemented yet (planned in {milestone}, {} {})",
        debrief_core::APP_NAME,
        debrief_core::VERSION
    )
}

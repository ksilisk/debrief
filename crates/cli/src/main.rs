//! `debrief`: record and process meetings from the terminal.

mod analyze;
mod format;
mod record;
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
    /// Transcribe a recorded meeting folder (M1).
    Process {
        /// Meeting folder to process.
        folder: PathBuf,
    },
    /// Write a summary for a processed meeting folder (M1.5).
    Summarize {
        /// Meeting folder to summarize.
        folder: PathBuf,
    },
    /// Measure sync and echo between me.wav and them.wav (M0.2 test).
    #[command(hide = true)]
    Analyze(analyze::Args),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sources => sources::run(),
        Command::Record(args) => record::run(args),
        Command::Process { folder } => not_yet(&format!("process {}", folder.display()), "M1"),
        Command::Summarize { folder } => {
            not_yet(&format!("summarize {}", folder.display()), "M1.5")
        }
        Command::Analyze(args) => analyze::run(args),
    }
}

fn not_yet(command: &str, milestone: &str) -> anyhow::Result<()> {
    bail!(
        "`debrief {command}` is not implemented yet (planned in {milestone}, {} {})",
        debrief_core::APP_NAME,
        debrief_core::VERSION
    )
}

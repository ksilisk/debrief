//! `debrief`: record and process meetings from the terminal.
//!
//! Used for the M0 tests and for automation. The commands are placeholders
//! until their milestones land.

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
    /// Record the microphone and system audio as separate tracks (M0.2).
    Record,
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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Record => not_yet("record", "M0.2"),
        Command::Process { folder } => not_yet(&format!("process {}", folder.display()), "M1"),
        Command::Summarize { folder } => {
            not_yet(&format!("summarize {}", folder.display()), "M1.5")
        }
    }
}

fn not_yet(command: &str, milestone: &str) -> anyhow::Result<()> {
    bail!(
        "`debrief {command}` is not implemented yet (planned in {milestone}, {} {})",
        debrief_core::APP_NAME,
        debrief_core::VERSION
    )
}

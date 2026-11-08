//! `debrief`: record and process meetings from the terminal.

mod analyze;
mod config;
mod format;
mod model;
mod process;
mod record;
mod repair;
mod sources;
mod summarize;

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
    /// Write or rewrite the summary of a transcribed meeting.
    Summarize(summarize::Args),
    /// Show the settings, or set up summaries.
    Config(config::Args),
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
        Command::Summarize(args) => summarize::run(args),
        Command::Config(args) => config::run(args),
        Command::Analyze(args) => analyze::run(args),
        Command::Repair(args) => repair::run(args),
    }
}

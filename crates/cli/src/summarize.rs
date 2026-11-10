use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use debrief_core::process::{Event, ProcessError, summarize_now};
use debrief_core::summary::{self, MARKDOWN_FILE, PROMPT_FILE, Summarizer, TEXT_FILE};
use debrief_summarize::RemoteModel;

#[derive(clap::Args)]
pub struct Args {
    /// Meeting folder.
    folder: PathBuf,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let mut model = RemoteModel::from_settings()?
        .context("summaries are not set up. Run `debrief config summaries`")?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let on_ctrl_c = Arc::clone(&cancelled);
    ctrlc::set_handler(move || on_ctrl_c.store(true, Ordering::SeqCst))
        .context("could not catch Ctrl+C")?;

    let prompt = if summary::own_prompt(&args.folder)?.is_some() {
        format!("this meeting's own prompt ({PROMPT_FILE})")
    } else {
        "the default prompt".to_string()
    };
    println!("Writing the summary with {}, using {prompt}", model.model());
    let result = summarize_now(&args.folder, &mut model, &cancelled, &mut show_progress);
    println!();
    match result {
        Ok(_) => print_files(&args.folder),
        Err(ProcessError::Cancelled) => println!("Stopped."),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub fn show_progress(event: Event) {
    if let Event::Summarizing { characters } = event {
        print!("\r  {characters} characters received   ");
        let _ = io::stdout().flush();
    }
}

pub fn print_files(folder: &Path) {
    println!("Saved to {}", folder.display());
    for file in [MARKDOWN_FILE, TEXT_FILE] {
        println!("  {file}");
    }
}

use std::io::{self, Write};
use std::time::Instant;

use debrief_transcribe::models::LARGE_V3_TURBO;

use crate::format::size;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Download the Whisper model (1.6 GB) into the app's models folder.
    Download,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let model = LARGE_V3_TURBO;
    let path = model.path()?;
    let downloaded = model.is_downloaded()?;
    match args.action {
        None if downloaded => println!(
            "Whisper {}: downloaded, {}\n  {}",
            model.name,
            size(model.bytes),
            path.display()
        ),
        None => println!(
            "Whisper {}: not downloaded. Run `debrief model download` ({}).",
            model.name,
            size(model.bytes)
        ),
        Some(Action::Download) if downloaded => {
            println!("Whisper {} is already downloaded.", model.name)
        }
        Some(Action::Download) => {
            println!("Downloading Whisper {} ({})", model.name, size(model.bytes));
            let started = Instant::now();
            let path = model.download(&mut |done, total| {
                let seconds = started.elapsed().as_secs_f64().max(0.1);
                print!(
                    "\r  {:5.1}%  {} of {}  {}/s   ",
                    done as f64 * 100.0 / total as f64,
                    size(done),
                    size(total),
                    size((done as f64 / seconds) as u64),
                );
                let _ = io::stdout().flush();
            })?;
            println!("\nSaved to {}", path.display());
        }
    }
    Ok(())
}

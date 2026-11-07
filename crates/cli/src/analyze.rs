use std::path::PathBuf;

use debrief_core::analysis::{Level, analyze_folder};

use crate::format::{clock, table};

const PASS_SPREAD_MS: f64 = 50.0;

#[derive(clap::Args)]
pub struct Args {
    /// Recording folder with me.wav and them.wav.
    folder: PathBuf,
    /// Seconds between measured points.
    #[arg(long, default_value_t = 30.0)]
    every: f64,
    /// Print the full report as JSON.
    #[arg(long)]
    json: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let report = analyze_folder(&args.folder, args.every)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("{}", level("me.wav", &report.me));
    println!("{}", level("them.wav", &report.them));
    println!();

    if report.windows.is_empty() {
        println!(
            "No echo of Them found in Me ({} points checked), so sync can't be measured.",
            report.windows_checked
        );
        println!("For the sync test, play sound on speakers, not headphones.");
        return Ok(());
    }

    println!(
        "Echo of Them found in Me at {} of {} points:",
        report.windows.len(),
        report.windows_checked
    );
    let mut rows = vec![vec![
        "At".to_string(),
        "Me lags by".to_string(),
        "Echo".to_string(),
        "Match".to_string(),
    ]];
    rows.extend(report.windows.iter().map(|window| {
        vec![
            clock(window.at_seconds),
            format!("{:+.1} ms", window.lag_ms),
            format!("{:.1} dB", window.echo_db),
            format!("{:.2}", window.correlation),
        ]
    }));
    println!("{}\n", table(&rows));

    if let Some(spread) = report.lag_spread_ms {
        let verdict = if spread < PASS_SPREAD_MS {
            "pass"
        } else {
            "FAIL"
        };
        println!(
            "Lag changed by {spread:.1} ms over the recording: {verdict} (limit {PASS_SPREAD_MS} ms)."
        );
    }
    if let Some(echo) = report.median_echo_db {
        println!(
            "Them is {:.1} dB quieter in your mic than in them.wav (median).",
            -echo
        );
    }
    Ok(())
}

fn level(name: &str, level: &Level) -> String {
    format!(
        "{name:<9} {}  level {}  peak {}",
        clock(level.duration_seconds),
        dbfs(level.rms_dbfs),
        dbfs(level.peak_dbfs)
    )
}

fn dbfs(value: f64) -> String {
    if value <= -120.0 {
        "silent".into()
    } else {
        format!("{value:.1} dBFS")
    }
}

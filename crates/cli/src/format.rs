use std::time::Duration;

pub fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.2} GB", mb / 1000.0)
    } else {
        format!("{mb:.1} MB")
    }
}

pub use debrief_core::export::clock;

/// Accepts `90`, `90s`, `30m` or `1h`.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (number, unit) = match text.char_indices().last() {
        Some((index, unit @ ('s' | 'm' | 'h'))) => (&text[..index], unit),
        _ => (text, 's'),
    };
    let value: f64 = number
        .parse()
        .map_err(|_| format!("`{text}` is not a duration. Use 90s, 30m or 1h"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("`{text}` must be longer than zero"));
    }
    let seconds = match unit {
        'h' => value * 3600.0,
        'm' => value * 60.0,
        _ => value,
    };
    Ok(Duration::from_secs_f64(seconds))
}

pub fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(column, cell)| format!("{cell:<width$}", width = widths[column]))
                .collect();
            format!("  {}", cells.join("  ").trim_end())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("30m"), Ok(Duration::from_secs(1800)));
        assert_eq!(parse_duration("1.5h"), Ok(Duration::from_secs(5400)));
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("0m").is_err());
    }

    #[test]
    fn formats_clock_and_size() {
        assert_eq!(clock(3725.9), "01:02:05");
        assert_eq!(size(34_500_000), "34.5 MB");
        assert_eq!(size(1_250_000_000), "1.25 GB");
    }
}

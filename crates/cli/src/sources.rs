use crate::format::table;

pub fn run() -> anyhow::Result<()> {
    let sources = debrief_capture::sources()?;

    println!("Microphones (--mic)");
    let mut rows = vec![header(&["ID", "Name"])];
    rows.extend(sources.microphones.iter().map(|mic| {
        let default = if mic.is_default { " (default)" } else { "" };
        vec![mic.id.clone(), format!("{}{default}", mic.name)]
    }));
    println!("{}\n", table(&rows));

    println!("Apps for system audio (--app)");
    let mut rows = vec![header(&["Bundle ID", "Name"])];
    rows.extend(
        sources
            .apps
            .iter()
            .map(|app| vec![app.bundle_id.clone(), app.name.clone()]),
    );
    println!("{}\n", table(&rows));

    println!("Screens (--screen)");
    let mut rows = vec![header(&["ID", "Size in points"])];
    rows.extend(sources.displays.iter().map(|display| {
        vec![
            display.id.to_string(),
            format!("{} × {}", display.width, display.height),
        ]
    }));
    println!("{}\n", table(&rows));

    println!("Windows (--window)");
    let mut rows = vec![header(&["ID", "App", "Title", "Size in points"])];
    rows.extend(sources.windows.iter().map(|window| {
        vec![
            window.id.to_string(),
            window.app.clone(),
            window.title.clone(),
            format!("{} × {}", window.width, window.height),
        ]
    }));
    println!("{}", table(&rows));
    Ok(())
}

fn header(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|cell| cell.to_string()).collect()
}

use hostkit::Host;
use workstation::Style;

use crate::state::{Entry, State, Status};

pub fn status(style: &Style, this: Host, state: &State) -> String {
    let peer = state.peer.as_str();
    let direction = format!("{peer} → {}", this.name());
    let header = match (&state.route, &state.error) {
        (Some(route), _) if state.connected => format!("{}  {}", style.bold(&direction), route),
        (_, Some(error)) => format!("{}  {}", style.bold(&direction), style.red(error)),
        _ => format!("{}  {}", style.bold(&direction), style.dim("connecting")),
    };
    if !state.connected {
        return header;
    }
    let header = match &state.error {
        Some(warning) => format!("{header}\n{}", style.red(warning)),
        None => header,
    };
    if state.services.is_empty() {
        return format!("{header}\n{}", style.dim(&format!("no ports on {peer}")));
    }
    let rows = state
        .services
        .iter()
        .map(|entry| row(peer, entry))
        .collect::<Vec<_>>();
    let titles = ["PORT", "PROCESS", peer, "LOCALHOST"].map(str::to_uppercase);
    let widths = (0..3)
        .map(|column| {
            rows.iter()
                .map(|cells| cells[column].0.len())
                .chain([titles[column].len()])
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let mut lines = vec![header];
    lines.push(style.dim(&format!(
        "{:<w0$}  {:<w1$}  {:<w2$}  {}",
        titles[0],
        titles[1],
        titles[2],
        titles[3],
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2]
    )));
    for cells in rows {
        let line = cells
            .iter()
            .enumerate()
            .map(|(column, (text, tone))| {
                let padded = match widths.get(column) {
                    Some(width) => format!("{text:<width$}"),
                    None => text.clone(),
                };
                paint(style, *tone, &padded)
            })
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Good,
    Quiet,
    Bad,
}

pub fn row(peer: &str, entry: &Entry) -> [(String, Tone); 4] {
    [
        (entry.port.to_string(), Tone::Plain),
        (entry.process.clone(), Tone::Plain),
        cell(&entry.alias, &format!("http://{peer}:{}", entry.port)),
        cell(&entry.mirror, &format!("http://localhost:{}", entry.port)),
    ]
}

fn cell(status: &Status, url: &str) -> (String, Tone) {
    match status {
        Status::Active => (url.to_string(), Tone::Good),
        Status::Pending => ("pending".into(), Tone::Quiet),
        Status::Busy(Some(process)) => (format!("busy ({process})"), Tone::Quiet),
        Status::Busy(None) => ("busy".into(), Tone::Quiet),
        Status::Failed(_) => ("failed".into(), Tone::Bad),
    }
}

fn paint(style: &Style, tone: Tone, text: &str) -> String {
    match tone {
        Tone::Plain => text.to_string(),
        Tone::Good => style.green(text),
        Tone::Quiet => style.dim(text),
        Tone::Bad => style.red(text),
    }
}

#[cfg(test)]
#[path = "../tests/unit/render_tests.rs"]
mod tests;

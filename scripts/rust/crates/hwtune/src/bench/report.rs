use super::{
    compare::Comparison,
    record::{LIB, Run},
};
pub fn format_value(value: Option<f64>, scale: &str) -> String {
    let Some(value) = value else {
        return "—".into();
    };
    let text = if value.abs() >= 1000.0 {
        let rounded = format!("{value:.0}");
        let mut grouped = String::new();
        for (index, ch) in rounded.chars().rev().enumerate() {
            if index > 0 && index % 3 == 0 && ch != '-' {
                grouped.push(' ');
            }
            grouped.push(ch);
        }
        grouped.chars().rev().collect()
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    format!("{text} {scale}").trim().into()
}
pub fn describe_run(run: &Run) -> String {
    [
        run.host.as_str(),
        run.os_id(),
        run.tier.as_str(),
        run.grade.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join("  ")
}
fn table(headers: &[&str], rows: Vec<Vec<String>>) {
    let widths = headers
        .iter()
        .enumerate()
        .map(|(column, title)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| ui_terminal::text::width(cell))
                .max()
                .unwrap_or(0)
                .max(title.len())
        })
        .collect::<Vec<_>>();
    let row = |cells: &[String]| {
        let text = cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                format!(
                    "{cell}{}",
                    " ".repeat(widths[column].saturating_sub(ui_terminal::text::width(cell)))
                )
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("{}", text.trim_end());
    };
    row(&headers
        .iter()
        .map(|title| title.to_string())
        .collect::<Vec<_>>());
    for cells in rows {
        row(&cells);
    }
}
pub fn render_run(run: &Run) {
    println!(
        "\n  {}\n  {}  epoch {}",
        run.run_id,
        describe_run(run),
        run.epoch()
    );
    if !run.note.is_empty() {
        println!("  note: {}", run.note);
    }
    if !run.dotfiles_sha.is_empty() {
        println!("  dotfiles: {}", run.dotfiles_sha);
    }
    render_context(run);
    if run.bytes_written > 0 {
        println!(
            "  written: {:.1} GiB",
            run.bytes_written as f64 / 1073741824.0
        );
    }
    println!();
    table(
        &["metric", "median", "unit", "n", "rsd", "scope", "tool"],
        run.metrics
            .iter()
            .map(|m| {
                vec![
                    m.key.clone(),
                    format_value(m.median(), ""),
                    m.scale.clone(),
                    m.samples.len().to_string(),
                    format!("{:.1}%", m.rsd_pct()),
                    m.comparable.clone(),
                    format!("{} {}", m.tool, m.tool_version).trim().into(),
                ]
            })
            .collect(),
    );
    for reason in &run.gate_reasons {
        println!("  ! {reason}");
    }
    println!();
}
pub fn render_context(run: &Run) {
    let Some(context) = &run.context else { return };
    for (label, source) in [("BIOS", &context.bios), ("LACT", &context.lact)] {
        if let Some(source) = source {
            println!("  {label} settings: {}", source.settings_sha256);
            println!(
                "    source: {} ({}, active configuration not verified)",
                source.path, source.kind
            );
        }
    }
    if let Some(session) = &context.tuning_session {
        println!("  tuning session: {session}");
    }
    let mut observed = std::collections::BTreeMap::<&str, std::collections::BTreeSet<&str>>::new();
    for (key, value) in &context.observed {
        let label = if key.starts_with("cpu.policy") {
            key.rsplit('.').next().unwrap_or(key)
        } else {
            key
        };
        observed.entry(label).or_default().insert(value);
    }
    if !observed.is_empty() {
        println!(
            "  observed: {}",
            observed
                .into_iter()
                .map(|(key, values)| format!(
                    "{key}={}",
                    values.into_iter().collect::<Vec<_>>().join("/")
                ))
                .collect::<Vec<_>>()
                .join("  ")
        );
    }
    for link in &context.stability_sessions {
        println!(
            "  stability: {}  {}  {}  {}  evidence {}",
            link.session,
            link.profile,
            link.result,
            link.completed,
            if link.evidence_known {
                "complete"
            } else {
                "incomplete"
            }
        );
    }
    for warning in &context.warnings {
        println!("  context: {warning}");
    }
}
pub fn render_list(runs: &[Run]) {
    if runs.is_empty() {
        println!("  no runs recorded");
        return;
    }
    table(
        &[
            "date", "host", "os", "epoch", "tier", "grade", "metrics", "note",
        ],
        runs.iter()
            .map(|run| {
                vec![
                    run.started
                        .chars()
                        .take(16)
                        .collect::<String>()
                        .replace('T', " "),
                    run.host.clone(),
                    run.os_id().into(),
                    run.epoch(),
                    run.tier.clone(),
                    run.grade.clone(),
                    run.metrics.len().to_string(),
                    run.note.chars().take(32).collect(),
                ]
            })
            .collect(),
    );
}
pub fn render_comparison(left: &Run, right: &Run, result: &Comparison) {
    println!(
        "\n  {}   {}\n  {}   {}\n",
        left.run_id,
        describe_run(left),
        right.run_id,
        describe_run(right)
    );
    for (label, field, before, after) in &result.changes {
        let show = |v: &serde_json::Value| {
            v.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string())
        };
        println!(
            "  hardware changed: {label} {field}: {} → {}",
            show(before),
            show(after)
        );
    }
    table(
        &["metric", "left", "right", "unit", "change", "verdict"],
        result
            .deltas
            .iter()
            .map(|d| {
                vec![
                    d.key.clone(),
                    format_value(Some(d.left), ""),
                    format_value(Some(d.right), ""),
                    d.scale.clone(),
                    if d.verdict == "blocked" {
                        "n/a".into()
                    } else {
                        format!("{:+.1}%", d.change_pct)
                    },
                    if d.verdict == "noise" {
                        format!("within noise (±{:.1}%)", d.band_pct)
                    } else {
                        d.verdict.clone()
                    },
                ]
            })
            .collect(),
    );
    for delta in &result.deltas {
        if delta.verdict == "blocked" {
            println!("  ! {}: {}", delta.key, delta.reason);
        }
    }
    if !result.only_left.is_empty() {
        println!("  only on the left: {}", result.only_left.join(", "));
    }
    if !result.only_right.is_empty() {
        println!("  only on the right: {}", result.only_right.join(", "));
    }
    println!();
}
pub fn sparkline(values: &[f64]) -> String {
    if values.len() < 2 {
        return String::new();
    }
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sparks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    values
        .iter()
        .map(|value| {
            sparks[if high == low {
                4
            } else {
                (((value - low) / (high - low) * 8.0) as usize).min(7)
            }]
        })
        .collect()
}
pub fn render_trend(runs: &[Run], key: &str) {
    let mut rows = runs
        .iter()
        .filter_map(|run| Some((run, run.metric(key)?, run.metric(key)?.median()?)))
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.0.started.cmp(&b.0.started));
    let Some((_, first, _)) = rows.first() else {
        println!("  no runs carry {key}");
        return;
    };
    println!(
        "\n  {key}   {}   {}\n",
        first.scale,
        if first.proportion == LIB {
            "lower is better"
        } else {
            "higher is better"
        }
    );
    table(
        &["date", "epoch", "median", "rsd", "note"],
        rows.iter()
            .map(|(run, metric, value)| {
                vec![
                    run.started.chars().take(10).collect(),
                    run.epoch(),
                    format_value(Some(*value), ""),
                    format!("{:.1}%", metric.rsd_pct()),
                    run.note.chars().take(28).collect(),
                ]
            })
            .collect(),
    );
    let spark = sparkline(&rows.iter().map(|row| row.2).collect::<Vec<_>>());
    if !spark.is_empty() {
        println!("\n  {spark}");
    }
    println!();
}

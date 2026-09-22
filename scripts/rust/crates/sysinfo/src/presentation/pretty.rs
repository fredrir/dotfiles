use super::branding::{BrandProfile, block_text, header_illustration, illustration, resolve_brand};
use crate::formatting::{format_bytes, percentage};
use crate::health::health_summary;
use crate::identity;
use crate::model::{Component, DiskGauge, Gauge, HealthIssue, RenderOptions, Severity, SystemView};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::LazyLock;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Debug)]
pub struct Colors {
    pub text: String,
    pub subtext: String,
    pub overlay: String,
    pub system: String,
    pub hardware: String,
    pub desktop: String,
    pub green: String,
    pub yellow: String,
    pub red: String,
}
impl Colors {
    pub fn from_palette(palette: &Value) -> Result<Self, String> {
        Self::from_theme(&ui_theme::Palette::from_value(palette)?)
    }

    pub fn from_theme(palette: &ui_theme::Palette) -> Result<Self, String> {
        let color = |group: &str, key: &str| -> Result<String, String> {
            Ok(palette.named_color(group, key)?.to_string())
        };
        Ok(Self {
            text: color("colors", "fg")?,
            subtext: color("colors", "muted")?,
            overlay: color("colors", "separator")?,
            system: color("roles", "section_system")?,
            hardware: color("roles", "section_hardware")?,
            desktop: color("roles", "section_desktop")?,
            green: color("colors", "green")?,
            yellow: color("colors", "yellow")?,
            red: color("colors", "red")?,
        })
    }
    pub fn load() -> Result<Self, String> {
        Self::from_theme(&ui_theme::Palette::current())
    }
}
#[derive(Clone)]
struct Span {
    text: String,
    color: String,
    bold: bool,
}
type Line = Vec<Span>;
fn span(text: impl Into<String>, color: &str, bold: bool) -> Span {
    Span {
        text: text.into(),
        color: color.into(),
        bold,
    }
}
fn line(text: impl Into<String>, color: &str, bold: bool) -> Line {
    vec![span(text, color, bold)]
}
fn width(line: &Line) -> usize {
    line.iter()
        .map(|s| UnicodeWidthStr::width(s.text.as_str()))
        .sum()
}
fn compact_label(label: &str) -> String {
    static VERSION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\s+\d+(?:\.\d+)+(?:[-+._a-z0-9]*)?$").unwrap());
    VERSION.replace(label, "").to_string()
}

// Wrap spans before joining columns. Width is measured in terminal cells, including
// wide glyphs, so the same layout works in redirected output and real terminals.
fn wrap(source: &Line, maximum: usize) -> Vec<Line> {
    let maximum = maximum.max(1);
    let mut lines = Vec::new();
    let mut current = Line::new();
    let mut used = 0;
    for item in source {
        for word in item.text.split_inclusive(char::is_whitespace) {
            let cells = UnicodeWidthStr::width(word);
            if used > 0 && used + cells > maximum && cells <= maximum {
                lines.push(current);
                current = Line::new();
                used = 0;
            }
            let mut chunk = String::new();
            for ch in word.chars() {
                if ch == '\n' {
                    if !chunk.is_empty() {
                        current.push(span(std::mem::take(&mut chunk), &item.color, item.bold));
                    }
                    lines.push(current);
                    current = Line::new();
                    used = 0;
                    continue;
                }
                let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
                let (ch, cells) = if cells > maximum {
                    ('?', 1)
                } else {
                    (ch, cells)
                };
                if used + cells > maximum {
                    if !chunk.is_empty() {
                        current.push(span(std::mem::take(&mut chunk), &item.color, item.bold));
                    }
                    lines.push(current);
                    current = Line::new();
                    used = 0;
                }
                if used == 0 && ch.is_whitespace() {
                    continue;
                }
                chunk.push(ch);
                used += cells;
            }
            if !chunk.is_empty() {
                current.push(span(chunk, &item.color, item.bold));
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn preformatted(source: &Line, maximum: usize) -> Line {
    let mut remaining = maximum;
    let mut output = Vec::new();
    for item in source {
        let mut text = String::new();
        for ch in item.text.chars() {
            let cells = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cells > remaining {
                break;
            }
            text.push(ch);
            remaining -= cells;
        }
        output.push(span(text, &item.color, item.bold));
        if remaining == 0 {
            break;
        }
    }
    output
}

fn identity_lines(identity: Vec<Line>, maximum: usize) -> Vec<Line> {
    identity
        .into_iter()
        .enumerate()
        .flat_map(|(index, line)| {
            if (2..7).contains(&index) {
                vec![preformatted(&line, maximum)]
            } else if line.is_empty() {
                vec![line]
            } else {
                wrap(&line, maximum)
            }
        })
        .collect()
}
fn columns(left: &[Line], right: &[Line], left_width: usize, gap: usize) -> Vec<Line> {
    (0..left.len().max(right.len()))
        .map(|row| {
            let mut line = left.get(row).cloned().unwrap_or_default();
            if let Some(other) = right.get(row) {
                line.push(span(
                    " ".repeat(left_width.saturating_sub(width(&line)) + gap),
                    "",
                    false,
                ));
                line.extend(other.clone());
            }
            line
        })
        .collect()
}
fn fact_lines(
    label: &str,
    value: &str,
    label_width: usize,
    available: usize,
    colors: &Colors,
) -> Vec<Line> {
    if available <= label_width + 3 {
        return wrap(
            &vec![
                span(format!("{label}  "), &colors.subtext, false),
                span(value, &colors.text, false),
            ],
            available,
        );
    }
    let values = wrap(
        &line(value, &colors.text, false),
        available - label_width - 2,
    );
    let labels = wrap(&line(label, &colors.subtext, false), label_width);
    columns(&labels, &values, label_width, 2)
}
fn component_art(component: &Component) -> (&'static BrandProfile, Vec<String>) {
    let mut values = vec![component.vendor.as_str(), component.model.as_str()];
    values.extend(component.identifiers.iter().map(String::as_str));
    let brand = resolve_brand(&component.kind, &values);
    let art_kind = if component.art_kind.is_empty() {
        &component.kind
    } else {
        &component.art_kind
    };
    (brand, illustration(brand, art_kind))
}
fn art_gutter(components: &[Component]) -> usize {
    components
        .iter()
        .map(|component| {
            let (_, art) = component_art(component);
            art.iter()
                .map(|s| UnicodeWidthStr::width(s.as_str()))
                .max()
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}
fn component_card(
    component: &Component,
    available: usize,
    gutter: usize,
    colors: &Colors,
) -> Vec<Line> {
    let (brand, art) = component_art(component);
    let generic = brand.key == component.kind;
    let mut title = Vec::new();
    if !generic && !brand.mark.is_empty() {
        title.push(span(format!("{}  ", brand.mark), &brand.accent, true));
    }
    title.push(span(
        if generic {
            &component.label
        } else {
            &brand.name
        },
        &brand.accent,
        true,
    ));
    if !generic && !component.label.eq_ignore_ascii_case(&brand.name) {
        title.push(span(
            format!("  {}", component.label),
            &colors.overlay,
            false,
        ));
    }
    let show_art = available >= 46 && !art.is_empty();
    let art_width = if show_art { gutter } else { 0 };
    let body_width = available.saturating_sub(if show_art { art_width + 2 } else { 0 });
    let mut body = wrap(&title, body_width);
    if !component.model.is_empty() {
        body.extend(wrap(
            &line(&component.model, &colors.text, true),
            body_width,
        ));
    }
    for fact in &component.facts {
        body.extend(fact_lines(&fact.label, &fact.value, 12, body_width, colors));
    }
    if show_art {
        columns(
            &art.iter()
                .map(|s| line(s, &brand.accent, true))
                .collect::<Vec<_>>(),
            &body,
            art_width,
            2,
        )
    } else {
        body
    }
}

// Nerd Font (Material Design Icons) glyphs for the dashboard metric rows.
const GLYPH_CPU: &str = "\u{f0ee0}";
const GLYPH_GPU: &str = "\u{f08ae}";
const GLYPH_RAM: &str = "\u{f035b}";

fn gauge_glyph(kind: &str) -> &'static str {
    match kind {
        "cpu" => GLYPH_CPU,
        "gpu" => GLYPH_GPU,
        _ => GLYPH_RAM,
    }
}
fn load_color(value: f64, colors: &Colors) -> &str {
    if value >= 80.0 {
        &colors.red
    } else if value >= 50.0 {
        &colors.yellow
    } else {
        &colors.green
    }
}
fn temperature_color(value: f64, colors: &Colors) -> &str {
    if value >= 80.0 {
        &colors.red
    } else if value >= 60.0 {
        &colors.yellow
    } else {
        &colors.green
    }
}
fn rule(width: usize, colors: &Colors) -> Line {
    line("\u{2500}".repeat(width.clamp(1, 60)), &colors.overlay, false)
}
fn usage_bar(percent: f64, width: usize) -> String {
    let filled = ((percent.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(width - filled))
}
fn usage_text(used: f64, total: f64) -> String {
    let used_text = format_bytes(used);
    let total_text = format_bytes(total);
    let unit = total_text.rsplit_once(' ').map(|(_, unit)| unit).unwrap_or("");
    let value = used_text
        .rsplit_once(' ')
        .map(|(value, _)| value)
        .unwrap_or(&used_text);
    if !unit.is_empty() && used_text.ends_with(unit) {
        format!("{value} / {total_text}")
    } else {
        format!("{used_text} / {total_text}")
    }
}
fn push_cell(row: &mut Line, text: &str, width: usize, color: &str, bold: bool, right: bool) {
    if !row.is_empty() {
        row.push(span("  ", "", false));
    }
    let padding = " ".repeat(width.saturating_sub(UnicodeWidthStr::width(text)));
    let text = if right {
        format!("{padding}{text}")
    } else {
        format!("{text}{padding}")
    };
    row.push(span(text, color, bold));
}
fn widest(values: &[String]) -> usize {
    values
        .iter()
        .map(|value| UnicodeWidthStr::width(value.as_str()))
        .max()
        .unwrap_or(0)
}
fn gauge_lines(gauges: &[Gauge], available: usize, colors: &Colors) -> Vec<Line> {
    if gauges.is_empty() {
        return Vec::new();
    }
    let glyphs = gauges
        .iter()
        .map(|gauge| gauge_glyph(&gauge.kind).to_string())
        .collect::<Vec<_>>();
    let labels = gauges
        .iter()
        .map(|gauge| gauge.label.clone())
        .collect::<Vec<_>>();
    let loads = gauges
        .iter()
        .map(|gauge| gauge.load.map(|v| format!("{v:.0}%")).unwrap_or_default())
        .collect::<Vec<_>>();
    let usages = gauges
        .iter()
        .map(|gauge| match (gauge.used, gauge.total) {
            (Some(used), Some(total)) => usage_text(used, total),
            _ => String::new(),
        })
        .collect::<Vec<_>>();
    let temperatures = gauges
        .iter()
        .map(|gauge| {
            gauge
                .temperature
                .map(|v| format!("{v:.0}\u{b0}C"))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let widths = [
        widest(&glyphs),
        widest(&labels),
        widest(&loads),
        widest(&usages),
        widest(&temperatures),
    ];
    // Drop the rightmost optional columns until the row fits the content width.
    let mut keep = [true; 5];
    loop {
        let kept = keep.iter().filter(|kept| **kept).count();
        let total = widths
            .iter()
            .zip(keep)
            .filter(|(_, kept)| *kept)
            .map(|(width, _)| width)
            .sum::<usize>()
            + kept.saturating_sub(1) * 2;
        if total <= available {
            break;
        }
        if keep[4] {
            keep[4] = false;
        } else if keep[3] {
            keep[3] = false;
        } else if keep[2] {
            keep[2] = false;
        } else {
            break;
        }
    }
    let mut rows = Vec::new();
    for index in 0..gauges.len() {
        let mut row: Line = Vec::new();
        push_cell(
            &mut row,
            &glyphs[index],
            widths[0],
            &colors.hardware,
            true,
            false,
        );
        if keep[1] {
            push_cell(
                &mut row,
                &labels[index],
                widths[1],
                &colors.text,
                true,
                false,
            );
        }
        if keep[2] {
            let color = gauges[index]
                .load
                .map(|value| load_color(value, colors))
                .unwrap_or(&colors.text);
            push_cell(&mut row, &loads[index], widths[2], color, false, true);
        }
        if keep[3] {
            push_cell(
                &mut row,
                &usages[index],
                widths[3],
                &colors.subtext,
                false,
                true,
            );
        }
        if keep[4] {
            let color = gauges[index]
                .temperature
                .map(|value| temperature_color(value, colors))
                .unwrap_or(&colors.text);
            push_cell(&mut row, &temperatures[index], widths[4], color, false, true);
        }
        rows.push(row);
    }
    rows
}
fn disk_lines(disks: &[DiskGauge], available: usize, colors: &Colors) -> Vec<Line> {
    if disks.is_empty() {
        return Vec::new();
    }
    let labels = disks
        .iter()
        .map(|disk| disk.label.clone())
        .collect::<Vec<_>>();
    let percents = disks
        .iter()
        .map(|disk| format!("{:.0}%", percentage(disk.used, disk.total)))
        .collect::<Vec<_>>();
    let usages = disks
        .iter()
        .map(|disk| usage_text(disk.used, disk.total))
        .collect::<Vec<_>>();
    let (label_width, percent_width, usage_width) =
        (widest(&labels), widest(&percents), widest(&usages));
    let keep_usage = available >= label_width + 2 + percent_width + 2 + usage_width;
    let base = label_width + 2 + percent_width + if keep_usage { 2 + usage_width } else { 0 };
    let bar_width = available.saturating_sub(base + 2).min(24);
    let mut rows = Vec::new();
    for index in 0..disks.len() {
        let mut row: Line = Vec::new();
        push_cell(
            &mut row,
            &labels[index],
            label_width,
            &colors.text,
            true,
            false,
        );
        if bar_width >= 3 {
            let value = percentage(disks[index].used, disks[index].total);
            push_cell(
                &mut row,
                &usage_bar(value, bar_width),
                bar_width,
                load_color(value, colors),
                false,
                false,
            );
        }
        push_cell(
            &mut row,
            &percents[index],
            percent_width,
            &colors.subtext,
            false,
            true,
        );
        if keep_usage {
            push_cell(
                &mut row,
                &usages[index],
                usage_width,
                &colors.subtext,
                false,
                true,
            );
        }
        rows.push(row);
    }
    rows
}
/// Default `-p` dashboard: platform logo beside hostname art, gauges, and disks.
fn dashboard_lines(
    view: &SystemView,
    issues: &[HealthIssue],
    available: usize,
    hostname: &str,
    colors: &Colors,
) -> Vec<Line> {
    let platform = resolve_brand(
        &view.platform.kind,
        &[&view.platform.vendor, &view.platform.label],
    );
    let logo = header_illustration(platform);
    let logo_width = logo
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0);
    let logo_lines = logo
        .iter()
        .map(|s| line(s, &platform.accent, true))
        .collect::<Vec<_>>();
    let host_art = block_text(hostname);
    let host_width = host_art
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0);
    let gap = 4;
    let side_by_side = available >= logo_width + gap + host_width.max(24);
    let content_width = if side_by_side {
        available - logo_width - gap
    } else {
        available
    };
    let mut content: Vec<Line> = host_art
        .iter()
        .map(|row| preformatted(&line(row, &platform.accent, true), content_width))
        .collect();
    content.push(Vec::new());
    content.push(rule(content_width, colors));
    let gauges = gauge_lines(&view.gauges, content_width, colors);
    if !gauges.is_empty() {
        content.push(Vec::new());
        content.extend(gauges);
        content.push(Vec::new());
        content.push(rule(content_width, colors));
    }
    let disks = disk_lines(&view.disks, content_width, colors);
    if !disks.is_empty() {
        content.push(Vec::new());
        content.extend(disks);
        content.push(Vec::new());
        content.push(rule(content_width, colors));
    }
    let summary = health_summary(issues);
    if !summary.is_empty() {
        content.push(Vec::new());
        content.push(line(
            summary,
            if issues.iter().any(|issue| issue.severity == Severity::Error) {
                &colors.red
            } else {
                &colors.yellow
            },
            true,
        ));
    }
    if side_by_side {
        columns(&logo_lines, &content, logo_width, gap)
    } else {
        let mut lines = logo_lines
            .into_iter()
            .flat_map(|line| wrap(&line, available))
            .collect::<Vec<_>>();
        lines.push(Vec::new());
        lines.extend(content.into_iter().flat_map(|line| wrap(&line, available)));
        lines
    }
}
fn health_lines(issues: &[HealthIssue], available: usize, colors: &Colors) -> Vec<Line> {
    let mut lines = Vec::new();
    lines.push(Vec::new());
    lines.extend(wrap(&line("HEALTH", &colors.yellow, true), available));
    lines.push(Vec::new());
    for issue in issues {
        let color = if issue.severity == Severity::Error {
            &colors.red
        } else {
            &colors.yellow
        };
        lines.extend(wrap(
            &vec![
                span(issue.severity.as_str().to_uppercase(), color, true),
                span(format!("  {}", issue.title), &colors.text, true),
            ],
            available,
        ));
        if !issue.detail.is_empty() {
            lines.extend(wrap(&line(&issue.detail, &colors.subtext, false), available));
        }
        if !issue.action.is_empty() {
            lines.extend(wrap(
                &vec![
                    span("Action  ", &colors.desktop, true),
                    span(&issue.action, &colors.text, false),
                ],
                available,
            ));
        }
        lines.push(Vec::new());
    }
    lines
}

pub struct PrettyContext<'a> {
    pub colors: &'a Colors,
    pub width: usize,
    pub username: &'a str,
    pub hostname: &'a str,
    pub colored: bool,
}
pub fn render_with(
    view: &SystemView,
    issues: &[HealthIssue],
    options: RenderOptions,
    context: PrettyContext<'_>,
) -> String {
    render_with_depth(
        view,
        issues,
        options,
        context,
        ui_theme::ColorDepth::detect(),
    )
}

fn render_with_depth(
    view: &SystemView,
    issues: &[HealthIssue],
    options: RenderOptions,
    context: PrettyContext<'_>,
    depth: ui_theme::ColorDepth,
) -> String {
    let PrettyContext {
        colors,
        width: available,
        username,
        hostname,
        colored,
    } = context;
    let available = available.clamp(1, 132);
    let mut lines = if !options.full {
        let mut lines = dashboard_lines(view, issues, available, hostname, colors);
        if options.health && !issues.is_empty() {
            lines.extend(health_lines(issues, available, colors));
        }
        lines
    } else {
    let platform = resolve_brand(
        &view.platform.kind,
        &[&view.platform.vendor, &view.platform.label],
    );
    let mut identity = vec![
        vec![
            span(username.to_uppercase(), &colors.text, true),
            span("   ", &colors.overlay, false),
            span(&view.machine_type, &colors.overlay, true),
        ],
        Vec::new(),
    ];
    let hostname_art = block_text(hostname);
    identity.extend(hostname_art.iter().map(|s| line(s, &platform.accent, true)));
    identity.push(Vec::new());
    let prefix = if platform.mark.is_empty() {
        String::new()
    } else {
        format!("{}  ", platform.mark)
    };
    identity.push(line(
        format!(
            "{prefix}{}",
            compact_label(&view.platform.label).to_uppercase()
        ),
        &platform.accent,
        true,
    ));
    let mut environment = Vec::new();
    for badge in view
        .software
        .iter()
        .filter(|b| matches!(b.kind.as_str(), "hyprland" | "wm" | "session"))
    {
        if !environment.is_empty() {
            environment.push(span("   ", &colors.overlay, false));
        }
        let brand = resolve_brand(&badge.kind, &[&badge.vendor, &badge.label]);
        environment.push(span(
            compact_label(&badge.label).to_uppercase(),
            &brand.accent,
            true,
        ));
    }
    if !environment.is_empty() {
        identity.push(environment);
    }
    let summary = health_summary(issues);
    if !summary.is_empty() {
        identity.push(line(
            summary,
            if issues.iter().any(|i| i.severity == Severity::Error) {
                &colors.red
            } else {
                &colors.yellow
            },
            true,
        ));
    }
    let art = header_illustration(platform);
    let art_width = art
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0);
    let identity_width = hostname_art
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0);
    let mut lines = if available >= 76 && art_width + identity_width + 4 <= available {
        let identity = identity_lines(identity, available - art_width - 4);
        columns(
            &art.iter()
                .map(|s| line(s, &platform.accent, true))
                .collect::<Vec<_>>(),
            &identity,
            art_width,
            4,
        )
    } else {
        identity_lines(identity, available)
    };
    lines.push(Vec::new());
    lines.extend(wrap(&line("HARDWARE", &colors.hardware, true), available));
    lines.push(Vec::new());
    let gutter = art_gutter(&view.components);
    if available >= 94 {
        let card_width = (available - 4) / 2;
        for pair in view.components.chunks(2) {
            let left = component_card(&pair[0], card_width, gutter, colors);
            let right = pair
                .get(1)
                .map(|c| component_card(c, card_width, gutter, colors))
                .unwrap_or_default();
            lines.extend(columns(&left, &right, card_width, 4));
            lines.push(Vec::new());
        }
    } else {
        for component in &view.components {
            lines.extend(component_card(component, available, gutter, colors));
            lines.push(Vec::new());
        }
    }
    if options.full {
        if !view.software.is_empty() {
            lines.extend(wrap(&line("SOFTWARE", &colors.desktop, true), available));
            let mut strip = Vec::new();
            for badge in &view.software {
                let brand = resolve_brand(&badge.kind, &[&badge.vendor, &badge.label]);
                let badge_line = vec![
                    span(format!("{} ", brand.mark), &brand.accent, true),
                    span(&badge.label, &colors.subtext, false),
                ];
                if available < 70 {
                    lines.extend(wrap(&badge_line, available));
                } else {
                    if !strip.is_empty() {
                        strip.push(span("    ", "", false));
                    }
                    strip.extend(badge_line);
                }
            }
            if !strip.is_empty() {
                lines.extend(wrap(&strip, available));
            }
        }
        if !view.system_facts.is_empty() {
            lines.push(Vec::new());
            lines.extend(wrap(&line("SYSTEM", &colors.system, true), available));
            lines.push(Vec::new());
            for fact in &view.system_facts {
                lines.extend(fact_lines(&fact.label, &fact.value, 20, available, colors));
            }
        }
    }
    if options.health && !issues.is_empty() {
        lines.push(Vec::new());
        lines.extend(wrap(&line("HEALTH", &colors.yellow, true), available));
        lines.push(Vec::new());
        for issue in issues {
            let color = if issue.severity == Severity::Error {
                &colors.red
            } else {
                &colors.yellow
            };
            lines.extend(wrap(
                &vec![
                    span(issue.severity.as_str().to_uppercase(), color, true),
                    span(format!("  {}", issue.title), &colors.text, true),
                ],
                available,
            ));
            if !issue.detail.is_empty() {
                lines.extend(wrap(
                    &line(&issue.detail, &colors.subtext, false),
                    available,
                ));
            }
            if !issue.action.is_empty() {
                lines.extend(wrap(
                    &vec![
                        span("Action  ", &colors.desktop, true),
                        span(&issue.action, &colors.text, false),
                    ],
                    available,
                ));
            }
            lines.push(Vec::new());
        }
    }
        lines
    };
    // One blank line of breathing room above and below the presentation.
    lines.insert(0, Vec::new());
    lines.push(Vec::new());
    let mut output = String::new();
    let mut codes = HashMap::new();
    for line in lines {
        let plain = line.iter().map(|s| s.text.as_str()).collect::<String>();
        let visible = plain.trim_end().len();
        let mut remaining = visible;
        for item in line {
            let length = remaining.min(item.text.len());
            let text = &item.text[..length];
            remaining -= length;
            if text.is_empty() {
                continue;
            }
            if colored {
                if item.bold {
                    output.push_str("\x1b[1m");
                }
                let code = codes.entry(item.color).or_insert_with_key(|color| {
                    ui_theme::Color::parse(color)
                        .map(|color| format!("\x1b[{}m", color.at_depth(depth).sgr(false)))
                        .unwrap_or_default()
                });
                output.push_str(code);
                output.push_str(text);
                output.push_str("\x1b[0m");
            } else {
                output.push_str(text);
            }
        }
        output.push('\n');
    }
    output
}

#[cfg(test)]
#[path = "../../tests/unit/presentation/pretty.rs"]
mod tests;
pub fn render_pretty(
    view: &SystemView,
    issues: &[HealthIssue],
    options: RenderOptions,
) -> Result<String, String> {
    let colors = Colors::load()?;
    let username = identity::display_username();
    let hostname = identity::display_hostname();
    Ok(render_with(
        view,
        issues,
        options,
        PrettyContext {
            colors: &colors,
            width: workstation::terminal_width().unwrap_or(80),
            username: &username,
            hostname: &hostname,
            colored: workstation::ColorMode::Auto.enabled(std::io::stdout().is_terminal()),
        },
    ))
}

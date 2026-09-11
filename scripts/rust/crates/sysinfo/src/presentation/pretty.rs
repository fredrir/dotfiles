use super::branding::{block_text, header_illustration, illustration, resolve_brand};
use crate::health::health_summary;
use crate::model::{Component, HealthIssue, RenderOptions, Severity, SystemView};
use crate::{collect, identity, inventory};
use regex::Regex;
use serde_json::Value;
use std::io::IsTerminal;
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;
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
        if palette["version"] != 1 {
            return Err("unsupported palette version".into());
        }
        let color = |group: &str, key: &str| -> Result<String, String> {
            let value = palette[group][key]
                .as_str()
                .ok_or_else(|| format!("missing palette {group}.{key}"))?;
            if value.len() != 7
                || !value.starts_with('#')
                || !value[1..]
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err(format!("invalid palette color: {group}.{key}"));
            }
            Ok(value.into())
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
        let text = collect::probe(
            Command::new(collect::native_binary("dotfile")?)
                .args(["theme", "palette", "--json"])
                .current_dir(inventory::repo_root()),
            Duration::from_secs(10),
        )
        .map_err(|e| format!("theme palette: {e}"))?;
        let palette = serde_json::from_str(&text).map_err(|e| format!("theme palette: {e}"))?;
        Self::from_palette(&palette).map_err(|e| format!("theme palette: {e}"))
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
fn component_card(component: &Component, available: usize, colors: &Colors) -> Vec<Line> {
    let mut values = vec![component.vendor.as_str(), component.model.as_str()];
    values.extend(component.identifiers.iter().map(String::as_str));
    let brand = resolve_brand(&component.kind, &values);
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
    let art_kind = if component.art_kind.is_empty() {
        &component.kind
    } else {
        &component.art_kind
    };
    let art = illustration(brand, art_kind);
    let art_width = art
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0);
    let show_art = available >= 46 && !art.is_empty();
    let body_width = available.saturating_sub(if show_art { art_width + 2 } else { 0 });
    let mut body = wrap(&title, body_width);
    body.extend(wrap(
        &line(&component.model, &colors.text, true),
        body_width,
    ));
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
    let PrettyContext {
        colors,
        width: available,
        username,
        hostname,
        colored,
    } = context;
    let available = available.clamp(1, 132);
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
    identity.push(line(hostname.to_uppercase(), &colors.subtext, true));
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
    if available >= 94 {
        let card_width = (available - 4) / 2;
        for pair in view.components.chunks(2) {
            let left = component_card(&pair[0], card_width, colors);
            let right = pair
                .get(1)
                .map(|c| component_card(c, card_width, colors))
                .unwrap_or_default();
            lines.extend(columns(&left, &right, card_width, 4));
            lines.push(Vec::new());
        }
    } else {
        for component in &view.components {
            lines.extend(component_card(component, available, colors));
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
    let mut output = String::new();
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
                if item.color.len() == 7 {
                    let rgb = u32::from_str_radix(&item.color[1..], 16).unwrap_or_default();
                    output.push_str(&format!(
                        "\x1b[38;2;{};{};{}m",
                        rgb >> 16,
                        (rgb >> 8) & 255,
                        rgb & 255
                    ));
                }
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

//! Doctor report model and rendering: non-package issues, then the packages
//! to install grouped by the manager that provides them.
use std::collections::{BTreeMap, BTreeSet};
use ui_theme::{Role, Style};
use unicode_width::UnicodeWidthStr;

const DETAIL_LIMIT: usize = 12;
const NAME_WIDTH_LIMIT: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Note,
    Warn,
    Bad,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Manager {
    Pacman,
    Aur,
    Apt,
    Brew,
    Cargo,
    Uv,
}

impl Manager {
    pub fn parse(tag: &str) -> Option<Self> {
        match tag {
            "pacman" => Some(Self::Pacman),
            "aur" => Some(Self::Aur),
            "apt" => Some(Self::Apt),
            "brew" => Some(Self::Brew),
            "cargo" => Some(Self::Cargo),
            "uv" => Some(Self::Uv),
            _ => None,
        }
    }
    pub fn for_platform(platform: &str) -> Option<Self> {
        match platform {
            "arch-linux" => Some(Self::Pacman),
            "ubuntu" => Some(Self::Apt),
            "macos" => Some(Self::Brew),
            _ => None,
        }
    }
    pub fn program(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Aur => "yay",
            Self::Apt => "apt-get",
            Self::Brew => "brew",
            Self::Cargo => "cargo",
            Self::Uv => "uv",
        }
    }
    pub fn command(self) -> &'static str {
        match self {
            Self::Pacman => "sudo pacman -S --needed",
            Self::Aur => "yay -S --needed",
            Self::Apt => "sudo apt-get install -y",
            Self::Brew => "brew install",
            Self::Cargo => "cargo install --locked",
            Self::Uv => "uv tool install",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub manager: Option<Manager>,
    pub name: String,
    pub optional: bool,
}

pub struct Row {
    pub status: Status,
    pub label: String,
    pub summary: String,
    pub details: Vec<(String, String)>,
    pub packages: Vec<Package>,
    pub problems: usize,
}

impl Row {
    pub fn new(
        status: Status,
        label: impl Into<String>,
        summary: impl Into<String>,
        problems: usize,
    ) -> Self {
        Self {
            status,
            label: label.into(),
            summary: summary.into(),
            details: Vec::new(),
            packages: Vec::new(),
            problems,
        }
    }
    pub fn missing(label: &str, wanted: usize, packages: Vec<Package>) -> Self {
        if packages.is_empty() {
            return Self::new(Status::Ok, label, format!("{wanted} installed"), 0);
        }
        let mut row = Self::new(
            Status::Bad,
            label,
            format!("{} of {wanted} missing", packages.len()),
            packages.len(),
        );
        row.packages = packages;
        row
    }
}

pub struct Report<'a> {
    pub profile: &'a str,
    pub rows: &'a [Row],
    pub show_all: bool,
}

pub fn render(report: &Report<'_>, style: &Style) -> String {
    let mut out = String::new();
    let issues = collect_issues(report.rows, report.show_all);
    let (names, by_manager) = collect_packages(report.rows, report.show_all);
    if issues.is_empty() && names.is_empty() {
        out.push_str(&style.dim("nothing missing"));
        out.push('\n');
    }
    if !issues.is_empty() {
        out.push('\n');
        out.push_str(&heading(style, "Issues:"));
        out.push('\n');
        out.push_str(&issue_lines(&issues, style));
    }
    if !names.is_empty() {
        out.push('\n');
        out.push_str(&heading(style, "Packages:"));
        out.push('\n');
        for name in &names {
            out.push_str(name);
            out.push('\n');
        }
    }
    if !by_manager.is_empty() {
        out.push('\n');
        out.push_str(&heading(style, "Install:"));
        out.push('\n');
        for (index, (manager, names)) in by_manager.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&install_block(*manager, names));
        }
    }
    out.push('\n');
    out
}

fn heading(style: &Style, text: &str) -> String {
    style.bold(&style.paint(Role::Accent, text))
}

struct Issue<'a> {
    label: &'a str,
    name: String,
    hint: String,
}

/// Failing rows without packages become one line per finding; package rows are
/// covered by the install section.
fn collect_issues(rows: &[Row], show_all: bool) -> Vec<Issue<'_>> {
    let mut issues = Vec::new();
    for row in rows {
        if !matches!(row.status, Status::Warn | Status::Bad) {
            continue;
        }
        if row.details.is_empty() {
            if row.packages.is_empty() {
                issues.push(Issue {
                    label: &row.label,
                    name: row.summary.clone(),
                    hint: String::new(),
                });
            }
            continue;
        }
        let limit = if show_all {
            row.details.len()
        } else {
            row.details.len().min(DETAIL_LIMIT)
        };
        for (name, hint) in row.details.iter().take(limit) {
            issues.push(Issue {
                label: &row.label,
                name: name.clone(),
                hint: hint.clone(),
            });
        }
        if row.details.len() > limit {
            issues.push(Issue {
                label: &row.label,
                name: format!("… and {} more", row.details.len() - limit),
                hint: "--all to list".into(),
            });
        }
    }
    issues
}

fn issue_lines(issues: &[Issue<'_>], style: &Style) -> String {
    let label_width = issues
        .iter()
        .map(|issue| UnicodeWidthStr::width(issue.label))
        .max()
        .unwrap_or(0);
    let name_width = issues
        .iter()
        .filter(|issue| !issue.hint.is_empty())
        .map(|issue| UnicodeWidthStr::width(issue.name.as_str()))
        .max()
        .unwrap_or(0)
        .min(NAME_WIDTH_LIMIT);
    let mut out = String::new();
    for issue in issues {
        let mut line = format!(
            "{}{}  {}",
            issue.label,
            pad(issue.label, label_width),
            issue.name
        );
        if !issue.hint.is_empty() {
            line.push_str(&pad(&issue.name, name_width));
            line.push_str("  ");
            line.push_str(&style.dim(&issue.hint));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn pad(text: &str, width: usize) -> String {
    " ".repeat(width.saturating_sub(UnicodeWidthStr::width(text)))
}

fn collect_packages(
    rows: &[Row],
    show_all: bool,
) -> (BTreeSet<String>, BTreeMap<Manager, BTreeSet<String>>) {
    let mut names = BTreeSet::new();
    let mut by_manager = BTreeMap::<Manager, BTreeSet<String>>::new();
    for package in rows.iter().flat_map(|row| &row.packages) {
        if package.optional && !show_all {
            continue;
        }
        names.insert(package.name.clone());
        if let Some(manager) = package.manager {
            by_manager
                .entry(manager)
                .or_default()
                .insert(package.name.clone());
        }
    }
    (names, by_manager)
}

fn install_block(manager: Manager, names: &BTreeSet<String>) -> String {
    let mut block = format!("{} \\\n", manager.command());
    let last = names.len().saturating_sub(1);
    for (index, name) in names.iter().enumerate() {
        block.push_str("  ");
        block.push_str(name);
        block.push_str(if index == last { "\n" } else { " \\\n" });
    }
    block
}

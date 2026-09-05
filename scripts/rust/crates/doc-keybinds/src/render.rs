use crate::{Binding, MARKER, Package};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
        .replace('|', "&#124;")
        .replace('`', "&#96;")
        .replace('\r', "")
        .replace('\n', "<br>")
}

fn code(s: &str) -> String {
    if s.is_empty() {
        "—".into()
    } else {
        format!(
            "<code>{}</code>",
            escape(if s == " " { "Space" } else { s })
        )
    }
}

fn source(row: &Binding) -> String {
    let url = row
        .source
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('(', "%28")
        .replace(')', "%29");
    format!("../../{url}#L{}", row.line)
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum Os {
    Shared,
    Mac,
    LinuxWindows,
    Linux,
    Windows,
}

impl Os {
    fn name(self) -> &'static str {
        match self {
            Self::Shared => "Shared",
            Self::Mac => "Mac",
            Self::LinuxWindows => "Linux / Windows",
            Self::Linux => "Linux",
            Self::Windows => "Windows",
        }
    }

    fn anchor(self) -> &'static str {
        match self {
            Self::Shared => "shared-keybinds",
            Self::Mac => "mac-keybinds",
            Self::LinuxWindows => "linux--windows-keybinds",
            Self::Linux => "linux-keybinds",
            Self::Windows => "windows-keybinds",
        }
    }
}

fn platform<'a>(row: &'a Binding, package: &str) -> Option<(Os, &'a str)> {
    let (first, remainder) = row.context.split_once("; ").unwrap_or((&row.context, ""));
    let (mut os, context) = match first {
        "macOS" => (Os::Mac, remainder),
        "Linux/Windows" => (Os::LinuxWindows, remainder),
        "Linux" => (Os::Linux, remainder),
        "Windows" => (Os::Windows, remainder),
        _ => (Os::Shared, row.context.as_str()),
    };
    if row.source.starts_with("macos/") {
        if matches!(os, Os::Linux | Os::LinuxWindows | Os::Windows) {
            return None;
        }
        os = Os::Mac;
    } else if row.source.starts_with("linux/") {
        if matches!(os, Os::Mac | Os::Windows) {
            return None;
        }
        os = Os::Linux;
    } else if package == "vscode"
        && row
            .key
            .split(['+', ' '])
            .any(|key| key.eq_ignore_ascii_case("cmd"))
    {
        os = Os::Mac;
    }
    Some((os, context))
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
struct Row<'a> {
    key: &'a str,
    action: &'a str,
    description: &'a str,
    context: &'a str,
}

struct Entry<'a> {
    row: Row<'a>,
    sources: Vec<&'a Binding>,
}

fn groups<'a>(rows: &'a [Binding], package: &str) -> BTreeMap<Os, Vec<Entry<'a>>> {
    let mut matches: BTreeMap<Row<'a>, BTreeMap<Os, Vec<&Binding>>> = BTreeMap::new();
    for binding in rows {
        let Some((os, context)) = platform(binding, package) else {
            continue;
        };
        let row = Row {
            key: &binding.key,
            action: &binding.action,
            description: &binding.description,
            context,
        };
        matches
            .entry(row)
            .or_default()
            .entry(os)
            .or_default()
            .push(binding);
    }
    let mut groups: BTreeMap<Os, Vec<Entry<'a>>> = BTreeMap::new();
    for (row, variants) in matches {
        let shared = variants.contains_key(&Os::Shared)
            || variants.contains_key(&Os::Mac)
                && (variants.contains_key(&Os::LinuxWindows)
                    || variants.contains_key(&Os::Linux) && variants.contains_key(&Os::Windows));
        if shared {
            groups.entry(Os::Shared).or_default().push(Entry {
                row,
                sources: variants.into_values().flatten().collect(),
            });
        } else {
            for (os, sources) in variants {
                groups.entry(os).or_default().push(Entry { row, sources });
            }
        }
    }
    groups
}

fn linked(value: &str, entry: &Entry<'_>) -> String {
    let sources: BTreeSet<_> = entry.sources.iter().map(|row| source(row)).collect();
    let mut links = sources.into_iter().enumerate().map(|(i, url)| {
        format!(
            "[{}]({url})",
            if i == 0 {
                code(value)
            } else {
                (i + 1).to_string()
            }
        )
    });
    let mut result = links.next().unwrap_or_else(|| code(value));
    for link in links {
        result.push(' ');
        result.push_str(&link);
    }
    result
}

fn description(row: &Row<'_>) -> String {
    let mut description = escape(row.description);
    if !row.context.is_empty() && row.context != "global" {
        if !description.is_empty() {
            description.push_str("<br>");
        }
        description.push_str(&code(row.context));
    }
    if description.is_empty() {
        "—".into()
    } else {
        description
    }
}

fn title(package: &str) -> &str {
    match package {
        "hyprland" => "Hyprland",
        "kde" => "KDE",
        "nvim" => "Neovim",
        "vscode" => "VS Code",
        "wezterm" => "WezTerm",
        "yazi" => "Yazi",
        "zsh" => "Zsh",
        _ => package,
    }
}

pub fn pages(packages: &BTreeMap<String, Package>) -> BTreeMap<String, String> {
    let mut pages = BTreeMap::new();
    let mut index = format!(
        "# Keybinds\n\n{MARKER}\n\n| Command | Action |\n| --- | --- |\n| `doc-keybinds` | Regenerate pages |\n| `doc-keybinds --check` | Exit 1 when pages drift |\n| `doc-keybinds --root /path/to/dotfiles` | Select repository |\n| `dotfile sync` | Regenerate with other documentation |\n\n| Scope | Value |\n| --- | --- |\n| Inputs | Repository configurations; all platform variants |\n| Bindings | Configured declarations, including removals and conditional alternatives |\n| Shared | Identical bindings across platform variants appear once |\n| Defaults | Inherited application/plugin defaults are not expanded |\n| Lua | Static syntax parsing; literal loops expanded; unresolved expressions retained |\n| Conditions | Modes and runtime conditions appear in Description |\n| Descriptions | Explicit descriptions, then action names or source expressions |\n| Sources | Action links point to configuration files and lines |\n\n| Package | Keybinds | Settings |\n| --- | ---: | ---: |\n"
    );
    for (name, package) in packages {
        let bindings = groups(&package.bindings, name);
        let settings = groups(&package.settings, name);
        writeln!(
            index,
            "| [{name}](./{name}.md) | {} | {} |",
            bindings.values().map(Vec::len).sum::<usize>(),
            settings.values().map(Vec::len).sum::<usize>()
        )
        .unwrap();
        let mut page = format!(
            "# {} Keybinds\n\n{MARKER}\n\n| OS | Keybinds |\n| --- | --- |\n",
            title(name)
        );
        let mut sections: BTreeSet<_> = bindings.keys().chain(settings.keys()).copied().collect();
        if sections.is_empty() {
            sections.insert(Os::Shared);
        }
        for os in &sections {
            writeln!(
                page,
                "| {} | [{} Keybinds](#{}) |",
                os.name(),
                os.name(),
                os.anchor()
            )
            .unwrap();
        }
        page.push_str("\n[All packages](./_INDEX.md)\n");
        for os in sections {
            writeln!(page, "\n## {} Keybinds\n", os.name()).unwrap();
            if let Some(settings) = settings.get(&os) {
                let values = settings
                    .iter()
                    .map(|entry| {
                        let value = format!(
                            "{} = {}",
                            entry.row.key,
                            if entry.row.action == " " {
                                "Space"
                            } else {
                                entry.row.action
                            }
                        );
                        let mut value = linked(&value, entry);
                        if !entry.row.context.is_empty() {
                            value.push_str(&format!(" ({})", code(entry.row.context)));
                        }
                        value
                    })
                    .collect::<Vec<_>>()
                    .join(" · ");
                writeln!(page, "{values}\n").unwrap();
            }
            page.push_str("| Key | Action | Description |\n| --- | --- | --- |\n");
            if let Some(bindings) = bindings.get(&os) {
                for entry in bindings {
                    writeln!(
                        page,
                        "| {} | {} | {} |",
                        code(entry.row.key),
                        linked(entry.row.action, entry),
                        description(&entry.row)
                    )
                    .unwrap();
                }
            } else {
                page.push_str("| — | — | No configured bindings |\n");
            }
        }
        pages.insert(format!("{name}.md"), page);
    }
    pages.insert("_INDEX.md".into(), index);
    pages
}

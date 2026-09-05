use crate::{Binding, MARKER, Package};
use std::collections::BTreeMap;
use std::fmt::Write;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
    format!(
        "[{}:{}](../../{}#L{})",
        escape(&row.source),
        row.line,
        url,
        row.line
    )
}

pub fn pages(packages: &BTreeMap<String, Package>) -> BTreeMap<String, String> {
    let mut pages = BTreeMap::new();
    let mut index = format!(
        "# Keybindings\n\n{MARKER}\n\n| Command | Action |\n| --- | --- |\n| `doc-keybinds` | Regenerate pages |\n| `doc-keybinds --check` | Exit 1 when pages drift |\n| `doc-keybinds --root /path/to/dotfiles` | Select repository |\n| `dotfile sync` | Regenerate with other documentation |\n\n| Scope | Value |\n| --- | --- |\n| Inputs | Repository configurations; all platform variants |\n| Bindings | Configured declarations, including removals and conditional alternatives |\n| Defaults | Inherited application/plugin defaults are not expanded |\n| Lua | Static syntax parsing; literal loops expanded; unresolved expressions retained |\n| Conditions | Source conditions and callback scopes; no application code executed |\n| Descriptions | Explicit descriptions, then action names or source expressions |\n| Sources | File and line links; edits belong in the configuration |\n\n| Package | Bindings | Settings |\n| --- | ---: | ---: |\n"
    );
    for (name, package) in packages {
        writeln!(
            index,
            "| [{name}](./{name}.md) | {} | {} |",
            package.bindings.len(),
            package.settings.len()
        )
        .unwrap();
        let mut page =
            format!("# {name} keybindings\n\n{MARKER}\n\n[All packages](./_INDEX.md)\n\n");
        if !package.settings.is_empty() {
            page.push_str("| Setting | Value | Context | Source |\n| --- | --- | --- | --- |\n");
            for row in &package.settings {
                writeln!(
                    page,
                    "| {} | {} | {} | {} |",
                    code(&row.key),
                    code(&row.action),
                    code(&row.context),
                    source(row)
                )
                .unwrap();
            }
            page.push('\n');
        }
        page.push_str(
            "| Key | Context | Action | Description | Source |\n| --- | --- | --- | --- | --- |\n",
        );
        for row in &package.bindings {
            writeln!(
                page,
                "| {} | {} | {} | {} | {} |",
                code(&row.key),
                code(&row.context),
                code(&row.action),
                escape(&row.description),
                source(row)
            )
            .unwrap();
        }
        if package.bindings.is_empty() {
            page.push_str("| — | — | — | No configured bindings | — |\n");
        }
        pages.insert(format!("{name}.md"), page);
    }
    pages.insert("_INDEX.md".into(), index);
    pages
}

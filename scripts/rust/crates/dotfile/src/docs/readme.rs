use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use unicode_width::UnicodeWidthChar;

use super::plan::Output;
use crate::context::Context;
use crate::process::{CaptureLimits, output};

fn escape() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap())
}

fn column() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\x1b\[[0-9]+G").unwrap())
}

fn visible_width(text: &str) -> usize {
    text.chars()
        .map(|c| match c as u32 {
            0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd => 2,
            _ => c.width().unwrap_or(0),
        })
        .sum()
}

fn version(context: &Context, program: &str) -> String {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let version = VERSION.get_or_init(|| Regex::new(r"\d+(?:\.\d+)+").unwrap());
    let Ok(result) = output(
        context.command(program).arg("--version"),
        CaptureLimits::default(),
        Duration::from_secs(5),
    ) else {
        return String::new();
    };
    version
        .find(&String::from_utf8_lossy(&result.stdout))
        .map_or(String::new(), |m| m.as_str().into())
}

fn with_version(context: &Context, program: &str) -> String {
    let name = Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program);
    format!("{name} {}", version(context, program))
        .trim()
        .into()
}

fn shell(context: &Context) -> String {
    #[cfg(unix)]
    let login = nix::unistd::User::from_uid(nix::unistd::getuid())
        .ok()
        .flatten()
        .map(|user| user.shell.into_os_string());
    #[cfg(not(unix))]
    let login: Option<std::ffi::OsString> = None;
    let shell = login
        .filter(|value| !value.is_empty())
        .or_else(|| context.env("SHELL"))
        .unwrap_or_else(|| "sh".into());
    with_version(context, &shell.to_string_lossy())
}

fn terminal(context: &Context) -> String {
    let env = |name| {
        context
            .env(name)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let term = env("TERM");
    let program = if !env("KONSOLE_VERSION").is_empty() {
        Some("konsole")
    } else if !env("KITTY_WINDOW_ID").is_empty() || term == "xterm-kitty" {
        Some("kitty")
    } else if !env("ALACRITTY_WINDOW_ID").is_empty() || !env("ALACRITTY_SOCKET").is_empty() {
        Some("alacritty")
    } else if term.starts_with("foot") {
        Some("foot")
    } else {
        None
    };
    if let Some(program) = program {
        return with_version(context, program);
    }
    let program = env("TERM_PROGRAM");
    if !program.is_empty() {
        program
    } else if !term.is_empty() {
        term
    } else {
        "terminal".into()
    }
}

fn render(text: &str, shell: &str, terminal: &str) -> String {
    let mut rows = Vec::new();
    for raw in text.lines() {
        let plain = escape().replace_all(raw, "");
        if plain.contains("Local IP") {
            continue;
        }
        let Some(last) = column().find_iter(raw).last() else {
            rows.push((plain.trim_end().to_string(), None));
            continue;
        };
        let label = escape().replace_all(&raw[..last.start()], "").into_owned();
        let value = if plain
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| word == "Shell")
        {
            shell.into()
        } else if plain
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| word == "Terminal")
        {
            terminal.into()
        } else {
            escape().replace_all(&raw[last.end()..], "").into_owned()
        };
        rows.push((label, Some(value)));
    }
    let width = rows
        .iter()
        .filter(|(_, value)| value.is_some())
        .map(|(label, _)| visible_width(label))
        .max()
        .unwrap_or(0);
    let lines = rows
        .into_iter()
        .map(|(label, value)| match value {
            None => label,
            Some(value) => format!(
                "{label}{}{value}",
                " ".repeat(width.saturating_sub(visible_width(&label)))
            )
            .trim_end()
            .into(),
        })
        .collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(start, |i| i + 1);
    lines[start..end].join("\n")
}

pub(super) fn output_file(context: &Context) -> Result<Output, String> {
    let path = context.root.join("README.md");
    let previous = super::plan::read(&path)?;
    // Check markers before starting external collection.
    super::markdown::replace_block(&previous, "fastfetch", "")?;
    let result = output(
        context
            .command("fastfetch")
            .args(["--config"])
            .arg(context.root.join("shared/fastfetch/config.jsonc"))
            .args(["--pipe", "false"]),
        CaptureLimits::default(),
        Duration::from_secs(15),
    )
    .map_err(|e| format!("fastfetch: {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "fastfetch exited {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    if result.stdout_truncated {
        return Err("fastfetch output exceeds limit".into());
    }
    let body = render(
        &String::from_utf8_lossy(&result.stdout),
        &shell(context),
        &terminal(context),
    );
    let updated =
        super::markdown::replace_block(&previous, "fastfetch", &format!("\n```\n{body}\n```\n"))?;
    Ok(Output::text("README.md", updated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_aligns_unicode_strips_ansi_and_hides_local_ip() {
        let rendered = render(
            "\n\x1b[31m Shell \x1b[20Gwrong\x1b[0m\nテ Terminal \x1b[20Gwrong\n  Local IP \x1b[20G192.0.2.1\n e\u{301} \x1b[20Gvalue\n\n",
            "zsh 5.9",
            "kitty",
        );
        assert!(rendered.contains("zsh 5.9"));
        assert!(rendered.contains("kitty"));
        assert!(!rendered.contains("wrong"));
        assert!(!rendered.contains("192.0.2.1"));
        assert!(!rendered.contains('\x1b'));
        assert_eq!(visible_width("テe\u{301}"), 5);
        assert!(!rendered.starts_with('\n'));
        assert!(!rendered.ends_with('\n'));
    }
}

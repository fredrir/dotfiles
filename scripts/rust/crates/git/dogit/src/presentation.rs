use std::collections::{BTreeMap, BTreeSet};
use std::io::{Seek, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use crate::diff::{ExactDiff, LineKind};

const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffPresentationConfig {
    pub split: bool,
    pub split_wrap: WrapPolicy,
    pub line_numbers: bool,
    pub left_format: String,
    pub right_format: String,
    pub tab_width: usize,
    pub syntax_theme: Option<String>,
    pub hunk_headers: bool,
    pub keep_markers: bool,
    pub number_styles: BTreeMap<String, String>,
    pub delta: Option<DeltaCommand>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WrapPolicy {
    Disabled,
    Limited(usize),
    #[default]
    Unlimited,
}

impl WrapPolicy {
    pub fn row_limit(self) -> Option<usize> {
        match self {
            Self::Disabled => Some(1),
            Self::Limited(continuations) => Some(continuations.saturating_add(1)),
            Self::Unlimited => None,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value == "unlimited" || value == "∞" || value.starts_with("inf") {
            Some(Self::Unlimited)
        } else {
            value.parse::<usize>().ok().map(|count| {
                if count == 0 {
                    Self::Disabled
                } else {
                    Self::Limited(count)
                }
            })
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeltaCommand {
    program: String,
    args: Vec<String>,
}

impl Default for DiffPresentationConfig {
    fn default() -> Self {
        Self {
            split: false,
            split_wrap: WrapPolicy::Unlimited,
            line_numbers: true,
            left_format: String::new(),
            right_format: "{np:>4}│".into(),
            tab_width: 4,
            syntax_theme: None,
            hunk_headers: true,
            keep_markers: true,
            number_styles: BTreeMap::new(),
            delta: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Renderer {
    Builtin,
    Delta,
}

#[derive(Clone, Debug)]
pub struct DiffPresentation {
    pub lines: BTreeMap<usize, String>,
    pub config: DiffPresentationConfig,
    pub renderer: Renderer,
}

pub fn load_config(
    root: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<DiffPresentationConfig, String> {
    let pager_env = std::env::var("GIT_PAGER").ok();
    load_config_with_pager(root, cancel, pager_env.as_deref())
}

fn load_config_with_pager(
    root: &Path,
    cancel: Option<&AtomicBool>,
    pager_env: Option<&str>,
) -> Result<DiffPresentationConfig, String> {
    cancelled(cancel)?;
    let mut git = Command::new("git");
    git.current_dir(root)
        .args(["config", "--null", "--list"])
        .env("GIT_OPTIONAL_LOCKS", "0");
    let output = run(git, None, cancel)?;
    let settings = parse_git_config(&output);
    let delta = configured_delta(&settings, pager_env);
    let mut config = DiffPresentationConfig {
        delta,
        ..DiffPresentationConfig::default()
    };
    if let Some(delta) = &config.delta {
        let resolved = resolve_git_features(&settings);
        let mut command = delta.command(root, false);
        command.args([
            "--show-config",
            "--paging=never",
            "--detect-dark-light=never",
        ]);
        let active = run(command, None, cancel);
        let features_env = std::env::var("DELTA_FEATURES").ok();
        config.split_wrap = resolve_wrap_policy(&settings, delta, features_env.as_deref());
        apply_settings(&mut config, &resolved);
        match active {
            Ok(output) => {
                apply_settings(&mut config, &parse_show_config(&output));
                config.number_styles = parse_number_styles(&output);
            }
            Err(_) => cancelled(cancel)?,
        }
    }
    Ok(config)
}

pub fn render(
    root: &Path,
    diff: &ExactDiff,
    width: usize,
    config: &DiffPresentationConfig,
    cancel: Option<&AtomicBool>,
) -> Result<DiffPresentation, String> {
    cancelled(cancel)?;
    if let Some(delta) = &config.delta {
        let mut command = delta.command(root, true);
        command.args([
            "--color-only",
            "--paging=never",
            "--detect-dark-light=never",
            "--true-color=always",
            "--line-numbers-left-format=",
            "--line-numbers-right-format=",
            "--hunk-header-style=raw",
            "--hunk-header-decoration-style=none",
            "--file-style=raw",
            "--file-decoration-style=none",
            "--inspect-raw-lines=false",
            "--tabs=0",
        ]);
        command.arg(format!("--width={}", width.max(1)));
        if let Ok(output) = run(command, Some(&diff.raw), cancel)
            && let Some(lines) = map_delta_lines(diff, &output)
        {
            return Ok(DiffPresentation {
                lines,
                config: config.clone(),
                renderer: Renderer::Delta,
            });
        }
        cancelled(cancel)?;
    }
    Ok(DiffPresentation {
        lines: highlight_builtin(diff, config, cancel)?,
        config: config.clone(),
        renderer: Renderer::Builtin,
    })
}

impl DeltaCommand {
    fn command(&self, root: &Path, color_only: bool) -> Command {
        let mut command = Command::new(&self.program);
        command.current_dir(root);
        let mut arguments = self.args.iter();
        while let Some(argument) = arguments.next() {
            let option = argument.split('=').next().unwrap_or(argument);
            let valued_override = matches!(option, "--paging" | "--detect-dark-light")
                || color_only
                    && matches!(
                        option,
                        "--true-color"
                            | "--24-bit-color"
                            | "--line-numbers-left-format"
                            | "--line-numbers-right-format"
                            | "--hunk-header-style"
                            | "--hunk-header-decoration-style"
                            | "--file-style"
                            | "--file-decoration-style"
                            | "--inspect-raw-lines"
                            | "--tabs"
                            | "--width"
                            | "-w"
                    );
            if valued_override {
                if !argument.contains('=') {
                    arguments.next();
                }
            } else if !matches!(option, "--show-config")
                && !(color_only && option == "--color-only")
            {
                command.arg(argument);
            }
        }
        command
    }
}

fn configured_delta(
    settings: &BTreeMap<String, String>,
    pager_env: Option<&str>,
) -> Option<DeltaCommand> {
    if let Some(pager) = pager_env {
        return parse_delta_command(pager);
    }
    let pager = settings
        .get("pager.diff")
        .or_else(|| settings.get("core.pager"));
    if let Some(pager) = pager {
        return parse_delta_command(pager);
    }
    settings
        .get("interactive.difffilter")
        .and_then(|value| parse_delta_command(value))
}

fn parse_delta_command(value: &str) -> Option<DeltaCommand> {
    if value.contains(['\n', '\r', '\0']) || contains_shell_syntax(value) {
        return None;
    }
    let words = shlex::split(value)?;
    let program = words.first()?;
    if !matches!(
        Path::new(program).file_name()?.to_str()?,
        "delta" | "delta.exe"
    ) {
        return None;
    }
    Some(DeltaCommand {
        program: program.clone(),
        args: words[1..].to_vec(),
    })
}

fn contains_shell_syntax(value: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
        } else if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote != Some('\'') && matches!(character, '`' | '$')
            || quote.is_none() && matches!(character, ';' | '|' | '&' | '<' | '>')
        {
            return true;
        }
    }
    false
}

fn parse_git_config(output: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8_lossy(output)
        .split('\0')
        .filter_map(|entry| entry.split_once('\n'))
        .map(|(key, value)| (key.to_lowercase(), value.to_owned()))
        .collect()
}

fn parse_show_config(output: &[u8]) -> BTreeMap<String, String> {
    strip_ansi(&String::from_utf8_lossy(output))
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            let value = value.trim();
            let value = value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
                .unwrap_or(value);
            (key.trim().to_owned(), value.to_owned())
        })
        .collect()
}

fn parse_number_styles(output: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let key = key
                .trim()
                .strip_prefix("line-numbers-")?
                .strip_suffix("-style")?;
            if !matches!(key, "minus" | "plus" | "zero" | "left" | "right") {
                return None;
            }
            let value = value.trim_start();
            let mut length = 0;
            while let Some(sequence) = value[length..].strip_prefix("\x1b[") {
                let end = sequence.find('m')?;
                if !sequence[..end]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b';')
                {
                    break;
                }
                length += 2 + end + 1;
            }
            Some((key.to_owned(), value[..length].to_owned()))
        })
        .collect()
}

fn resolve_git_features(settings: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut resolved = BTreeMap::new();
    if let Some(features) = settings.get("delta.features") {
        for feature in features.split_whitespace() {
            let prefix = format!("delta.{}.", feature.to_lowercase());
            for (key, value) in settings {
                if let Some(key) = key.strip_prefix(&prefix) {
                    resolved.insert(key.to_owned(), value.clone());
                }
            }
        }
    }
    for (key, value) in settings {
        if let Some(key) = key.strip_prefix("delta.")
            && !key.contains('.')
        {
            resolved.insert(key.to_owned(), value.clone());
        }
    }
    resolved
}

fn resolve_wrap_policy(
    settings: &BTreeMap<String, String>,
    delta: &DeltaCommand,
    features_env: Option<&str>,
) -> WrapPolicy {
    if let Some(value) = command_option(&delta.args, "--wrap-max-lines") {
        return WrapPolicy::parse(value).unwrap_or_default();
    }
    if delta
        .args
        .iter()
        .any(|argument| argument == "--no-gitconfig")
    {
        return WrapPolicy::Unlimited;
    }
    if let Some(value) = settings.get("delta.wrap-max-lines") {
        return WrapPolicy::parse(value).unwrap_or_default();
    }
    let configured_features = command_option(&delta.args, "--features")
        .or_else(|| settings.get("delta.features").map(String::as_str))
        .unwrap_or("");
    let mut features = configured_features.split_whitespace().collect::<Vec<_>>();
    if let Some(value) = features_env {
        if let Some(appended) = value.strip_prefix('+') {
            features.extend(appended.split_whitespace().rev());
        } else {
            features = value.split_whitespace().collect();
        }
    }
    features
        .iter()
        .rev()
        .find_map(|feature| feature_wrap_value(settings, feature, &mut BTreeSet::new()))
        .and_then(WrapPolicy::parse)
        .unwrap_or_default()
}

fn feature_wrap_value<'a>(
    settings: &'a BTreeMap<String, String>,
    feature: &str,
    seen: &mut BTreeSet<String>,
) -> Option<&'a str> {
    let feature = feature.to_lowercase();
    if seen.len() >= 64 || !seen.insert(feature.clone()) {
        return None;
    }
    let prefix = format!("delta.{feature}.");
    if let Some(value) = settings.get(&format!("{prefix}wrap-max-lines")) {
        return Some(value);
    }
    settings
        .get(&format!("{prefix}features"))
        .and_then(|features| {
            features
                .split_whitespace()
                .rev()
                .find_map(|child| feature_wrap_value(settings, child, seen))
        })
}

fn command_option<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments.iter().enumerate().find_map(|(index, argument)| {
        if argument == name {
            arguments.get(index + 1).map(String::as_str)
        } else {
            argument
                .strip_prefix(name)
                .and_then(|value| value.strip_prefix('='))
        }
    })
}

fn apply_settings(config: &mut DiffPresentationConfig, settings: &BTreeMap<String, String>) {
    let boolean = |key: &str| {
        settings.get(key).and_then(|value| match value.as_str() {
            "true" | "yes" | "on" | "1" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        })
    };
    if let Some(value) = boolean("side-by-side") {
        config.split = value;
    }
    if let Some(value) = boolean("line-numbers") {
        config.line_numbers = value;
    }
    if let Some(value) = boolean("keep-plus-minus-markers") {
        config.keep_markers = value;
    }
    if let Some(value) = settings.get("line-numbers-left-format") {
        config.left_format.clone_from(value);
    }
    if let Some(value) = settings.get("line-numbers-right-format") {
        config.right_format.clone_from(value);
    }
    if let Some(value) = settings.get("tabs").and_then(|value| value.parse().ok()) {
        config.tab_width = value;
    }
    if let Some(value) = settings.get("syntax-theme") {
        config.syntax_theme = Some(value.clone());
    }
    if let Some(value) = settings.get("hunk-header-style") {
        config.hunk_headers = !value.split_whitespace().any(|word| word == "omit");
    }
}

fn strip_ansi(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\x1b' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for character in chars.by_ref() {
                    if ('@'..='~').contains(&character) {
                        break;
                    }
                }
            }
            Some(']' | 'P' | '^' | '_') => {
                while let Some(character) = chars.next() {
                    if character == '\x07' {
                        break;
                    }
                    if character == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some('(' | ')') => {
                chars.next();
            }
            _ => {}
        }
    }
    output
}

fn map_delta_lines(diff: &ExactDiff, output: &[u8]) -> Option<BTreeMap<usize, String>> {
    let output = std::str::from_utf8(output).ok()?;
    let raw = std::str::from_utf8(&diff.raw).ok()?;
    if strip_ansi(output) != raw {
        return None;
    }
    let mut result = BTreeMap::new();
    let mut hunks = diff.hunks.iter();
    let mut lines = None;
    for (raw, colored) in raw.split_inclusive('\n').zip(output.split_inclusive('\n')) {
        if raw.starts_with("@@ ") {
            lines = Some(hunks.next()?.lines.iter());
        } else if raw.starts_with([' ', '+', '-'])
            && let Some(line) = lines.as_mut().and_then(Iterator::next)
        {
            let mut colored = colored.trim_end_matches(['\n', '\r']).to_owned();
            let mut index = 0;
            let bytes = colored.as_bytes();
            while bytes.get(index) == Some(&0x1b) && bytes.get(index + 1) == Some(&b'[') {
                index += 2;
                while bytes
                    .get(index)
                    .is_some_and(|byte| !(b'@'..=b'~').contains(byte))
                {
                    index += 1;
                }
                index += 1;
            }
            if !matches!(bytes.get(index), Some(b' ' | b'+' | b'-')) {
                return None;
            }
            colored.remove(index);
            result.insert(line.id, colored);
        }
    }
    (result.len()
        == diff
            .hunks
            .iter()
            .map(|hunk| hunk.lines.len())
            .sum::<usize>())
    .then_some(result)
}

fn highlight_builtin(
    diff: &ExactDiff,
    config: &DiffPresentationConfig,
    cancel: Option<&AtomicBool>,
) -> Result<BTreeMap<usize, String>, String> {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let themes = THEMES.get_or_init(ThemeSet::load_defaults);
    let theme = config
        .syntax_theme
        .as_ref()
        .and_then(|name| themes.themes.get(name))
        .unwrap_or(&themes.themes["base16-ocean.dark"]);
    let mut result = BTreeMap::new();
    let mut highlighted_bytes = 0usize;
    for hunk in &diff.hunks {
        cancelled(cancel)?;
        let path = if diff.path.as_os_str().is_empty() {
            hunk.file_path.to_str().unwrap_or("")
        } else {
            diff.path.to_str().unwrap_or("")
        };
        let extension = Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let syntax = syntaxes
            .find_syntax_by_extension(extension)
            .or_else(|| syntaxes.find_syntax_by_extension(Path::new(path).file_name()?.to_str()?))
            .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
        let mut old = HighlightLines::new(syntax, theme);
        let mut new = HighlightLines::new(syntax, theme);
        for line in &hunk.lines {
            cancelled(cancel)?;
            let text = String::from_utf8_lossy(&line.content)
                .chars()
                .filter(|character| !character.is_control() || matches!(character, '\t' | '\n'))
                .collect::<String>();
            highlighted_bytes = highlighted_bytes.saturating_add(text.len());
            if text.len() > 16 * 1024 || highlighted_bytes > 4 * 1024 * 1024 {
                result.insert(line.id, text.trim_end_matches(['\r', '\n']).to_owned());
                continue;
            }
            let spans = match line.kind {
                LineKind::Deletion => old.highlight_line(&text, syntaxes),
                LineKind::Addition => new.highlight_line(&text, syntaxes),
                LineKind::Context => {
                    let _ = old.highlight_line(&text, syntaxes);
                    new.highlight_line(&text, syntaxes)
                }
            };
            let colored = spans.map_or_else(
                |_| text.trim_end_matches(['\r', '\n']).to_owned(),
                |spans| {
                    syntect::util::as_24_bit_terminal_escaped(&spans, false)
                        .trim_end_matches(['\r', '\n'])
                        .to_owned()
                },
            );
            result.insert(line.id, colored);
        }
    }
    Ok(result)
}

fn cancelled(cancel: Option<&AtomicBool>) -> Result<(), String> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
        Err("Diff presentation cancelled".into())
    } else {
        Ok(())
    }
}

fn run(
    mut command: Command,
    input: Option<&[u8]>,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, String> {
    cancelled(cancel)?;
    if let Some(input) = input {
        let mut file = tempfile::tempfile().map_err(|error| error.to_string())?;
        file.write_all(input).map_err(|error| error.to_string())?;
        file.rewind().map_err(|error| error.to_string())?;
        command.stdin(Stdio::from(file));
    } else {
        command.stdin(Stdio::null());
    }
    let output = hostkit::process::output_cancellable(
        &mut command,
        hostkit::process::CaptureLimits {
            stdout: OUTPUT_LIMIT,
            stderr: 128 * 1024,
        },
        TIMEOUT,
        &|| cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)),
    )
    .map_err(|error| error.to_string())?;
    cancelled(cancel)?;
    if output.stdout_truncated {
        return Err("Diff presentation exceeds the output limit".into());
    }
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}

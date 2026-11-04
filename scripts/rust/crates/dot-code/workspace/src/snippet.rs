//! A file as a fenced Markdown block under its workspace-relative path.

use std::path::PathBuf;

use serde::Deserialize;

use crate::siblings::slash_separated;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub path: PathBuf,
    /// Workspace folder the header path is relative to.
    #[serde(default)]
    pub root: Option<PathBuf>,
    pub text: String,
}

pub fn markdown(params: &Params) -> String {
    let Params { path, root, text } = params;
    let header = root
        .as_deref()
        .and_then(|root| path.strip_prefix(root).ok())
        .map(slash_separated)
        .filter(|relative| !relative.is_empty())
        .unwrap_or_else(|| path.display().to_string());
    let language = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('.'))
        .map_or("", |(_, extension)| extension);
    let fence = "`".repeat(longest_backtick_run(text).max(2) + 1);
    let newline = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    format!("---\n**{header}:**\n{fence}{language}\n{text}{newline}{fence}\n---\n\n")
}

fn longest_backtick_run(text: &str) -> usize {
    text.split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0)
}

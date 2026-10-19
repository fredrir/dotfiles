//! oh-my-zsh plugins that rewrite a completion file in the background on each
//! start, `$ZSH_CACHE_DIR/completions/_NAME`: the build runs that job once
//! against a scratch cache and writes the file, and the bundle runs the job
//! only when the file is missing.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use hostkit::process::{CaptureLimits, output};
use zshrs_parse::parser::ZshList;

use super::{Compiler, Cx};
use crate::cache::{Key, Outcome};
use crate::expand::{Env, Var};
use crate::quote;
use crate::script::{self, Script};

const TARGET: &str = "\"$ZSH_CACHE_DIR/completions/_";

/// What a plugin loads at its top level for the job.
const PRELUDE: &str = "zmodload -F zsh/files b:zf_mv\nautoload -Uz is-at-least\n";

const TIMEOUT: Duration = Duration::from_secs(30);

/// A completion file the build writes in place of a plugin's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    pub path: PathBuf,
    pub text: String,
}

impl Compiler {
    /// The background job at `lines`, when it writes one completion file:
    /// the code that runs it only when that file is missing.
    pub(super) fn regenerated(
        &mut self,
        script: &Script,
        list: &ZshList,
        lines: &RangeInclusive<usize>,
        cx: &Cx,
    ) -> Option<String> {
        if cx.analysis || self.dead > 0 || self.loops > 0 {
            return None;
        }
        let text = script.lines(lines);
        let name = target(text)?;
        let job = without_trailing_comments(text);
        let job = job
            .strip_suffix("&|")
            .or_else(|| job.strip_suffix("&!"))?;
        if job.contains("&|") || job.contains("&!") {
            return None;
        }
        let Var::Scalar(cache) = self.state.var("ZSH_CACHE_DIR") else {
            return None;
        };
        let program = self.state.command(&name)??;
        let origin = cx.origin?;
        if self.reads_set_style(std::slice::from_ref(list)) {
            self.left_to_runtime(Some(origin), &format!("_{name}: zstyle set for it"));
            return None;
        }
        let zsh = self.zsh.clone()?;
        let code = sandbox(origin, &self.state.var("ZSH"), job)?;
        let path = self
            .state
            .path()
            .and_then(|path| std::env::join_paths(path).ok());
        let identity = [
            "completion-file".to_string(),
            name.clone(),
            format!("{:016x}", script::hash(&code)),
        ];
        let key = Key::new(&identity, &program, path.as_deref());
        let outcome = self
            .folder
            .cache
            .get_or_run(key, || run(&zsh, &code, &name, path.as_deref()));
        match outcome {
            Ok(generated) => {
                self.generated.push(Generated {
                    path: PathBuf::from(cache)
                        .join("completions")
                        .join(format!("_{name}")),
                    text: generated,
                });
                self.folded += 1;
                Some(format!("[[ -f {TARGET}{name}\" ]] || {{\n{text}}}\n"))
            }
            Err(error) => {
                self.left_to_runtime(Some(origin), &format!("_{name}: {error}"));
                None
            }
        }
    }
}

/// Writes the files whose text changed; returns whether any did.
pub fn write(files: &[Generated], dry_run: bool) -> Result<bool, String> {
    let mut changed = false;
    for file in files {
        if std::fs::read_to_string(&file.path).is_ok_and(|current| current == file.text) {
            continue;
        }
        changed = true;
        if !dry_run {
            crate::emit::write_atomic(&file.path, &file.text)?;
        }
    }
    Ok(changed)
}

/// The one completion function every target in `text` names.
fn target(text: &str) -> Option<String> {
    let mut names = text.match_indices(TARGET).map(|(at, _)| {
        let rest = &text[at + TARGET.len()..];
        rest.split_once('"').map(|(name, _)| name)
    });
    let first = names.next()??;
    let plain = !first.is_empty() && first.bytes().all(quote::is_plain);
    (plain && names.all(|name| name == Some(first))).then(|| first.to_string())
}

/// `text` up to its last line that is neither blank nor a comment.
fn without_trailing_comments(text: &str) -> &str {
    let mut end = text.trim_end();
    while let Some((head, last)) = end.rsplit_once('\n') {
        if !last.trim_start().starts_with('#') {
            break;
        }
        end = head.trim_end();
    }
    end
}

/// The job as a script for `zsh -f`, with what it reads of the plugin.
fn sandbox(origin: &Path, zsh: &Var, job: &str) -> Option<String> {
    let origin = quote::word(origin.to_str()?);
    let zsh = match zsh {
        Var::Scalar(zsh) => format!("ZSH={}\n", quote::word(zsh)),
        _ => String::new(),
    };
    Some(format!("0={origin}\n{zsh}{PRELUDE}{job}\n"))
}

/// Runs `code` with a scratch `ZSH_CACHE_DIR`; the file it wrote for `name`.
fn run(
    zsh: &Path,
    code: &str,
    name: &str,
    path: Option<&std::ffi::OsStr>,
) -> Result<Outcome, String> {
    let scratch = tempfile::Builder::new()
        .prefix("zsh-build-completion.")
        .tempdir()
        .map_err(|error| format!("scratch directory: {error}"))?;
    let completions = scratch.path().join("completions");
    std::fs::create_dir_all(&completions)
        .map_err(|error| format!("create {}: {error}", completions.display()))?;
    let mut command = Command::new(zsh);
    command
        .args(["-fc", code])
        .current_dir(scratch.path())
        .env("ZSH_CACHE_DIR", scratch.path())
        .stdin(Stdio::null());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let captured = output(&mut command, CaptureLimits::default(), TIMEOUT)
        .map_err(|error| format!("_{name}: {error}"))?;
    let file = completions.join(format!("_{name}"));
    Ok(match std::fs::read_to_string(&file) {
        Ok(text) if !text.trim().is_empty() => Ok(text),
        _ if !captured.status.success() => Err(format!("job failed: {}", captured.status)),
        _ => Err("job wrote no file".to_string()),
    })
}

#[cfg(test)]
#[path = "../../tests/unit/compile/regenerate_tests.rs"]
mod tests;

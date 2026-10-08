//! The global startup files zsh reads before `.zshrc`, and the user's
//! `.zprofile`, compiled to run from the bundle in zsh's order.

use std::path::{Path, PathBuf};

use super::{Compiler, Cx, Mode, Origin, placeholder};
use crate::script::Script;
use crate::state::State;

/// Set by the guard when zsh skips the global startup files; the bundle and
/// `.zprofile` then run them instead.
pub const FLAG: &str = "_zsh_build_rcs";

/// Global files zsh reads before `.zshrc`; `zlogin` and `zlogout` stay with zsh.
const LOGIN: &str = "zprofile";
const INTERACTIVE: &str = "zshrc";

pub struct Startup<'a> {
    pub dir: &'a Path,
    /// `~/.zprofile`, or `$ZDOTDIR/.zprofile`.
    pub link: PathBuf,
    /// The repo file `~/.zprofile` must link to.
    pub profile: Option<PathBuf>,
    pub env: &'a [PathBuf],
}

pub enum Outcome {
    Compiled(Section),
    /// Nothing to compile; zsh keeps its startup files.
    Empty,
    /// Left to zsh, with the reason and whether the dotfiles can fix it.
    Kept {
        reason: String,
        fixable: bool,
    },
}

pub struct Section {
    pub text: String,
    /// Changes that make the section stale.
    pub sources: Vec<PathBuf>,
    /// The repo file `~/.zprofile` links to; `None` when it must not exist.
    pub profile: Option<PathBuf>,
}

impl Compiler {
    /// Compiles the section with `state`, leaving the compiler's state as it was.
    pub fn system(&mut self, state: State, startup: &Startup) -> Result<Outcome, String> {
        let saved = std::mem::replace(&mut self.state, state);
        let snapshot = self.snapshot();
        self.state.forget("PATH");
        let mut result = Ok(());
        for env in startup.env {
            result = result.and(self.analyze(env));
        }
        self.startup = true;
        self.refusals.clear();
        let dependencies = self.dependencies.len();
        let outcome = result.and_then(|()| self.section(startup));
        self.startup = false;
        let outcome = match outcome {
            Ok(Outcome::Compiled(mut section)) => {
                section
                    .sources
                    .extend(self.dependencies.drain(dependencies..));
                Ok(Outcome::Compiled(section))
            }
            other => {
                self.restore(snapshot);
                self.dependencies.truncate(dependencies);
                other
            }
        };
        self.state = saved;
        outcome
    }

    fn section(&mut self, startup: &Startup) -> Result<Outcome, String> {
        let global = |name: &str| {
            let path = startup.dir.join(name);
            path.exists().then_some(path)
        };
        let login = global(LOGIN);
        let interactive = global(INTERACTIVE);
        let profile = match (startup.link.exists(), &startup.profile) {
            (false, _) => None,
            (true, Some(profile)) if same_file(&startup.link, profile) => Some(profile.clone()),
            (true, _) => {
                return Ok(Outcome::Kept {
                    reason: format!(
                        "{} is not linked to {}",
                        startup.link.display(),
                        startup.profile.as_ref().map_or_else(
                            || "a profile".to_string(),
                            |path| path.display().to_string()
                        )
                    ),
                    fixable: true,
                });
            }
        };
        if login.is_none() && interactive.is_none() && profile.is_none() {
            return Ok(Outcome::Empty);
        }
        let mut login_text = String::new();
        for path in login.iter().chain(profile.as_ref().map(|_| &startup.link)) {
            match self.startup_file(path) {
                Ok(text) => login_text.push_str(&text),
                Err(reason) => return Ok(kept(path, reason, *path == startup.link)),
            }
        }
        let mut interactive_text = String::new();
        if let Some(path) = &interactive {
            match self.startup_file(path) {
                Ok(text) => interactive_text.push_str(&text),
                Err(reason) => return Ok(kept(path, reason, false)),
            }
        }
        let login_block = if login.is_some() || profile.is_some() {
            format!("if [[ -o login ]]; then\n{}fi\n", placeholder(login_text))
        } else {
            String::new()
        };
        let text = format!(
            "if (( ${{+{FLAG}}} )); then\nunset {FLAG}\nsetopt global_rcs\n{login_block}{interactive_text}fi\n"
        );
        if let Err(error) = crate::script::parse(&text) {
            return Ok(kept(startup.dir, format!("does not parse: {error}"), false));
        }
        let mut sources = vec![startup.dir.to_path_buf()];
        sources.extend(login.into_iter().chain(interactive));
        sources.extend(profile.clone());
        Ok(Outcome::Compiled(Section {
            text,
            sources,
            profile,
        }))
    }

    /// The compiled file, or why it stays with zsh.
    fn startup_file(&mut self, path: &Path) -> Result<String, String> {
        let script = super::read_text(path).and_then(Script::parse)?;
        self.stack.push(path.to_path_buf());
        let text = self.compile(
            &script,
            &Cx {
                mode: Mode::Startup,
                origin: Some(path),
                analysis: false,
                source: path.to_str().map(Origin::File),
            },
        );
        self.stack.pop();
        match self.refusals.first() {
            Some(reason) => Err(reason.clone()),
            None => text,
        }
    }
}

fn kept(path: &Path, reason: String, fixable: bool) -> Outcome {
    Outcome::Kept {
        reason: format!("{}: {reason}", path.display()),
        fixable,
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

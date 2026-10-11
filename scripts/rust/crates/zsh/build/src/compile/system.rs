//! The global startup files zsh reads before `.zshrc`, and the user's
//! `.zprofile`, compiled to run from the bundle in zsh's order.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{Compiler, Cx, Mode, Origin, placeholder};
use crate::alias::{Seen, Table};
use crate::script::Script;
use crate::state::State;

/// Set by the guard when zsh skips the global startup files; the bundle and
/// `.zprofile` then run them instead.
pub const FLAG: &str = "_zsh_build_rcs";

/// Global files zsh reads before `.zshrc`; `zlogin` and `zlogout` stay with zsh.
const LOGIN: &str = "zprofile";
const INTERACTIVE: &str = "zshrc";
/// Read ahead of `.zshenv`; never compiled.
const ENV: &str = "zshenv";

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

/// Aliases and the `aliases` option a run of startup files changed.
pub struct AliasChange {
    before: (Table, BTreeSet<String>),
    after: (Table, BTreeSet<String>),
}

impl AliasChange {
    fn new(before: &State, after: &State) -> Self {
        let aliases = |state: &State| (state.alias_table.clone(), state.aliases.clone());
        Self {
            before: aliases(before),
            after: aliases(after),
        }
    }

    /// The same change made to `state`.
    pub fn apply(&self, state: &mut State) {
        state.alias_table.apply(&self.before.0, &self.after.0);
        for name in self.before.1.difference(&self.after.1) {
            state.aliases.remove(name);
        }
        state
            .aliases
            .extend(self.after.1.difference(&self.before.1).cloned());
    }
}

/// The alias changes of zsh's own startup files, around the env files.
pub struct StartupAliases {
    /// `zshenv`, read before them.
    pub zshenv: AliasChange,
    /// `zprofile` and `~/.zprofile` in login shells only, then `zshrc`.
    pub rcs: AliasChange,
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

    /// What zsh's own startup files do to aliases, read with `state` in
    /// zsh's order, whether the bundle or zsh runs them; the compiler is
    /// left as it was, but for the names that may be aliases when zsh
    /// parses the bundle.
    pub fn startup_aliases(&mut self, state: State, startup: &Startup) -> StartupAliases {
        let saved = std::mem::replace(&mut self.state, state);
        let snapshot = self.snapshot();
        let dependencies = self.dependencies.len();
        let emulation_changed = self.emulation_changed;
        let line = self.line;
        self.state.forget("PATH");
        let start = self.state.clone();
        self.analyze_startup(&startup.dir.join(ENV));
        let zshenv = AliasChange::new(&start, &self.state);
        for env in startup.env {
            if self.analyze(env).is_err() {
                self.state.alias_table.forget_all();
            }
        }
        let before = self.state.clone();
        let login: Vec<PathBuf> = [startup.dir.join(LOGIN), startup.link.clone()]
            .into_iter()
            .filter(|path| path.exists())
            .collect();
        if !login.is_empty() {
            for path in &login {
                self.analyze_startup(path);
            }
            self.state.merge(&before);
        }
        self.analyze_startup(&startup.dir.join(INTERACTIVE));
        let rcs = AliasChange::new(&before, &self.state);
        // Set when zsh parses the bundle: with the guard current, the bundle runs the files after it
        let mut seen = Seen::from_table(&before.alias_table);
        seen.extend(Seen::from_table(&self.state.alias_table));
        self.restore(snapshot);
        self.dependencies.truncate(dependencies);
        self.emulation_changed = emulation_changed;
        self.line = line;
        self.state = saved;
        self.alias_seen.extend(seen);
        StartupAliases { zshenv, rcs }
    }

    /// Follows a startup file zsh reads for its effect on state; any alias
    /// may be set by one the build cannot read.
    fn analyze_startup(&mut self, path: &Path) {
        if !path.exists() {
            return;
        }
        let Ok(script) = super::read_text(path).and_then(Script::parse) else {
            self.state.alias_table.forget_all();
            return;
        };
        self.stack.push(path.to_path_buf());
        let result = self.compile(
            &script,
            &Cx {
                mode: Mode::Startup,
                origin: Some(path),
                analysis: true,
                source: None,
            },
        );
        self.stack.pop();
        if result.is_err() {
            self.state.alias_table.forget_all();
        }
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
        let before_login = (self.state.alias_table.clone(), self.state.aliases.clone());
        for path in login.iter().chain(profile.as_ref().map(|_| &startup.link)) {
            match self.startup_file(path) {
                Ok(text) => login_text.push_str(&text),
                Err(reason) => return Ok(kept(path, reason, *path == startup.link)),
            }
        }
        // Login shells only set what these files set
        self.state.alias_table.merge(&before_login.0);
        self.state
            .aliases
            .retain(|name| before_login.1.contains(name));
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

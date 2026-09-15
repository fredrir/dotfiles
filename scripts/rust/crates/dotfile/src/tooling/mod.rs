use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::cli::SyncCli;
use crate::context::Context;
use crate::event::{Event, EventSink, Phase, Summary};

pub mod catalog;
pub mod digest;
pub mod install;
pub mod requirements;

#[derive(Clone)]
pub struct Refresh {
    executable: PathBuf,
    options: Arc<install::Options>,
    replaced: Arc<AtomicBool>,
}

/// The toolchain work a sync has to do before it can reconcile anything: an
/// explicit install, or the rebuild an out-of-date `.bin` implies.
pub fn pending(cli: &SyncCli) -> Result<Option<Refresh>, String> {
    if cli.dry_run {
        return Ok(None);
    }
    let context = Context::discover()?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let options = install::Options {
        rebuild: cli.rebuild,
        ..if cli.native_only {
            install::Options::native()
        } else if cli.commands_only {
            install::Options::commands()
        } else {
            install::Options::everything()
        }
    };
    let requested = cli.installs_only() || cli.rebuild;
    if !requested {
        // Re-execing into a rebuilt binary already installed the toolchain, and a
        // build tree copy of dotfile is not this machine's installed toolchain.
        if std::env::var_os("DOTFILE_REEXECED").is_some()
            || !is_installed(&context.home, &executable)
            || current(&context, &options)?
        {
            return Ok(None);
        }
    }
    Ok(Some(Refresh {
        executable,
        options: Arc::new(options),
        replaced: Arc::new(AtomicBool::new(false)),
    }))
}

impl Refresh {
    pub fn run(&self, events: &dyn EventSink) -> Result<Summary, String> {
        let started = Instant::now();
        let context = Context::discover()?;
        events.emit(Event::PhaseStarted {
            phase: Phase::Tooling,
            total: None,
        });
        if !std::io::stderr().is_terminal() {
            eprintln!("dotfile: updating workstation commands…");
        }
        let _lock = crate::lock::SetupLock::acquire(&context)?;
        let report = install::ensure(&context, &self.options, events)?;
        self.replaced
            .store(report.dotfile_changed, Ordering::SeqCst);
        crate::cancel::check()?;
        Ok(Summary {
            profile: String::new(),
            peer: None,
            remote_changed: None,
            checked: 0,
            changed: report.installed.len(),
            links: 0,
            merges: 0,
            secrets: 0,
            generated: 0,
            dry_run: false,
            elapsed: started.elapsed(),
        })
    }

    /// Only the binary running this sync needs a restart to take effect.
    pub fn replaced_running_binary(&self) -> bool {
        self.replaced.load(Ordering::SeqCst)
    }

    pub fn reexec(&self, arguments: &[OsString]) -> Result<(), String> {
        reexec(&self.executable, arguments)
    }
}

/// Whether every compiled command in `.bin` was built from the current sources.
pub(crate) fn native_current() -> Result<bool, String> {
    let context = Context::discover()?;
    current(&context, &install::Options::native())
}

fn current(context: &Context, options: &install::Options) -> Result<bool, String> {
    let toolchain = catalog::Toolchain::read(&context.root)?;
    let bin = install::binary_dir(context);
    let stamps = context.root_config.join("sync");
    for language in &options.languages {
        let Some(stage) = toolchain.stage(*language) else {
            continue;
        };
        if !stage.binaries.iter().all(|name| bin.join(name).is_file()) {
            return Ok(false);
        }
        let saved = fs::read_to_string(stamps.join(language.key())).unwrap_or_default();
        if saved.trim() != digest::of(&stage.inputs)? {
            return Ok(false);
        }
    }
    Ok(install::completions_current(context))
}

fn is_installed(home: &Path, executable: &Path) -> bool {
    let bin = home.join("dotfiles/.bin");
    executable.parent().is_some_and(|parent| {
        parent == bin
            || fs::canonicalize(parent).ok() == fs::canonicalize(&bin).ok()
                && fs::canonicalize(&bin).is_ok()
    })
}

fn reexec(executable: &Path, arguments: &[OsString]) -> Result<(), String> {
    let mut command = Command::new(executable);
    command.args(arguments).env("DOTFILE_REEXECED", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(format!(
            "cannot restart updated dotfile: {}",
            command.exec()
        ))
    }
    #[cfg(not(unix))]
    {
        let status = command.status().map_err(|error| error.to_string())?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tooling_tests.rs"]
mod tests;

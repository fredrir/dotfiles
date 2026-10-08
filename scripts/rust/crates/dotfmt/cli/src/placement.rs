use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use dotfmt::render::{self, Placement, Source};
use workstation::{Answer, Style};

use crate::args::Cli;

const NAME: &str = "dotfmt.dotfile";
const BUNDLED: &str = include_str!("../../../../../../shared/tools/dotfmt.dotfile");

pub fn run(cli: &Cli) -> Result<ExitCode, String> {
    let repository = std::env::var_os("DOTFILE_ROOT")
        .map(PathBuf::from)
        .map(|root| root.join("shared/tools").join(NAME));
    let (body, source) = match repository.filter(|path| path.is_file()) {
        Some(path) => (
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?,
            Source::Repo,
        ),
        None => (BUNDLED.to_owned(), Source::Embedded),
    };
    let defaults = [PathBuf::from(".")];
    let targets = if cli.targets.is_empty() {
        &defaults[..]
    } else {
        &cli.targets
    };
    let mut all = false;
    for target in targets {
        workstation::path::require_directory(target)?;
        let path = target.join(NAME);
        let exists = path.exists();
        if cli.sync && !exists {
            return Err(format!(
                "{}: no configuration to sync; use --add first",
                path.display()
            ));
        }
        if cli.add && !all {
            let verb = if exists { "Replace" } else { "Copy" };
            match workstation::confirm_each(&format!("{verb} {}? [Y/n/a] ", path.display())) {
                Some(Answer::All) => all = true,
                Some(Answer::Yes) => {}
                Some(Answer::No) | None => continue,
            }
        }
        if exists {
            dotfmt_core::file::replace(&path, body.as_bytes())
        } else {
            fs::write(&path, &body)
        }
        .map_err(|error| format!("{}: {error}", path.display()))?;
        if !cli.quiet {
            let style = Style::for_stderr();
            if cli.verbose {
                for line in render::heading(
                    "dotfmt",
                    target,
                    if cli.sync { "sync" } else { "add" },
                    &style,
                ) {
                    eprintln!("{line}");
                }
            }
            let placement = Placement { name: NAME, exists };
            for line in render::placed(&[&placement], &style) {
                eprintln!("{line}");
            }
            if let Some(line) = render::provenance(&source, &style) {
                eprintln!("{line}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

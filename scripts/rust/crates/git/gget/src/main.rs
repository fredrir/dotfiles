mod fetch;
mod list;
mod target;

use std::env;
use std::fs;
use std::process::ExitCode;

use clap::Parser;
use workstation::{Completions, Style};

use target::Target;

const PROGRAM: &str = "gget";

const OWNER: &str = "fredrir";

const ARROW: &str = " -> ";

#[derive(Parser)]
#[command(
    version,
    about = "Download a file or folder out of a GitHub repository",
    long_about = "Download a file or folder out of a GitHub repository into the current directory.",
    after_long_help = "Examples:
  gget https://github.com/user/repo/folder_8/folder_9    From the default branch
  gget https://github.com/user/repo/tree/dev/src         From the branch in the URL
  gget -f nsql/README.md                                 From github.com/fredrir
  gget -f -b dev nsql/README.md                          From that repository's dev
  gget user/repo                                         The whole repository, as files
  gget -l user/repo/tree/main/src                        List it instead of downloading
  gget -la user/repo                                     List it, dotfiles and all"
)]
struct Cli {
    #[arg(value_name = "TARGET", required_unless_present = "shell")]
    target: Option<String>,

    #[arg(short, long)]
    fredrir: bool,

    #[arg(short, long, value_name = "BRANCH")]
    branch: Option<String>,

    #[arg(short, long)]
    yes: bool,

    #[arg(short, long)]
    list: bool,

    #[arg(short, long, requires = "list")]
    all: bool,

    #[command(flatten)]
    completions: Completions,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    match get(&cli) {
        Ok(status) => status,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

fn get(cli: &Cli) -> Result<ExitCode, String> {
    // clap has already refused a run with neither this nor --completions.
    let input = cli.target.as_deref().unwrap_or_default();
    let mut target = target::parse(input, cli.fredrir.then_some(OWNER))?;
    // A `tree/<branch>` URL and `--branch` answer the same question, and the
    // one that was typed just now wins.
    if cli.branch.is_some() {
        target.reference = cli.branch.clone();
    }

    if cli.list {
        return list::list(&target, cli.all);
    }

    let here = env::current_dir().map_err(|error| format!("the current directory: {error}"))?;
    let destination = here.join(target.name());
    let style = Style::for_stdout();

    if fs::symlink_metadata(&destination).is_ok() && !cli.yes {
        println!(
            "  {} {} with {}",
            style.red("overwrite"),
            target.name(),
            describe(&target, None)
        );
        match workstation::confirm("  Continue? [Y/n] ") {
            Some(true) => {}
            Some(false) => {
                println!("{PROGRAM}: cancelled");
                return Ok(ExitCode::SUCCESS);
            }
            // The answers ran out; leave the prompt's line and stop.
            None => {
                println!();
                return Ok(ExitCode::FAILURE);
            }
        }
    }

    match fetch::fetch(&target, &here)? {
        // git has said what was wrong with it; its status is the answer.
        fetch::Outcome::Refused(code) => Ok(ExitCode::from(byte(code))),
        fetch::Outcome::Ready(fetched) => {
            let source = describe(&target, fetched.branch());
            fetched.install(&destination)?;
            println!(
                "  {}{ARROW}{}",
                style.dim(&source),
                style.teal(target.name())
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn describe(target: &Target, branch: Option<&str>) -> String {
    let source = fetch::reported(target, branch);
    if target.path.is_empty() {
        source
    } else {
        format!("{source} {}", target.path)
    }
}

fn byte(code: i32) -> u8 {
    u8::try_from(code).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_statuses_pass_through() {
        assert_eq!(byte(0), 0);
        assert_eq!(byte(1), 1);
        assert_eq!(byte(128), 128);
    }

    #[test]
    fn statuses_outside_a_byte_still_fail() {
        assert_eq!(byte(-1), 1);
        assert_eq!(byte(300), 1);
    }
}

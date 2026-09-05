use clap::{Args, Parser};
use workstation::{Completable, Completions};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Push,
    Pull,
}

impl Direction {
    pub fn program(self) -> &'static str {
        match self {
            Direction::Push => "hpush",
            Direction::Pull => "hpull",
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            Direction::Push => "push",
            Direction::Pull => "pull",
        }
    }
}

#[derive(Args)]
pub struct Common {
    #[arg(value_name = "PATH")]
    pub path: Option<String>,

    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    #[arg(short = 'c', long = "checksum")]
    pub checksum: bool,

    #[arg(short = 'a', long = "all", alias = "no-excludes")]
    pub all: bool,

    #[arg(short = 'y', long = "yes")]
    pub yes: bool,

    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    #[command(flatten)]
    pub completions: Completions,
}

#[derive(Parser)]
#[command(
    name = "hpush",
    version,
    about = "Copy a path from this machine to the same place on the other one",
    long_about = "Copy a path from this machine to the same place on the other one.

With no destination given, the other machine's filesystem is browsed for one,
starting on the mirrored location. Files the working tree ignores are left
behind unless --all says otherwise.",
    after_long_help = "Examples:
  hpush                      Push this directory, choosing where it lands
  hpush .tmux.conf           Push one file, choosing where it lands
  hpush go --yes             Push straight to the mirrored path, asking nothing
  hpush go --to ~/scratch    Push into a named directory over there
  hpush --dry-run            Show what would be transferred and stop
  hpush --all                Include ignored files, .git, and the excluded list"
)]
pub struct Push {
    #[command(flatten)]
    pub common: Common,

    #[arg(long = "to", value_name = "PATH")]
    pub to: Option<String>,
}

#[derive(Parser)]
#[command(
    name = "hpull",
    version,
    about = "Copy a path from the other machine to the same place on this one",
    long_about = "Copy a path from the other machine to the same place on this one.

With no source given, the other machine's filesystem is browsed for one,
starting on the mirrored location. Files the working tree ignores are left
behind unless --all says otherwise.",
    after_long_help = "Examples:
  hpull                        Browse the other machine and pull what is chosen
  hpull notes.md               Pull the matching path, choosing which one
  hpull go --yes               Pull the mirrored path, asking nothing
  hpull --from ~/scratch/go    Pull a named path from over there
  hpull --dry-run              Show what would be transferred and stop
  hpull --all                  Include ignored files, .git, and the excluded list"
)]
pub struct Pull {
    #[command(flatten)]
    pub common: Common,

    #[arg(long = "from", value_name = "PATH")]
    pub from: Option<String>,
}

impl Completable for Push {
    fn completions(&self) -> &Completions {
        &self.common.completions
    }
}

impl Completable for Pull {
    fn completions(&self) -> &Completions {
        &self.common.completions
    }
}

// What the two parsers agree on once their one asymmetric flag is read off.
pub struct Request {
    pub direction: Direction,
    pub path: Option<String>,
    pub remote: Option<String>,
    pub dry_run: bool,
    pub checksum: bool,
    pub all: bool,
    pub yes: bool,
    pub verbose: bool,
}

impl Request {
    fn new(direction: Direction, common: Common, remote: Option<String>) -> Request {
        Request {
            direction,
            path: common.path,
            remote,
            dry_run: common.dry_run,
            checksum: common.checksum,
            all: common.all,
            yes: common.yes,
            verbose: common.verbose,
        }
    }
}

impl From<Push> for Request {
    fn from(cli: Push) -> Request {
        Request::new(Direction::Push, cli.common, cli.to)
    }
}

impl From<Pull> for Request {
    fn from(cli: Pull) -> Request {
        Request::new(Direction::Pull, cli.common, cli.from)
    }
}

#[cfg(test)]
#[path = "../tests/unit/cli_tests.rs"]
mod tests;

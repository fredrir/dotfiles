use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use workstation::{Completable, Completions};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(
        long,
        value_name = "PATH",
        help = "Repository root (default: DOTFILE_ROOT, current repository, build repository)"
    )]
    root: Option<PathBuf>,
    #[arg(long, help = "Report stale pages without writing")]
    check: bool,
    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run::<Cli>("doc-keybinds", |cli| {
        let root = doc_keybinds::root(cli.root)?;
        let changed = doc_keybinds::generate(&root, cli.check)?;
        for path in &changed {
            println!(
                "{} {}",
                if cli.check { "drifted" } else { "updated" },
                path.display()
            );
        }
        if changed.is_empty() {
            println!("docs/keybinds is current");
        }
        Ok(if cli.check && !changed.is_empty() {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        })
    })
}

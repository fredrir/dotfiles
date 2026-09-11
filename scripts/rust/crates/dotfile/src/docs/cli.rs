use std::process::ExitCode;

use crate::context::Context;
use clap::ValueEnum;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, ValueEnum)]
pub enum Target {
    Cli,
    Keybinds,
    Packages,
    Readme,
    Benchmarks,
}

#[derive(clap::Args)]
pub struct Args {
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        help = "Select outputs; defaults to cli,keybinds,packages"
    )]
    pub only: Vec<Target>,
    #[arg(
        long,
        conflicts_with = "dry_run",
        help = "Check without writing; exit 1 on drift or missing metadata"
    )]
    pub check: bool,
    #[arg(short = 'n', long, help = "Show changes without writing")]
    pub dry_run: bool,
    #[arg(long, help = "Show unified changes without writing")]
    pub diff: bool,
    #[arg(long, help = "Print the change report as JSON")]
    pub json: bool,
}

pub fn run(context: &Context, args: Args) -> Result<ExitCode, String> {
    let mut targets = if args.only.is_empty() {
        vec![Target::Cli, Target::Keybinds, Target::Packages]
    } else {
        args.only
    };
    targets.sort();
    targets.dedup();
    let preview = args.check || args.dry_run || args.diff;
    let _lock = if preview {
        None
    } else {
        Some(crate::lock::MutationLock::acquire(context)?)
    };
    if !preview {
        crate::fs::transaction::recover(context)?;
    }
    let (plan, missing) = super::prepare(context, &targets, false)?;
    let paths = plan.paths();
    let changes = plan.report(args.diff);
    if !preview {
        plan.apply(&context.root)?;
    }
    if args.json {
        let mode = if args.check {
            "check"
        } else if args.dry_run {
            "dry-run"
        } else if args.diff {
            "diff"
        } else {
            "write"
        };
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"version":1,"mode":mode,"changes":changes,"missing_metadata":missing,"current":paths.is_empty() && missing.is_empty()})).map_err(|e| e.to_string())?);
    } else {
        for name in &missing {
            eprintln!("{name}: command metadata unavailable; documentation retained");
        }
        if args.diff {
            for change in changes {
                if let Some(diff) = change.diff {
                    print!("{diff}");
                }
            }
        } else {
            for path in &paths {
                println!(
                    "{} {}",
                    if args.check {
                        "drifted"
                    } else if args.dry_run {
                        "would update"
                    } else {
                        "updated"
                    },
                    path.display()
                );
            }
        }
        if paths.is_empty() && missing.is_empty() {
            println!("documentation is current");
        }
    }
    Ok(
        if args.check && (!paths.is_empty() || !missing.is_empty()) {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        },
    )
}

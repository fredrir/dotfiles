use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use workstation::Completions;

const PROGRAM: &str = "path";

#[derive(Parser)]
#[command(
    version,
    about = "Print the repository-relative or home-relative path of a target"
)]
struct Cli {
    #[arg(value_hint = ValueHint::AnyPath, default_value = ".")]
    target: PathBuf,

    #[arg(short = 'f', long = "full")]
    full: bool,

    #[command(flatten)]
    completions: Completions,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    let resolved = real_path(&cli.target);
    if cli.full {
        println!("{}", resolved.display());
        return ExitCode::SUCCESS;
    }
    let root = repository_root(&resolved);
    let home = std::env::var_os("HOME").map(|home| real_path(Path::new(&home)));
    println!("{}", describe(&resolved, root.as_deref(), home.as_deref()));
    ExitCode::SUCCESS
}

fn describe(resolved: &Path, root: Option<&Path>, home: Option<&Path>) -> String {
    if let Some(inside) = root.and_then(|root| relative(resolved, root)) {
        return format!("/{inside}");
    }
    match home.and_then(|home| relative(resolved, home)) {
        Some(inside) if inside.is_empty() => "~".to_string(),
        Some(inside) => format!("~/{inside}"),
        None => resolved.display().to_string(),
    }
}

fn relative(target: &Path, base: &Path) -> Option<String> {
    let inside = target.strip_prefix(base).ok()?;
    Some(inside.to_string_lossy().into_owned())
}

fn repository_root(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|ancestor| fs::symlink_metadata(ancestor.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

fn real_path(target: &Path) -> PathBuf {
    let absolute = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(target)
    };
    let parts: Vec<Component> = absolute.components().collect();
    for split in (1..=parts.len()).rev() {
        let head: PathBuf = parts[..split].iter().collect();
        if let Ok(resolved) = fs::canonicalize(&head) {
            return extend(resolved, &parts[split..]);
        }
    }
    extend(PathBuf::new(), &parts)
}

fn extend(mut base: PathBuf, rest: &[Component]) -> PathBuf {
    for part in rest {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                base.pop();
            }
            other => base.push(other),
        }
    }
    base
}

#[cfg(test)]
#[path = "../tests/unit/main_tests.rs"]
mod tests;

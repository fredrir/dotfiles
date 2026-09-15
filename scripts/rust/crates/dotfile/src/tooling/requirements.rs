use std::io::IsTerminal;
use std::process::Stdio;

use crate::context::Context;

const SECRET_TOOLS: [&str; 3] = ["age", "age-keygen", "sops"];

struct Manager {
    program: &'static str,
    arguments: &'static [&'static str],
    elevated: bool,
}

const MANAGERS: [Manager; 3] = [
    Manager {
        program: "brew",
        arguments: &["install"],
        elevated: false,
    },
    Manager {
        program: "pacman",
        arguments: &["-S", "--needed"],
        elevated: true,
    },
    Manager {
        program: "apt-get",
        arguments: &["install", "-y"],
        elevated: true,
    },
];

pub fn secret_tools(context: &Context) -> Result<(), String> {
    if !uses_secrets(context) {
        return Ok(());
    }
    ensure(context, &SECRET_TOOLS)
}

/// Offers to install anything missing; a declined or impossible install is not fatal.
pub fn ensure(context: &Context, tools: &[&str]) -> Result<(), String> {
    let missing: Vec<&str> = tools
        .iter()
        .copied()
        .filter(|tool| context.program(tool).is_none())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let packages = packages_for(&missing);
    let Some(manager) = MANAGERS
        .iter()
        .find(|manager| context.program(manager.program).is_some())
    else {
        eprintln!(
            "dotfile: {} not installed and no known package manager on PATH",
            missing.join(", ")
        );
        return Ok(());
    };
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "dotfile: {} not installed; install with {}",
            missing.join(", "),
            command_line(manager, &packages)
        );
        return Ok(());
    }
    println!("{} not installed on this machine", missing.join(", "));
    let question = format!("install with {}? [Y/n] ", command_line(manager, &packages));
    if workstation::confirm(&question) != Some(true) {
        return Ok(());
    }
    install(context, manager, &packages)?;
    let still_missing: Vec<&str> = missing
        .into_iter()
        .filter(|tool| context.program(tool).is_none())
        .collect();
    if still_missing.is_empty() {
        return Ok(());
    }
    eprintln!("dotfile: still not on PATH: {}", still_missing.join(", "));
    Ok(())
}

fn install(context: &Context, manager: &Manager, packages: &[&str]) -> Result<(), String> {
    let mut command = if manager.elevated {
        let mut command = context.command("sudo");
        command.arg(manager.program);
        command
    } else {
        context.command(manager.program)
    };
    command
        .args(manager.arguments)
        .args(packages)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let status = command
        .status()
        .map_err(|error| format!("{} could not run: {error}", manager.program))?;
    if !status.success() {
        eprintln!("dotfile: {} did not install cleanly", manager.program);
    }
    Ok(())
}

fn packages_for(missing: &[&str]) -> Vec<&'static str> {
    let mut packages = Vec::new();
    for tool in missing {
        let package = match *tool {
            "age" | "age-keygen" => "age",
            "sops" => "sops",
            other => Box::leak(other.to_string().into_boxed_str()),
        };
        if !packages.contains(&package) {
            packages.push(package);
        }
    }
    packages
}

fn command_line(manager: &Manager, packages: &[&str]) -> String {
    let prefix = if manager.elevated { "sudo " } else { "" };
    format!(
        "{prefix}{} {} {}",
        manager.program,
        manager.arguments.join(" "),
        packages.join(" ")
    )
}

fn uses_secrets(context: &Context) -> bool {
    context.root_config.join("keys.dotfile").is_file() || context.root.join(".sops.yaml").is_file()
}

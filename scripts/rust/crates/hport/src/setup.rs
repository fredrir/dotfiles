use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use hostkit::Host;
use workstation::Style;

use crate::config::home;
use crate::forward::{ALIAS, alias_ready};

const HOSTS: &str = "/etc/hosts";
const ALIAS_LABEL: &str = "com.fredrir.hport.alias";
const ALIAS_PLIST: &str = "/Library/LaunchDaemons/com.fredrir.hport.alias.plist";
const ALIAS_PLIST_TEXT: &str = include_str!("../assets/com.fredrir.hport.alias.plist");
const AGENT_LABEL: &str = "com.fredrir.hport";
const UNIT: &str = "hport.service";

pub fn run(style: &Style, dry_run: bool) -> Result<(), String> {
    let this = Host::this()?;
    let peer = this.peer().name();
    let hosts = std::fs::read_to_string(HOSTS).map_err(|error| format!("{HOSTS}: {error}"))?;
    step(
        style,
        dry_run,
        &format!("{peer} → {ALIAS}"),
        HOSTS,
        maps(&hosts, peer),
        || {
            sudo(&["tee", "-a", HOSTS], Some(&append(&hosts, peer)))?;
            if this == Host::Macie {
                sudo(&["dscacheutil", "-flushcache"], None)?;
                sudo(&["killall", "-HUP", "mDNSResponder"], None)?;
            }
            Ok(())
        },
    )?;
    if this == Host::Macie {
        let installed =
            std::fs::read_to_string(ALIAS_PLIST).is_ok_and(|text| text == ALIAS_PLIST_TEXT);
        step(
            style,
            dry_run,
            &format!("{ALIAS} on lo0"),
            ALIAS_PLIST,
            installed && alias_ready(),
            || {
                sudo(&["tee", ALIAS_PLIST], Some(ALIAS_PLIST_TEXT))?;
                let _ = sudo(
                    &["launchctl", "bootout", &format!("system/{ALIAS_LABEL}")],
                    None,
                );
                sudo(&["launchctl", "bootstrap", "system", ALIAS_PLIST], None)
            },
        )?;
    }
    match this {
        Host::Macie => agent(style, dry_run),
        Host::Archie => unit(style, dry_run),
    }
}

fn agent(style: &Style, dry_run: bool) -> Result<(), String> {
    let plist = home()?.join(format!("Library/LaunchAgents/{AGENT_LABEL}.plist"));
    require(&plist)?;
    let domain = format!("gui/{}", nix::unistd::getuid());
    let service = format!("{domain}/{AGENT_LABEL}");
    let running = quiet(Command::new("launchctl").args(["print", &service]));
    step(style, dry_run, "daemon", AGENT_LABEL, running, || {
        let plist = plist.to_string_lossy();
        run_checked(Command::new("launchctl").args(["bootstrap", &domain, &plist]))
    })
}

fn unit(style: &Style, dry_run: bool) -> Result<(), String> {
    require(&home()?.join(".config/systemd/user").join(UNIT))?;
    let running = quiet(Command::new("systemctl").args(["--user", "is-active", "--quiet", UNIT]));
    step(style, dry_run, "daemon", UNIT, running, || {
        run_checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        run_checked(Command::new("systemctl").args(["--user", "enable", "--now", UNIT]))
    })
}

fn step(
    style: &Style,
    dry_run: bool,
    label: &str,
    detail: &str,
    done: bool,
    apply: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let line = |mark: String| println!("{mark} {label:<20} {}", style.dim(detail));
    if done {
        line(style.green("✓"));
        return Ok(());
    }
    line(style.teal("+"));
    if dry_run { Ok(()) } else { apply() }
}

pub fn maps(hosts: &str, name: &str) -> bool {
    let alias = ALIAS.to_string();
    hosts
        .lines()
        .filter_map(|line| line.split('#').next())
        .any(|line| {
            let mut fields = line.split_whitespace();
            fields.next() == Some(alias.as_str()) && fields.any(|field| field == name)
        })
}

pub fn append(hosts: &str, name: &str) -> String {
    let separator = if hosts.is_empty() || hosts.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    format!("{separator}{ALIAS} {name}\n")
}

fn require(path: &Path) -> Result<(), String> {
    if path.exists() {
        Ok(())
    } else {
        Err(format!("{} not found; run dotfile sync", path.display()))
    }
}

fn sudo(args: &[&str], input: Option<&str>) -> Result<(), String> {
    let mut command = Command::new("sudo");
    command
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("sudo {}: {error}", args.join(" ")))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(input.as_bytes())
            .map_err(|error| format!("sudo {}: {error}", args.join(" ")))?;
    }
    let status = child
        .wait()
        .map_err(|error| format!("sudo {}: {error}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("sudo {} failed ({status})", args.join(" ")))
    }
}

fn quiet(command: &mut Command) -> bool {
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn run_checked(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(hostkit::ssh::stderr_reason(
        &output.stderr,
        &format!("{program} failed"),
    ))
}

#[cfg(test)]
#[path = "../tests/unit/setup_tests.rs"]
mod tests;

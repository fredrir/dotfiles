use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use hostkit::Host;
use sha2::{Digest, Sha256};
use workstation::Style;

pub const DEFAULT_IDENTITY: &str = "Developer ID Application";

const BUNDLE_ID: &str = "com.fredrir.op-bridge";
const LABEL: &str = "com.fredrir.op-bridge";
const INFO_PLIST: &str = include_str!("../assets/Info.plist");
const STAMP: &str = "Contents/Resources/source.sha256";
const EXECUTABLE: &str = "Contents/MacOS/op-bridge";
const BOOTOUT_WAIT: Duration = Duration::from_secs(10);

pub struct Paths {
    pub app: PathBuf,
    pub plist: PathBuf,
}

impl Paths {
    pub fn under(home: &Path) -> Paths {
        Paths {
            app: home.join("Applications/op-bridge.app"),
            plist: home.join(format!("Library/LaunchAgents/{LABEL}.plist")),
        }
    }

    pub fn executable(&self) -> PathBuf {
        self.app.join(EXECUTABLE)
    }
}

pub fn run(style: &Style, dry_run: bool, identity: &str) -> Result<(), String> {
    if Host::this()? != Host::Macie {
        return Err("setup runs on macie; archie only needs dotfile sync".to_string());
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let paths = Paths::under(Path::new(&home));
    let source = std::env::current_exe().map_err(|error| format!("current executable: {error}"))?;
    let digest = sha256(&source)?;

    let installed = stamp(&paths.app).as_deref() == Some(digest.as_str()) && verified(&paths.app);
    step(style, dry_run, "app", &paths.app, installed, || {
        install(&paths.app, &source, &digest, identity)
    })?;

    if !paths.plist.exists() {
        return Err(format!(
            "{} not found; run dotfile sync",
            paths.plist.display()
        ));
    }
    let domain = format!("gui/{}", nix::unistd::getuid());
    let service = format!("{domain}/{LABEL}");
    let running = installed && runs(&service, &paths.executable());
    step(style, dry_run, "daemon", &paths.plist, running, || {
        restart(&domain, &service, &paths.plist)
    })
}

fn install(app: &Path, source: &Path, digest: &str, identity: &str) -> Result<(), String> {
    let staged = app.with_extension("app.new");
    let display = |path: &Path| {
        let path = path.display().to_string();
        move |error: std::io::Error| format!("{path}: {error}")
    };
    if staged.exists() {
        fs::remove_dir_all(&staged).map_err(display(&staged))?;
    }
    let executable = staged.join(EXECUTABLE);
    for directory in [executable.parent(), staged.join(STAMP).parent()]
        .into_iter()
        .flatten()
    {
        fs::create_dir_all(directory).map_err(display(directory))?;
    }
    fs::copy(source, &executable).map_err(display(&executable))?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .map_err(display(&executable))?;
    fs::write(staged.join("Contents/Info.plist"), INFO_PLIST).map_err(display(&staged))?;
    fs::write(staged.join(STAMP), digest).map_err(display(&staged))?;

    checked(Command::new("codesign").args(sign_args(identity, &staged)))?;
    if app.exists() {
        fs::remove_dir_all(app).map_err(display(app))?;
    }
    fs::rename(&staged, app).map_err(display(app))
}

pub fn sign_args(identity: &str, app: &Path) -> Vec<String> {
    [
        "--force",
        "--sign",
        identity,
        "--identifier",
        BUNDLE_ID,
        "--timestamp=none",
    ]
    .map(str::to_string)
    .into_iter()
    .chain([app.display().to_string()])
    .collect()
}

// launchd keeps the plist it loaded, so a changed plist needs bootout, not kickstart
fn restart(domain: &str, service: &str, plist: &Path) -> Result<(), String> {
    if quiet(Command::new("launchctl").args(["print", service])) {
        checked(Command::new("launchctl").args(["bootout", service]))?;
        let deadline = Instant::now() + BOOTOUT_WAIT;
        while quiet(Command::new("launchctl").args(["print", service])) {
            if Instant::now() >= deadline {
                return Err(format!("{service} still loaded after bootout"));
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    checked(
        Command::new("launchctl")
            .arg("bootstrap")
            .arg(domain)
            .arg(plist),
    )
}

fn runs(service: &str, executable: &Path) -> bool {
    Command::new("launchctl")
        .args(["print", service])
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && program(&String::from_utf8_lossy(&output.stdout)) == Some(executable)
        })
}

pub fn program(print: &str) -> Option<&Path> {
    print
        .lines()
        .find_map(|line| line.trim().strip_prefix("program = "))
        .map(Path::new)
}

fn stamp(app: &Path) -> Option<String> {
    fs::read_to_string(app.join(STAMP)).ok()
}

fn verified(app: &Path) -> bool {
    quiet(
        Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(app),
    )
}

fn sha256(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn step(
    style: &Style,
    dry_run: bool,
    label: &str,
    detail: &Path,
    done: bool,
    apply: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let detail = workstation::path::home_relative(detail);
    let line = |mark: String| println!("{mark} {label:<8} {}", style.dim(&detail));
    if done {
        line(style.green("✓"));
        return Ok(());
    }
    line(style.teal("+"));
    if dry_run { Ok(()) } else { apply() }
}

fn quiet(command: &mut Command) -> bool {
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

// Inherits the terminal, so a keychain prompt from codesign can be answered
fn checked(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .stdin(Stdio::inherit())
        .status()
        .map_err(|error| format!("{program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed ({status})"))
    }
}

#[cfg(test)]
#[path = "../tests/unit/setup_tests.rs"]
mod tests;

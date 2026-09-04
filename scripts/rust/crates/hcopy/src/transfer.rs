use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hostkit::Route;

use crate::cli::Direction;
use crate::place;

const PROGRAM: &str = "rsync";
const FORMAT: &str = "--out-format=%i|%n|%l";
const GITIGNORE: &str = "--filter=:- .gitignore";
const TRANSPORT: &str = "ssh -o ConnectTimeout=8";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteArguments {
    Protected,
    ShellQuoted,
}

impl RemoteArguments {
    fn path(self, path: &str) -> String {
        match self {
            Self::Protected => path.to_string(),
            Self::ShellQuoted => place::quote(path),
        }
    }
}

pub struct Plan {
    pub direction: Direction,
    pub host: String,
    pub local: PathBuf,
    pub local_display: String,
    pub remote: String,
    pub remote_display: String,
    pub route: Option<Route>,
    pub dry_run: bool,
    pub checksum: bool,
    pub all: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub files: usize,
    pub created: usize,
    pub bytes: u64,
    pub elapsed: Duration,
    pub lines: Vec<String>,
}

impl Outcome {
    pub fn quiet(&self) -> bool {
        self.files == 0 && self.created == 0
    }
}

impl Plan {
    fn source(&self, remote_arguments: RemoteArguments) -> String {
        match self.direction {
            Direction::Push => self.local.to_string_lossy().into_owned(),
            Direction::Pull => format!("{}:{}", self.host, remote_arguments.path(&self.remote)),
        }
    }

    fn destination(&self, remote_arguments: RemoteArguments) -> String {
        match self.direction {
            Direction::Push => format!(
                "{}:{}/",
                self.host,
                remote_arguments.path(place::parent_of(&self.remote))
            ),
            Direction::Pull => format!("{}/", self.local_parent().to_string_lossy()),
        }
    }

    pub fn local_parent(&self) -> PathBuf {
        self.local
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    // The delta algorithm is a trade of processor time against link time, and
    // over the cable there is no link time worth buying.
    fn whole_file(&self) -> bool {
        matches!(
            self.route,
            Some(Route::Cable) | Some(Route::Wifi) | Some(Route::Lan)
        )
    }

    fn arguments(&self, remote_arguments: RemoteArguments) -> Vec<String> {
        let mut found = vec![
            "-a".to_string(),
            "-i".to_string(),
            FORMAT.to_string(),
            "-e".to_string(),
            TRANSPORT.to_string(),
        ];
        if remote_arguments == RemoteArguments::Protected {
            found.push("--no-old-args".into());
        }
        if self.dry_run {
            found.push("-n".into());
        }
        if self.checksum {
            found.push("-c".into());
        }
        if self.whole_file() {
            found.push("-W".into());
        }
        if !self.all {
            found.push(GITIGNORE.into());
            found.push("--exclude=.git/".into());
            if let Some(file) = excludes()
                && let Some(text) = file.to_str()
            {
                found.push(format!("--exclude-from={text}"));
            }
        }
        found.push("--".into());
        found.push(self.source(remote_arguments));
        found.push(self.destination(remote_arguments));
        found
    }
}

fn remote_arguments() -> Result<RemoteArguments, String> {
    let output = Command::new(PROGRAM)
        .arg("--help")
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("{PROGRAM} is not installed"),
            _ => format!("{PROGRAM}: {error}"),
        })?;
    if !output.status.success() {
        return Err(explain(
            &String::from_utf8_lossy(&output.stderr),
            output.status.code(),
        ));
    }
    Ok(remote_arguments_from_help(&output.stdout, &output.stderr))
}

fn remote_arguments_from_help(stdout: &[u8], stderr: &[u8]) -> RemoteArguments {
    let supports_old_args = [stdout, stderr].into_iter().any(|text| {
        text.split(u8::is_ascii_whitespace)
            .any(|word| word == b"--old-args")
    });
    match supports_old_args {
        true => RemoteArguments::Protected,
        false => RemoteArguments::ShellQuoted,
    }
}

pub fn excludes() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    let file = base.join("rsync/excludes");
    file.is_file().then_some(file)
}

pub fn run(plan: &Plan, mut progress: impl FnMut(&Outcome)) -> Result<Outcome, String> {
    let started = Instant::now();
    let remote_arguments = remote_arguments()?;
    let mut child = Command::new(PROGRAM)
        .args(plan.arguments(remote_arguments))
        // The selected argument encoding must not be changed behind our back.
        .env_remove("RSYNC_OLD_ARGS")
        .env_remove("RSYNC_PROTECT_ARGS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("{PROGRAM} is not installed"),
            _ => format!("{PROGRAM}: {error}"),
        })?;

    let complaints = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        })
    });

    let mut outcome = Outcome::default();
    if let Some(stdout) = child.stdout.take() {
        let mut shown = Instant::now();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            absorb(&mut outcome, &line);
            if shown.elapsed() >= Duration::from_millis(60) {
                shown = Instant::now();
                progress(&outcome);
            }
        }
    }

    let status = child
        .wait()
        .map_err(|error| format!("{PROGRAM}: {error}"))?;
    outcome.elapsed = started.elapsed();
    let stderr = complaints
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    if !status.success() {
        return Err(explain(&stderr, status.code()));
    }
    Ok(outcome)
}

fn explain(stderr: &str, code: Option<i32>) -> String {
    let reason = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| !line.starts_with("rsync error:"))
        .or_else(|| stderr.lines().map(str::trim).find(|line| !line.is_empty()));
    match reason {
        Some(text) => text.to_string(),
        None => match code {
            Some(code) => format!("{PROGRAM} exited with status {code}"),
            None => format!("{PROGRAM} was killed"),
        },
    }
}

// `%i|%n|%l`, split from both ends so that a name with a bar in it stays whole.
fn absorb(outcome: &mut Outcome, line: &str) {
    let Some((flags, rest)) = line.split_once('|') else {
        return;
    };
    let Some((name, size)) = rest.rsplit_once('|') else {
        return;
    };
    let mut marks = flags.chars();
    let (Some(change), Some(shape)) = (marks.next(), marks.next()) else {
        return;
    };
    if !matches!(change, '<' | '>' | 'c') {
        return;
    }
    if change == 'c' && shape != 'd' {
        return;
    }
    let fresh = flags.contains('+');
    if shape == 'd' {
        if fresh {
            outcome.created += 1;
        }
    } else {
        outcome.files += 1;
        if fresh {
            outcome.created += 1;
        }
        outcome.bytes += size.trim().parse::<u64>().unwrap_or(0);
    }
    outcome.lines.push(format!("{flags} {name}"));
}

pub fn erase(target: &mut impl Write) {
    let _ = write!(target, "\r\x1b[2K");
    let _ = target.flush();
}

#[cfg(test)]
#[path = "../tests/unit/transfer_tests.rs"]
mod tests;

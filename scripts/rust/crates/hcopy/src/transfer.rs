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
mod tests {
    use super::*;

    fn plan(direction: Direction) -> Plan {
        Plan {
            direction,
            host: "archie".into(),
            local: PathBuf::from("/Users/fredrir/projects/my-app"),
            local_display: "~/projects/my-app".into(),
            remote: "/home/fredrir/projects/my-app".into(),
            remote_display: "~/projects/my-app".into(),
            route: Some(Route::Cable),
            dry_run: false,
            checksum: false,
            all: false,
        }
    }

    #[test]
    fn an_old_push_client_shell_quotes_the_remote_directory() {
        let plan = plan(Direction::Push);
        assert_eq!(
            plan.source(RemoteArguments::ShellQuoted),
            "/Users/fredrir/projects/my-app"
        );
        assert_eq!(
            plan.destination(RemoteArguments::ShellQuoted),
            "archie:'/home/fredrir/projects'/"
        );
    }

    #[test]
    fn a_modern_pull_client_leaves_the_remote_path_for_rsync_to_protect() {
        let plan = plan(Direction::Pull);
        assert_eq!(
            plan.source(RemoteArguments::Protected),
            "archie:/home/fredrir/projects/my-app"
        );
        assert_eq!(
            plan.destination(RemoteArguments::Protected),
            "/Users/fredrir/projects/"
        );
    }

    #[test]
    fn both_argument_styles_preserve_a_remote_path_that_needs_quoting() {
        let mut plan = plan(Direction::Pull);
        plan.remote = "/home/fredrir/odd path/it's here".into();
        assert_eq!(
            plan.source(RemoteArguments::Protected),
            "archie:/home/fredrir/odd path/it's here"
        );
        assert_eq!(
            plan.source(RemoteArguments::ShellQuoted),
            "archie:'/home/fredrir/odd path/it'\\''s here'"
        );
    }

    #[test]
    fn the_client_help_selects_the_argument_contract_it_supports() {
        assert_eq!(
            remote_arguments_from_help(b"--old-args  disable modern protection\n", b""),
            RemoteArguments::Protected
        );
        assert_eq!(
            remote_arguments_from_help(b"openrsync options\n", b""),
            RemoteArguments::ShellQuoted
        );
        assert_eq!(
            remote_arguments_from_help(b"", b"usage: rsync --old-args\n"),
            RemoteArguments::Protected
        );
    }

    #[test]
    fn a_modern_client_is_pinned_to_the_detected_contract() {
        let modern = plan(Direction::Pull).arguments(RemoteArguments::Protected);
        let legacy = plan(Direction::Pull).arguments(RemoteArguments::ShellQuoted);
        assert!(modern.contains(&"--no-old-args".to_string()));
        assert!(!legacy.contains(&"--no-old-args".to_string()));
    }

    #[test]
    fn the_default_transfer_skips_what_git_was_told_to_skip() {
        let arguments = plan(Direction::Push).arguments(RemoteArguments::Protected);
        assert!(arguments.contains(&GITIGNORE.to_string()));
        assert!(arguments.contains(&"--exclude=.git/".to_string()));
    }

    #[test]
    fn all_turns_every_filter_off_at_once() {
        let mut plan = plan(Direction::Push);
        plan.all = true;
        let arguments = plan.arguments(RemoteArguments::Protected);
        assert!(!arguments.contains(&GITIGNORE.to_string()));
        assert!(!arguments.contains(&"--exclude=.git/".to_string()));
        assert!(
            !arguments
                .iter()
                .any(|arg| arg.starts_with("--exclude-from="))
        );
    }

    #[test]
    fn a_fast_route_sends_whole_files_and_a_slow_one_does_not() {
        for route in [Route::Cable, Route::Wifi, Route::Lan] {
            let mut plan = plan(Direction::Push);
            plan.route = Some(route);
            assert!(
                plan.arguments(RemoteArguments::Protected)
                    .contains(&"-W".to_string())
            );
        }
        let mut plan = plan(Direction::Push);
        plan.route = Some(Route::Tailscale);
        assert!(
            !plan
                .arguments(RemoteArguments::Protected)
                .contains(&"-W".to_string())
        );
        plan.route = None;
        assert!(
            !plan
                .arguments(RemoteArguments::Protected)
                .contains(&"-W".to_string())
        );
    }

    #[test]
    fn the_paths_are_the_last_two_arguments_and_nothing_reads_them_as_flags() {
        let arguments = plan(Direction::Push).arguments(RemoteArguments::ShellQuoted);
        let end = arguments.len();
        assert_eq!(arguments[end - 3], "--");
        assert_eq!(arguments[end - 2], "/Users/fredrir/projects/my-app");
        assert_eq!(arguments[end - 1], "archie:'/home/fredrir/projects'/");
    }

    #[test]
    fn dry_run_and_checksum_reach_the_command() {
        let mut plan = plan(Direction::Push);
        plan.dry_run = true;
        plan.checksum = true;
        let arguments = plan.arguments(RemoteArguments::Protected);
        assert!(arguments.contains(&"-n".to_string()));
        assert!(arguments.contains(&"-c".to_string()));
    }

    #[test]
    fn a_transferred_file_is_counted_with_its_size() {
        let mut outcome = Outcome::default();
        absorb(&mut outcome, ">f+++++++|projects/my-app/main.rs|1024");
        absorb(&mut outcome, ">f..t....|projects/my-app/lib.rs|2048");
        assert_eq!(outcome.files, 2);
        assert_eq!(outcome.created, 1);
        assert_eq!(outcome.bytes, 3072);
    }

    #[test]
    fn an_unchanged_file_is_not_a_transfer() {
        let mut outcome = Outcome::default();
        absorb(&mut outcome, ".f........|projects/my-app/main.rs|1024");
        assert_eq!(outcome, Outcome::default());
        assert!(outcome.quiet());
    }

    #[test]
    fn a_new_directory_counts_once_and_carries_no_bytes() {
        let mut outcome = Outcome::default();
        absorb(&mut outcome, "cd+++++++|projects/my-app/|128");
        assert_eq!(outcome.files, 0);
        assert_eq!(outcome.created, 1);
        assert_eq!(outcome.bytes, 0);
        assert!(!outcome.quiet());
    }

    #[test]
    fn a_name_containing_a_bar_is_not_cut_in_half() {
        let mut outcome = Outcome::default();
        absorb(&mut outcome, ">f+++++++|odd|name.txt|64");
        assert_eq!(outcome.files, 1);
        assert_eq!(outcome.bytes, 64);
        assert_eq!(outcome.lines, [">f+++++++ odd|name.txt"]);
    }

    #[test]
    fn anything_that_is_not_a_formatted_line_is_ignored() {
        let mut outcome = Outcome::default();
        absorb(&mut outcome, "sent 147 bytes  received 38 bytes");
        absorb(&mut outcome, "");
        absorb(&mut outcome, "created directory /tmp/x");
        assert_eq!(outcome, Outcome::default());
    }

    #[test]
    fn a_failure_is_reported_by_its_cause_rather_than_its_code() {
        let stderr = "rsync: link_stat \"/x\" failed: No such file or directory (2)\n\
                      rsync error: some files could not be transferred (code 23)\n";
        assert!(explain(stderr, Some(23)).contains("No such file or directory"));
        assert_eq!(explain("", Some(23)), "rsync exited with status 23");
        assert_eq!(explain("", None), "rsync was killed");
    }

    #[test]
    fn an_error_with_only_a_summary_line_still_says_something() {
        let stderr = "rsync error: unexplained error (code 255)\n";
        assert!(explain(stderr, Some(255)).contains("unexplained error"));
    }
}

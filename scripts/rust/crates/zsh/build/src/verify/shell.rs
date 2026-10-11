//! One login zsh on a pty, started as a terminal starts it, from a scratch
//! `ZDOTDIR` whose `.zshrc` sources `~/.zshrc` and then the dump hook.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{ErrorKind, Read};
use std::os::fd::AsFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use pty_process::Size;
use pty_process::blocking::{Command, Pty, open};

use super::compare::Dump;
use crate::quote;

const HOOK: &str = include_str!("../../assets/verify.zsh");

const TIMEOUT: Duration = Duration::from_secs(60);

/// Startup files the scratch `ZDOTDIR` links to their copies in `~`.
const LINKED: &[&str] = &[".zshenv", ".zprofile", ".zlogin", ".zlogout", ".zkbd"];

/// What a terminal hands a new login shell, taken from the caller.
const TERMINAL: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "TERM",
    "COLORTERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "LANG",
    "TZ",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "DBUS_SESSION_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "__CF_USER_TEXT_ENCODING",
];

const TERMINAL_PREFIXES: &[&str] = &["LC_", "WEZTERM_"];

/// A request startup acts on with `cd`, and a dump path compinit would rewrite.
const WITHHELD: &[&str] = &["WEZTERM_GO_TO", "ZSH_COMPDUMP", "PATH"];

/// The search path launchd gives a terminal on macOS.
const PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The environment of both shells: the caller's terminal variables and the
/// config's ambient ones, which the build read from the same place.
pub fn environment(ambient: &[String]) -> BTreeMap<OsString, OsString> {
    let mut env: BTreeMap<OsString, OsString> = std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            let wanted = TERMINAL.contains(&&*name)
                || TERMINAL_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                || ambient.iter().any(|ambient| *ambient == name);
            wanted && !WITHHELD.contains(&&*name)
        })
        .collect();
    env.insert("PATH".into(), PATH.into());
    let term = env.get(&OsString::from("TERM"));
    if term.is_none_or(|term| term.is_empty() || term == "dumb") {
        env.insert("TERM".into(), "xterm-256color".into());
    }
    env
}

/// Which startup `~/.zshrc` takes.
pub enum Startup<'a> {
    /// The bundle; its completion directories, loaded before the dump, and
    /// the bundle to source in place of `~/.zshrc`, if any.
    Bundle(&'a [String], Option<&'a Path>),
    /// The sources, through `ZSH_BUILD_SKIP=1`.
    Sources,
}

impl Startup<'_> {
    fn label(&self) -> &'static str {
        match self {
            Startup::Bundle(..) => "bundle",
            Startup::Sources => "sources",
        }
    }
}

/// Starts zsh in `scratch` and returns the state it dumps.
pub fn run(
    zsh: &Path,
    scratch: &Path,
    env: &BTreeMap<OsString, OsString>,
    startup: &Startup,
) -> Result<Dump, String> {
    let label = startup.label();
    let zdotdir = prepare(scratch, env, startup)?;
    let (pty, pts) = open().map_err(|error| format!("{label}: pty: {error}"))?;
    pty.resize(Size::new(50, 200))
        .map_err(|error| format!("{label}: pty: {error}"))?;
    let flags = fcntl(&pty, FcntlArg::F_GETFL).map_err(|error| format!("{label}: pty: {error}"))?;
    fcntl(
        &pty,
        FcntlArg::F_SETFL(OFlag::from_bits_retain(flags) | OFlag::O_NONBLOCK),
    )
    .map_err(|error| format!("{label}: pty: {error}"))?;
    let mut command = Command::new(zsh)
        .arg0("-zsh")
        .env_clear()
        .envs(env)
        .env("ZDOTDIR", &zdotdir)
        .env("_ZO_DATA_DIR", scratch.join("zoxide"))
        .current_dir(&zdotdir);
    if matches!(startup, Startup::Sources) {
        command = command.env("ZSH_BUILD_SKIP", "1");
    }
    let mut child = command
        .spawn(pts)
        .map_err(|error| format!("{label}: {}: {error}", zsh.display()))?;
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        drain(&pty).map_err(|error| format!("{label}: pty: {error}"))?;
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("{label}: {error}"))?
        {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{label}: no dump within {}s", TIMEOUT.as_secs()));
        }
    };
    let dump = std::fs::read(scratch.join("dump")).unwrap_or_default();
    Dump::parse(&dump).map_err(|_| format!("{label}: no dump; zsh {}", exit(status)))
}

/// A fresh `ZDOTDIR` and hook for `startup`; the dump goes to `scratch/dump`.
fn prepare(
    scratch: &Path,
    env: &BTreeMap<OsString, OsString>,
    startup: &Startup,
) -> Result<PathBuf, String> {
    let home = env
        .get(&OsString::from("HOME"))
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let zdotdir = scratch.join("zdotdir");
    let dump = scratch.join("dump");
    for stale in [&dump, &zdotdir] {
        let _ = std::fs::remove_dir_all(stale).or_else(|_| std::fs::remove_file(stale));
    }
    std::fs::create_dir_all(&zdotdir)
        .map_err(|error| format!("create {}: {error}", zdotdir.display()))?;
    for name in LINKED {
        let linked = home.join(name);
        if linked.symlink_metadata().is_ok() {
            std::os::unix::fs::symlink(&linked, zdotdir.join(name))
                .map_err(|error| format!("link {}: {error}", linked.display()))?;
        }
    }
    let word = |path: &Path| quote::word(&path.to_string_lossy());
    let completions = match startup {
        Startup::Bundle(dirs, _) => dirs
            .iter()
            .map(|dir| quote::word(dir))
            .collect::<Vec<_>>()
            .join(" "),
        Startup::Sources => String::new(),
    };
    let hook = scratch.join("hook.zsh");
    let text = HOOK
        .replace("@HISTFILE@", &word(&scratch.join("history")))
        .replace("@DUMP@", &word(&dump))
        .replace("@COMPLETIONS@", &completions);
    write(&hook, &text)?;
    let rc = match startup {
        Startup::Bundle(_, Some(bundle)) => bundle.to_path_buf(),
        _ => home.join(".zshrc"),
    };
    let zshrc = format!(
        "__zbv_load() {{ \\builtin emulate -L zsh -o no_aliases; \\builtin source {hook} }}\n\
         [[ -r {rc} ]] && \\builtin source {rc}\n\
         \\__zbv_load\n",
        hook = word(&hook),
        rc = word(&rc),
    );
    write(&zdotdir.join(".zshrc"), &zshrc)?;
    Ok(zdotdir)
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|error| format!("write {}: {error}", path.display()))
}

/// Reads what the shell printed, so it never blocks on a full terminal.
fn drain(pty: &Pty) -> std::io::Result<()> {
    let mut descriptors = [PollFd::new(pty.as_fd(), PollFlags::POLLIN)];
    let _ = poll(&mut descriptors, PollTimeout::from(50_u16));
    let mut buffer = [0_u8; 8192];
    loop {
        match (&*pty).read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.raw_os_error() == Some(Errno::EIO as i32) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

fn exit(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited {code}"),
        (_, Some(signal)) => format!("killed by signal {signal}"),
        _ => status.to_string(),
    }
}

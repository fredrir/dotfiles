use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::ops::{Deref, DerefMut};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

pub(super) struct AgentChild {
    child: Child,
    terminal: File,
    foreground: i32,
    reaped: bool,
}

impl AgentChild {
    #[allow(unsafe_code)]
    pub fn spawn(command: &mut Command) -> Result<Self, String> {
        let terminal = File::options()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(super::error)?;
        let foreground = unsafe { libc::tcgetpgrp(terminal.as_raw_fd()) };
        if foreground < 0 {
            return Err(super::error(std::io::Error::last_os_error()));
        }
        let child = command.process_group(0).spawn().map_err(super::error)?;
        let mut owned = Self {
            child,
            terminal,
            foreground,
            reaped: false,
        };
        if let Err(error) = foreground_group(&owned.terminal, owned.child.id() as i32) {
            let _ = owned.terminate();
            return Err(error);
        }
        owned.signal(libc::SIGCONT)?;
        Ok(owned)
    }

    #[allow(unsafe_code)]
    pub fn signal(&self, signal: i32) -> Result<(), String> {
        if unsafe { libc::kill(-(self.child.id() as i32), signal) } != 0 {
            return Err(super::error(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn terminate(&mut self) -> Result<(), String> {
        if self.reaped {
            return Ok(());
        }
        if self.child.try_wait().map_err(super::error)?.is_some() {
            self.reaped = true;
            return foreground_group(&self.terminal, self.foreground);
        }
        let _ = self.signal(libc::SIGTERM);
        let _ = self.signal(libc::SIGCONT);
        let until = Instant::now() + Duration::from_secs(15);
        while self.child.try_wait().map_err(super::error)?.is_none() {
            if Instant::now() >= until {
                return Err("owned agent did not stop; destination remains fenced".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.reaped = true;
        foreground_group(&self.terminal, self.foreground)?;
        Ok(())
    }
}

impl Deref for AgentChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}
impl DerefMut for AgentChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}
impl Drop for AgentChild {
    fn drop(&mut self) {
        let _ = self.terminate();
        let _ = foreground_group(&self.terminal, self.foreground);
    }
}

#[allow(unsafe_code)]
fn foreground_group(terminal: &File, pid: i32) -> Result<(), String> {
    // tcsetpgrp from the supervising background group must not stop the supervisor.
    unsafe {
        let previous = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        let result = libc::tcsetpgrp(terminal.as_raw_fd(), pid);
        libc::signal(libc::SIGTTOU, previous);
        if result != 0 {
            return Err(super::error(std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

pub(super) fn descendants(root: u32) -> Result<BTreeSet<(u32, String)>, String> {
    let processes = processes()?;
    let mut parents = BTreeSet::from([root]);
    let mut found = BTreeSet::new();
    loop {
        let before = parents.len();
        for (pid, (parent, identity)) in &processes {
            if parents.contains(parent) && *pid != root {
                parents.insert(*pid);
                found.insert((*pid, identity.clone()));
            }
        }
        if before == parents.len() {
            break;
        }
    }
    Ok(found)
}
fn processes() -> Result<BTreeMap<u32, (u32, String)>, String> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,lstart=,stat="])
        .output()
        .map_err(super::error)?;
    if !output.status.success() {
        return Err("cannot inspect owned agent processes".into());
    }
    let mut result = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut words = line.split_whitespace();
        let (Some(pid), Some(parent)) = (words.next(), words.next()) else {
            continue;
        };
        let (Ok(pid), Ok(parent)) = (pid.parse(), parent.parse()) else {
            continue;
        };
        let identity = words.by_ref().take(5).collect::<Vec<_>>().join(" ");
        if words.next().is_some_and(|state| state.starts_with('Z')) {
            continue;
        }
        result.insert(pid, (parent, identity));
    }
    Ok(result)
}
pub(super) fn identity(pid: u32) -> Result<Option<String>, String> {
    Ok(processes()?.remove(&pid).map(|(_, identity)| identity))
}
pub(super) fn require_gone(owned: &BTreeSet<(u32, String)>) -> Result<(), String> {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let live = processes()?;
        if !owned.iter().any(|(pid, identity)| {
            live.get(pid)
                .is_some_and(|(_, current)| current == identity)
        }) {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(
                "owned background services still running; destination remains fenced".into(),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

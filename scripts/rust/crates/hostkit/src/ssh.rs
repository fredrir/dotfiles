use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output};

use crate::host::Route;

pub const CONNECT_TIMEOUT: &str = "ConnectTimeout=8";
pub const LOG_LEVEL: &str = "LogLevel=ERROR";
pub const OPTIONS: [&str; 2] = [CONNECT_TIMEOUT, LOG_LEVEL];

pub const BATCH_OPTIONS: [&str; 4] = [
    "BatchMode=yes",
    "ConnectionAttempts=1",
    "ServerAliveInterval=5",
    "ServerAliveCountMax=3",
];

pub const HOME_SCRIPT: &str = "printf '%s\\n' \"$HOME\"";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tty {
    #[default]
    Off,
    Forced,
}

impl Tty {
    pub fn flag(self) -> &'static str {
        match self {
            Tty::Off => "-T",
            Tty::Forced => "-tt",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    host: String,
    tty: Tty,
    options: Vec<String>,
    script: Option<String>,
}

impl Session {
    pub fn new(host: &str) -> Session {
        Session {
            host: host.to_string(),
            tty: Tty::Off,
            options: OPTIONS.iter().map(|option| option.to_string()).collect(),
            script: None,
        }
    }

    pub fn interactive(mut self) -> Session {
        self.tty = Tty::Forced;
        self
    }

    pub fn batch(self) -> Session {
        self.options(BATCH_OPTIONS)
    }

    pub fn option(mut self, option: &str) -> Session {
        let key = option_key(option);
        match self
            .options
            .iter_mut()
            .find(|existing| option_key(existing) == key)
        {
            Some(existing) => *existing = option.to_string(),
            None => self.options.push(option.to_string()),
        }
        self
    }

    pub fn options<I, S>(mut self, options: I) -> Session
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for option in options {
            self = self.option(option.as_ref());
        }
        self
    }

    pub fn script(mut self, script: &str) -> Session {
        self.script = Some(script.to_string());
        self
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn args(&self) -> Vec<OsString> {
        let mut args = vec![OsString::from(self.tty.flag())];
        for option in &self.options {
            args.push(OsString::from("-o"));
            args.push(OsString::from(option));
        }
        args.push(OsString::from("--"));
        args.push(OsString::from(&self.host));
        if let Some(script) = &self.script {
            args.push(OsString::from(script));
        }
        args
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new("ssh");
        command.args(self.args());
        command
    }
}

fn option_key(option: &str) -> &str {
    option.split_once('=').map_or(option, |(key, _)| key)
}

pub fn transport() -> String {
    let mut transport = String::from("ssh");
    for option in OPTIONS {
        transport.push_str(" -o ");
        transport.push_str(option);
    }
    transport
}

pub fn remote_home(host: &str) -> Result<PathBuf, String> {
    let output = Session::new(host)
        .script(HOME_SCRIPT)
        .command()
        .output()
        .map_err(command_error)?;
    if !output.status.success() {
        return Err(output_error(host, &output));
    }
    parse_home(host, &output.stdout)
}

pub fn parse_home(host: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| format!("{host} returned a non-UTF-8 home directory"))?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let invalid = || format!("{host} returned an invalid home directory");
    if text.is_empty() || text.contains('\r') || text.contains('\n') {
        return Err(invalid());
    }
    let home = PathBuf::from(text);
    if !home.is_absolute() {
        return Err(invalid());
    }
    Ok(home)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub hostname: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub proxy: Option<String>,
    pub bound: Option<String>,
    pub control_path: Option<String>,
    pub route: Route,
}

// The ordered `Match exec` probes in ~/.ssh/config.d/05-* through 07-* run
// during config resolution, so asking ssh to resolve a host is the same
// decision the next connection to it will make, without opening one.
pub fn resolve(host: &str) -> Result<Resolved, String> {
    let output = Command::new("ssh")
        .args(["-G", "--"])
        .arg(host)
        .output()
        .map_err(command_error)?;
    if !output.status.success() {
        return Err(stderr_reason(&output.stderr, "no config resolved"));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "ssh -G returned non-UTF-8 output".to_string())?;
    parse(&text)
}

pub fn parse(config: &str) -> Result<Resolved, String> {
    let mut hostname = String::new();
    let mut port = None;
    let mut user = None;
    let mut proxy = None;
    let mut interface = None;
    let mut address = None;
    let mut control_path = None;
    for line in config.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let value = value.trim();
        match key {
            "hostname" => hostname = value.to_string(),
            "port" => port = value.parse().ok(),
            "user" => user = Some(value.to_string()),
            "proxycommand" if value != "none" => proxy = Some(value.to_string()),
            "bindinterface" if value != "none" => interface = Some(value.to_string()),
            "bindaddress" if value != "none" => address = Some(value.to_string()),
            "controlpath" if value != "none" => control_path = Some(value.to_string()),
            _ => {}
        }
    }
    if hostname.is_empty() {
        return Err("ssh -G returned no hostname".into());
    }
    let route = route_of(&hostname, proxy.as_deref().unwrap_or(""));
    Ok(Resolved {
        route,
        bound: interface.or(address),
        hostname,
        port,
        user,
        proxy,
        control_path,
    })
}

fn route_of(hostname: &str, proxy: &str) -> Route {
    if hostname.starts_with("10.77.77.") {
        Route::Cable
    } else if hostname.starts_with("10.77.78.") {
        Route::Wifi
    } else if proxy.contains("home-lan-connect") {
        Route::Lan
    } else {
        Route::Tailscale
    }
}

pub fn resolved(host: &str) -> Option<Route> {
    resolve(host).ok().map(|resolved| resolved.route)
}

pub fn classify(config: &str) -> Option<Route> {
    parse(config).ok().map(|resolved| resolved.route)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlMaster {
    pub alive: bool,
    pub detail: Option<String>,
}

pub fn control_master(host: &str) -> ControlMaster {
    match Command::new("ssh")
        .args(["-O", "check", "--"])
        .arg(host)
        .output()
    {
        Ok(output) => {
            let detail = if output.stderr.is_empty() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            };
            ControlMaster {
                alive: output.status.success(),
                detail: (!detail.is_empty()).then_some(detail),
            }
        }
        Err(error) => ControlMaster {
            alive: false,
            detail: Some(format!("ssh: {error}")),
        },
    }
}

pub fn control_master_alive(host: &str) -> bool {
    control_master(host).alive
}

pub fn command_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        "ssh is required".to_string()
    } else {
        format!("ssh: {error}")
    }
}

pub fn output_error(host: &str, output: &Output) -> String {
    let reason = String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string);
    match (reason, output.status.code()) {
        (Some(reason), _) => format!("{host}: {reason}"),
        (None, Some(code)) => format!("{host}: ssh exited with status {code}"),
        (None, None) => format!("{host}: ssh was interrupted"),
    }
}

pub fn stderr_reason(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

#[cfg(test)]
#[path = "../tests/unit/ssh_tests.rs"]
mod tests;

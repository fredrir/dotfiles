use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::{
    Result,
    config::{Paths, clean},
    process::{self, Output},
};

#[derive(Clone)]
pub struct Tmux {
    pub binary: PathBuf,
    pub socket: Option<String>,
}

impl Tmux {
    pub fn new(socket: Option<String>) -> Self {
        Self {
            binary: std::env::var_os("TMUX_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|| "tmux".into()),
            socket: socket
                .or_else(|| std::env::var("TMUX_WORKSPACE_SOCKET").ok())
                .or_else(|| {
                    std::env::var("TMUX")
                        .ok()
                        .and_then(|v| v.rsplitn(3, ',').nth(2).map(str::to_owned))
                })
                .or_else(|| {
                    Some(format!(
                        "{}/tmux-{}/default",
                        std::env::var("TMUX_TMPDIR").unwrap_or_else(|_| "/tmp".into()),
                        nix::unistd::getuid()
                    ))
                }),
        }
    }
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        if let Some(socket) = &self.socket {
            command.args(["-S", socket]);
        }
        command
    }
    pub fn try_run(&self, args: &[&str]) -> Result<Output> {
        process::capture(
            self.command().args(args),
            None,
            Some(Duration::from_secs(10)),
        )
    }
    pub fn run(&self, args: &[&str]) -> Result<String> {
        Ok(self
            .try_run(args)?
            .checked()?
            .out
            .trim_end_matches('\n')
            .to_owned())
    }
    pub fn input(&self, args: &[&str], input: &[u8]) -> Result<Output> {
        process::capture(
            self.command().args(args),
            Some(input),
            Some(Duration::from_secs(10)),
        )?
        .checked()
    }
    pub fn format(
        &self,
        expression: &str,
        pane: Option<&str>,
        client: Option<&str>,
    ) -> Result<String> {
        let mut args = vec!["display-message", "-p"];
        if let Some(pane) = pane.filter(|p| !p.is_empty()) {
            args.extend(["-t", pane]);
        }
        if let Some(client) = client.filter(|c| !c.is_empty()) {
            args.extend(["-c", client]);
        }
        args.push(expression);
        self.run(&args)
    }
    pub fn option(&self, name: &str) -> String {
        self.run(&["show-options", "-gqv", name])
            .unwrap_or_default()
    }
    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        self.run(&["set-option", "-g", name, value]).map(drop)
    }
    pub fn socket(&self) -> Result<String> {
        self.socket
            .clone()
            .map(Ok)
            .unwrap_or_else(|| self.format("#{socket_path}", None, None))
    }
    pub fn require_version(&self) -> Result<()> {
        let value = self
            .format("#{version}", None, None)
            .or_else(|_| self.run(&["-V"]))?;
        let expression = regex::Regex::new(r"(\d+)\.(\d+)([a-z]?)")?;
        let c = expression
            .captures(&value)
            .ok_or("tmux version unavailable")?;
        let version = (c[1].parse::<u32>()?, c[2].parse::<u32>()?);
        if version < (3, 7) || (version == (3, 7) && &c[3] < "c") {
            return Err(format!("tmux 3.7c or newer required; found {}", value.trim()).into());
        }
        Ok(())
    }
    pub fn clients(&self) -> Result<Vec<Client>> {
        let output = self.try_run(&["list-clients", "-F", "#{client_name}\t#{client_pid}\t#{client_created}\t#{client_tty}\t#{client_termname}\t#{@workspace-internal}\t#{client_session}"])?;
        Ok(output
            .out
            .lines()
            .filter_map(|line| {
                let p: Vec<_> = line.split('\t').collect();
                (p.len() == 7).then(|| Client {
                    name: p[0].into(),
                    pid: p[1].into(),
                    created: p[2].into(),
                    tty: p[3].into(),
                    term: p[4].into(),
                    internal: !p[5].is_empty(),
                    session: p[6].into(),
                })
            })
            .collect())
    }
    pub fn panes(&self) -> Result<Vec<Pane>> {
        Ok(self.run(&["list-panes", "-a", "-F", "#{pane_id}\t#{session_id}\t#{session_name}\t#{window_id}\t#{pane_current_command}\t#{pane_current_path}\t#{@workspace-tool}\t#{@workspace-project}\t#{pane_floating_flag}"])?
            .lines().filter_map(|line| {
                let p: Vec<_> = line.split('\t').collect();
                (p.len() == 9).then(|| Pane { id: p[0].into(), session: p[1].into(), session_name: p[2].into(), window: p[3].into(), command: p[4].into(), path: p[5].into(), tool: p[6].into(), project: p[7].into(), floating: p[8] == "1" })
            }).collect())
    }
    pub fn subprocess_env(&self, command: &mut Command) -> Result<()> {
        let socket = self.socket()?;
        let pid = self.format("#{pid}", None, None)?;
        command
            .env("TMUX", format!("{socket},{pid},0"))
            .env("TMUX_WORKSPACE_SOCKET", &socket);
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Client {
    pub name: String,
    pub pid: String,
    pub created: String,
    pub tty: String,
    pub term: String,
    pub internal: bool,
    pub session: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Pane {
    pub id: String,
    pub session: String,
    pub session_name: String,
    pub window: String,
    pub command: String,
    pub path: String,
    pub tool: String,
    pub project: String,
    pub floating: bool,
}

#[derive(Clone)]
pub struct Context {
    pub tmux: Tmux,
    pub paths: Paths,
    pub pane: Option<String>,
    pub client: Option<String>,
}

impl Context {
    pub fn resolve(&mut self) -> Result<()> {
        if self.pane.is_none() {
            self.pane = Some(self.tmux.format("#{pane_id}", None, None)?);
        }
        let pane = self.pane()?;
        if !pane.starts_with('%')
            || pane.len() < 2
            || !pane[1..].bytes().all(|b| b.is_ascii_digit())
        {
            return Err("tmux pane required".into());
        }
        if self.client.is_none() {
            let session = self.fmt("#{session_name}")?;
            let internal = !self.fmt("#{@workspace-internal}")?.is_empty();
            let clients: Vec<_> = self
                .tmux
                .clients()?
                .into_iter()
                .filter(|c| (internal || !c.internal) && c.session == session)
                .collect();
            if clients.len() == 1 {
                self.client = Some(clients[0].name.clone());
            }
        }
        Ok(())
    }
    pub fn pane(&self) -> Result<&str> {
        self.pane
            .as_deref()
            .ok_or_else(|| "tmux pane required".into())
    }
    pub fn client(&self) -> Result<&str> {
        self.client.as_deref().ok_or_else(|| {
            "attached client required; pass --client when multiple clients are attached".into()
        })
    }
    pub fn fmt(&self, expression: &str) -> Result<String> {
        self.tmux
            .format(expression, self.pane.as_deref(), self.client.as_deref())
    }
    pub fn cwd(&self) -> PathBuf {
        self.pane
            .as_ref()
            .and_then(|_| self.fmt("#{pane_current_path}").ok())
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
    }
    pub fn root(&self) -> PathBuf {
        let cwd = self.cwd();
        process::run(
            Command::new("git")
                .arg("-C")
                .arg(&cwd)
                .args(["rev-parse", "--show-toplevel"]),
        )
        .ok()
        .map(|v| PathBuf::from(v.out.trim()))
        .filter(|p| p.is_dir())
        .unwrap_or(cwd)
    }
    pub fn notice(&self, message: &str) {
        if self.pane.is_some() || self.client.is_some() {
            let mut args = vec!["display-message", "-d", "4500"];
            if let Some(client) = &self.client {
                args.extend(["-c", client]);
            }
            let text = clean(message, 600).replace('#', "##");
            args.extend(["--", &text]);
            let _ = self.tmux.run(&args);
        } else {
            println!("{message}");
        }
    }
    pub fn self_command(&self, command: &str, extra: &[&str]) -> Result<Vec<String>> {
        let mut args = vec![
            std::env::current_exe()?.to_string_lossy().into_owned(),
            command.into(),
            "--config".into(),
            self.paths.config.to_string_lossy().into_owned(),
        ];
        for (flag, value) in [
            ("--pane", &self.pane),
            ("--client", &self.client),
            ("--socket", &self.tmux.socket),
        ] {
            if let Some(value) = value {
                args.extend([flag.into(), value.clone()]);
            }
        }
        args.extend(extra.iter().map(|v| (*v).to_owned()));
        Ok(args)
    }
    pub fn popup(&self, argv: &[String], title: &str, cwd: &Path, close: bool) -> Result<()> {
        let client = self.client()?;
        let command = process::shell(argv);
        let title = format!(" {} ", clean(title, 160));
        let mut args = vec![
            "display-popup",
            "-c",
            client,
            "-d",
            cwd.to_str().ok_or("invalid directory")?,
            "-w",
            "88%",
            "-h",
            "82%",
            "-T",
            &title,
        ];
        if close {
            args.push("-E");
        }
        args.push(&command);
        let result = process::capture(self.tmux.command().args(&args), None, None)?;
        if ![0, 129, 130].contains(&result.code) {
            result.checked()?;
        }
        Ok(())
    }
}

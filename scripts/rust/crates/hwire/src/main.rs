#![forbid(unsafe_code)]

mod client;
mod info;
mod proto;
mod report;
mod serve;

use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::Duration;

use clap::{Parser, Subcommand};
use hostkit::host;
use hostkit::{Host, Route};
use serde_json::json;
use workstation::{Completions, Style};

use client::{Direction, Peer};
use info::ColorMode;
use report::Run;

const PROGRAM: &str = "hwire";

const LATENCY_BUDGET: Duration = Duration::from_millis(500);

const REMOTE_PATH: &str = r#"PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH" "#;

#[derive(Parser)]
#[command(
    version,
    about = "Measure and inspect connections between macie and archie",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Half>,

    #[arg(
        short = 'r',
        long = "route",
        value_name = "ROUTE",
        help = "Select the cable, Wi-Fi, LAN, or Tailscale route to measure"
    )]
    route: Option<Route>,

    #[arg(
        short = 'a',
        long = "all",
        conflicts_with_all = ["route", "both"],
        help = "Measure every available route sequentially"
    )]
    all: bool,

    #[arg(
        short = 'b',
        long = "both",
        conflicts_with = "route",
        help = "Compatibility spelling for --all"
    )]
    both: bool,

    #[arg(
        short = 't',
        long = "time",
        value_name = "SECONDS",
        default_value_t = 1.0,
        help = "Set the transfer duration for each direction"
    )]
    time: f64,

    #[arg(
        short = 'P',
        long = "streams",
        value_name = "N",
        default_value_t = 1,
        help = "Set the number of concurrent transfer connections"
    )]
    streams: usize,

    #[arg(
        short = 'n',
        long = "samples",
        value_name = "N",
        default_value_t = 200,
        help = "Limit the number of round trips timed"
    )]
    samples: usize,

    #[arg(
        short = 'l',
        long = "latency",
        conflicts_with_all = ["up", "down"],
        help = "Measure round-trip latency without running transfers"
    )]
    latency: bool,

    #[arg(
        short = 'u',
        long = "up",
        conflicts_with = "down",
        help = "Transfer only from this machine to the peer"
    )]
    up: bool,

    #[arg(
        short = 'd',
        long = "down",
        help = "Transfer only from the peer to this machine"
    )]
    down: bool,

    #[arg(
        long = "at",
        value_name = "ADDRESS:PORT",
        conflicts_with_all = ["route", "all", "both"],
        help = "Measure an already-running server without starting one over SSH"
    )]
    at: Option<SocketAddrV4>,

    #[arg(
        long = "token",
        value_name = "HEX",
        requires = "at",
        help = "Use or require the server's authentication token"
    )]
    token: Option<String>,

    #[arg(
        long = "json",
        help = "Print measurement or connection information as JSON"
    )]
    json: bool,

    #[arg(
        short = 'i',
        long = "info",
        conflicts_with_all = [
            "route", "all", "both", "time", "streams", "samples", "latency", "up",
            "down", "at", "token"
        ],
        help = "Inspect the current connection or routes to HOST"
    )]
    info: bool,

    #[arg(
        short = 'v',
        long = "verbose",
        requires = "info",
        help = "Show full route, session, and SSH diagnostics"
    )]
    verbose: bool,

    #[arg(
        long = "watch",
        requires = "info",
        help = "Watch connection information and report meaningful changes"
    )]
    watch: bool,

    #[arg(
        long = "interval",
        value_name = "SECONDS",
        default_value_t = 1.0,
        requires = "watch",
        help = "Set the watch refresh interval"
    )]
    interval: f64,

    #[arg(
        long = "notify",
        requires = "watch",
        help = "Ring the terminal bell when the preferred route changes"
    )]
    notify: bool,

    #[arg(
        long = "color",
        value_name = "WHEN",
        default_value = "auto",
        help = "Control colored output"
    )]
    color: ColorMode,

    #[arg(
        value_name = "HOST",
        requires = "info",
        value_hint = clap::ValueHint::Hostname,
        help = "Resolve one or more explicit SSH targets"
    )]
    hosts: Vec<String>,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Subcommand)]
enum Half {
    /// Answer measurements until told to stop.
    Serve {
        #[arg(
            long = "bind",
            value_name = "ADDRESS",
            default_value = "0.0.0.0",
            help = "Set the address on which hwire serve listens"
        )]
        bind: Ipv4Addr,

        #[arg(
            long = "port",
            value_name = "PORT",
            default_value_t = 0,
            help = "Set the server port, with zero selecting an available port"
        )]
        port: u16,

        #[arg(
            long = "token",
            value_name = "HEX",
            help = "Require the server's authentication token"
        )]
        token: Option<String>,

        #[arg(
            long = "idle",
            value_name = "SECONDS",
            default_value_t = 15,
            help = "Set how long an idle server waits before exiting"
        )]
        idle: u64,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        if cli.completions.is_zsh() {
            print!("{}", include_str!("info/completion.zsh"));
        }
        return status;
    }
    let outcome = match &cli.command {
        Some(half) => serve(half),
        None if cli.info => information(&cli),
        None => measure(&cli),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

fn information(cli: &Cli) -> Result<(), String> {
    if !cli.interval.is_finite() || cli.interval <= 0.0 {
        return Err("--interval needs to be a positive number of seconds".into());
    }
    info::run(info::Options {
        verbose: cli.verbose,
        json: cli.json,
        watch: cli.watch,
        interval: Duration::from_secs_f64(cli.interval),
        notify: cli.notify,
        color: cli.color,
        targets: cli.hosts.clone(),
        diagnostics: cli.verbose || cli.json,
    })
}

fn serve(half: &Half) -> Result<(), String> {
    let Half::Serve {
        bind,
        port,
        token,
        idle,
    } = half;
    let token = token.as_deref().map(proto::unhex).transpose()?;
    serve::serve(
        SocketAddrV4::new(*bind, *port),
        token,
        (*idle > 0).then(|| Duration::from_secs(*idle)),
    )
    .map_err(|error| format!("serve: {error}"))
}

fn measure(cli: &Cli) -> Result<(), String> {
    if cli.streams == 0 {
        return Err("--streams needs at least one connection".into());
    }
    if !cli.time.is_finite() || cli.time <= 0.0 {
        return Err("--time needs to be a positive number of seconds".into());
    }
    let window = Duration::from_secs_f64(cli.time);
    let directions: Vec<Direction> = match (cli.latency, cli.up, cli.down) {
        (true, _, _) => Vec::new(),
        (_, true, _) => vec![Direction::Up],
        (_, _, true) => vec![Direction::Down],
        _ => vec![Direction::Up, Direction::Down],
    };

    let style = Style::for_stdout_with_color(info::measurement_color(cli.color));
    let this = Host::this()?;
    let mut runs = Vec::new();

    if let Some(address) = cli.at {
        let peer = Peer {
            address,
            local: None,
            token: match &cli.token {
                Some(text) => proto::unhex(text)?,
                None => [0u8; 16],
            },
        };
        let target = address.to_string();
        let measured = run(cli, &peer, None, None, &target, window, &directions)
            .map_err(|error| format!("{target}: {error}"))?;
        peer.bye();
        runs.push(measured);
        return present(cli, &style, this.name(), &target, &runs);
    }

    for route in routes(cli, this)? {
        let token = proto::token().map_err(|error| format!("/dev/urandom: {error}"))?;
        let remote = start(this.peer(), route, &token, window)?;
        let local_address = this.address(route)?;
        let peer_address = this.peer().address(route)?;
        let peer = Peer {
            address: remote.address,
            local: Some(local_address),
            token,
        };
        let measured = run(
            cli,
            &peer,
            Some(route),
            Some(local_address.to_string()),
            &peer_address.to_string(),
            window,
            &directions,
        );
        peer.bye();
        runs.push(remote.finish(measured).map_err(|error| {
            format!("{} over the {}: {error}", this.peer().name(), route.name())
        })?);
    }
    present(cli, &style, this.name(), this.peer().name(), &runs)
}

fn routes(cli: &Cli, this: Host) -> Result<Vec<Route>, String> {
    if cli.all || cli.both {
        let up: Vec<Route> = hostkit::snapshot::probe(this, this.peer(), 22)
            .available()
            .map(|probe| probe.route)
            .collect();
        return match up.is_empty() {
            true => Err(unreachable(this)),
            false => Ok(up),
        };
    }
    if let Some(route) = cli.route {
        return match route.up(this) {
            true => Ok(vec![route]),
            false => Err(format!(
                "{} does not answer over the {}",
                this.peer().name(),
                route.name()
            )),
        };
    }
    host::best(this)
        .map(|route| vec![route])
        .ok_or_else(|| unreachable(this))
}

fn unreachable(this: Host) -> String {
    format!(
        "{} is not reachable over cable, direct Wi-Fi, regular LAN, or Tailscale",
        this.peer().name()
    )
}

fn run(
    cli: &Cli,
    peer: &Peer,
    route: Option<Route>,
    from: Option<String>,
    to: &str,
    window: Duration,
    directions: &[Direction],
) -> Result<Run, String> {
    let latency = peer
        .latency(cli.samples, LATENCY_BUDGET)
        .map_err(|error| format!("timing round trips: {error}"))?;
    let mut transfers = Vec::new();
    for &direction in directions {
        let counted = peer
            .transfer(direction, window, cli.streams)
            .map_err(|error| format!("transferring {}: {error}", direction.name()))?;
        transfers.push((direction, counted));
    }
    Ok(Run {
        route,
        from,
        to: to.to_string(),
        latency,
        transfers,
        streams: cli.streams,
    })
}

fn present(cli: &Cli, style: &Style, this: &str, peer: &str, runs: &[Run]) -> Result<(), String> {
    if cli.json {
        let document = json!({
            "host": this,
            "peer": peer,
            "runs": runs.iter().map(Run::json).collect::<Vec<_>>(),
        });
        println!("{document}");
        return Ok(());
    }
    let blocks: Vec<String> = runs
        .iter()
        .map(|run| run.render(style, this, peer))
        .collect();
    println!("{}", blocks.join("\n\n"));
    Ok(())
}

struct Remote {
    child: Child,
    address: SocketAddrV4,
}

fn start(peer: Host, route: Route, token: &[u8; 16], window: Duration) -> Result<Remote, String> {
    let idle = 10 + 2 * window.as_secs();
    let bind = peer.address(route)?;
    let command = format!(
        "{REMOTE_PATH}hwire serve --bind {} --token {} --idle {idle}",
        bind,
        proto::hex(token),
    );
    let mut child = Command::new("ssh")
        .args(["-T", "-o", "ConnectTimeout=5", "-o", "LogLevel=ERROR"])
        .arg(peer.name())
        .arg(&command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("ssh: {error}"))?;

    let mut banner = String::new();
    let read = child
        .stdout
        .take()
        .map(|stdout| BufReader::new(stdout).read_line(&mut banner))
        .transpose()
        .map_err(|error| format!("ssh: {error}"))?
        .unwrap_or(0);
    match (read > 0).then(|| address_of(&banner)).flatten() {
        Some(address) => Ok(Remote { child, address }),
        None => Err(explain(&mut child, peer)),
    }
}

fn address_of(banner: &str) -> Option<SocketAddrV4> {
    let rest = banner.trim().strip_prefix(serve::BANNER)?;
    let (address, port) = rest.trim().split_once(' ')?;
    Some(SocketAddrV4::new(
        address.parse().ok()?,
        port.trim().parse().ok()?,
    ))
}

fn explain(child: &mut Child, peer: Host) -> String {
    let _ = child.kill();
    let _ = child.wait();
    let stderr = child
        .stderr
        .take()
        .map(|mut stderr| {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
            text
        })
        .unwrap_or_default();
    let reason = stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no answer from the peer")
        .trim()
        .to_string();
    let name = peer.name();
    if reason.contains("not found") {
        return format!("{name}: {reason} — run `dotfile sync` on {name} to install it");
    }
    format!("{name}: {reason}")
}

impl Remote {
    fn finish(mut self, measured: Result<Run, String>) -> Result<Run, String> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        measured.map_err(|error| {
            let stderr = self
                .child
                .stderr
                .take()
                .map(|mut stderr| {
                    let mut text = String::new();
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
                    text
                })
                .unwrap_or_default();
            match stderr.lines().find(|line| line.contains("hwire:")) {
                Some(line) => format!("{error} ({})", line.trim()),
                None => error,
            }
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/main_tests.rs"]
mod tests;

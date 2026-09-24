use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::RecvTimeoutError;
use hostkit::{Host, Route};
use signal_hook::consts::{SIGINT, SIGTERM};

use crate::config::Config;
use crate::forward::{self, ALIAS, Bind, Forward};
use crate::listener::{self, Listener, Service};
use crate::master::Master;
use crate::plan::{self, Action, Kind};
use crate::state::{self, Entry, Paths, State, Status};
use crate::stream::{Event, Stream};

const TICK: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_secs(10);
const ROUTE_CHECK: Duration = Duration::from_secs(10);
const MIN_DELAY: Duration = Duration::from_secs(2);
const MAX_DELAY: Duration = Duration::from_secs(30);

pub fn run(config: &Config) -> Result<(), String> {
    let this = Host::this()?;
    let paths = Paths::resolve()?;
    paths.prepare()?;
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&stop))
            .map_err(|error| format!("signal {signal}: {error}"))?;
    }
    let mut delay = MIN_DELAY;
    while !stop.load(Ordering::Relaxed) {
        let mut state = State::idle(this.peer());
        match session(this, config, &paths, &mut state, &stop) {
            Ok(()) => delay = MIN_DELAY,
            Err(error) => {
                eprintln!("hport: {error}");
                if state.connected {
                    delay = MIN_DELAY;
                }
                state = State {
                    error: Some(error),
                    ..State::idle(this.peer())
                };
                let _ = state::write(&paths, &state);
                pause(delay, &stop);
                delay = (delay * 2).min(MAX_DELAY);
            }
        }
    }
    let _ = std::fs::remove_file(&paths.state);
    Ok(())
}

fn pause(delay: Duration, stop: &AtomicBool) {
    let until = Instant::now() + delay;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(100));
    }
}

fn session(
    this: Host,
    config: &Config,
    paths: &Paths,
    state: &mut State,
    stop: &AtomicBool,
) -> Result<(), String> {
    let peer = this.peer();
    state::write(paths, state)?;
    let mut master = Master::connect(peer, &paths.socket)?;
    state.connected = true;
    state.route = master.route.map(|route| route.name().to_string());
    eprintln!(
        "hport: connected to {} over {}",
        peer.name(),
        route_name(master.route)
    );
    let (sender, events) = crossbeam_channel::unbounded();
    let _stream = Stream::spawn(&master, sender)?;
    let mut forwards = Forwards::default();
    let mut services = Vec::new();
    let mut route_check = Instant::now() + ROUTE_CHECK;
    let mut written: Option<State> = None;
    while !stop.load(Ordering::Relaxed) {
        match events.recv_timeout(TICK) {
            Ok(Event::Snapshot(listeners)) => services = admitted(config, &listeners),
            Ok(Event::Ended(reason)) => return Err(format!("{}: {reason}", peer.name())),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(format!("{}: listener stream closed", peer.name()));
            }
        }
        if !master.alive() {
            return Err(format!("{}: connection lost", peer.name()));
        }
        let local = listener::scan().unwrap_or_default();
        let alias_ready = forward::alias_ready();
        forwards.reconcile(&master, &services, &local, alias_ready);
        state.services = forwards.entries(&services, &local, master.pid());
        state.error = (!alias_ready).then(|| format!("{ALIAS} is missing; run hport setup"));
        if written.as_ref() != Some(state) {
            state::write(paths, state)?;
            written = Some(state.clone());
        }
        if Instant::now() >= route_check {
            route_check = Instant::now() + ROUTE_CHECK;
            if master.route != Some(Route::Cable) {
                let best = hostkit::best(this);
                if better(best, master.route) {
                    eprintln!("hport: {} is up; reconnecting", route_name(best));
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

fn admitted(config: &Config, listeners: &[Listener]) -> Vec<Service> {
    listener::services(listeners)
        .into_iter()
        .filter(|service| config.admits(service))
        .collect()
}

pub fn better(best: Option<Route>, current: Option<Route>) -> bool {
    let rank = |route: Route| Route::every().iter().position(|every| *every == route);
    match (best, current) {
        (Some(best), Some(current)) => rank(best) < rank(current),
        _ => false,
    }
}

fn route_name(route: Option<Route>) -> &'static str {
    route.map_or("an unknown route", Route::name)
}

#[derive(Default)]
struct Forwards {
    active: BTreeSet<Forward>,
    failures: BTreeMap<(Kind, u16), (Instant, String)>,
}

impl Forwards {
    fn reconcile(
        &mut self,
        master: &Master,
        services: &[Service],
        local: &[Listener],
        alias_ready: bool,
    ) {
        let now = Instant::now();
        let alias_free =
            |port| alias_ready && holder(local, port, master.pid(), wildcard).is_none();
        let actions = plan::plan(
            services,
            &self.active,
            |kind, port| {
                self.may_try(kind, port, now) && (kind == Kind::Mirror || alias_free(port))
            },
            forward::mirror_free,
        );
        for action in actions {
            self.apply(master, action);
        }
        self.failures
            .retain(|(_, port), _| services.iter().any(|service| service.port == *port));
    }

    fn may_try(&self, kind: Kind, port: u16, now: Instant) -> bool {
        self.failures
            .get(&(kind, port))
            .is_none_or(|(at, _)| now.duration_since(*at) >= RETRY)
    }

    fn apply(&mut self, master: &Master, action: Action) {
        match action {
            Action::Cancel(forward) => {
                if let Err(reason) = master.cancel(&forward) {
                    eprintln!("hport: - {}: {reason}", forward.spec());
                }
                self.active.remove(&forward);
            }
            Action::Alias(forward) => self.add(master, Kind::Alias, &[forward]),
            Action::Mirror(pair) => self.add(master, Kind::Mirror, &pair),
        }
    }

    fn add(&mut self, master: &Master, kind: Kind, forwards: &[Forward]) {
        let Some(port) = forwards.first().map(|forward| forward.port) else {
            return;
        };
        let mut added = Vec::new();
        for forward in forwards {
            if let Err(reason) = master.forward(forward) {
                for undo in &added {
                    let _ = master.cancel(undo);
                }
                let known = self.failures.contains_key(&(kind, port));
                if !known {
                    eprintln!("hport: + {}: {reason}", forward.spec());
                }
                self.failures.insert((kind, port), (Instant::now(), reason));
                return;
            }
            added.push(forward.clone());
        }
        self.failures.remove(&(kind, port));
        self.active.extend(added);
    }

    fn entries(&self, services: &[Service], local: &[Listener], master: u32) -> Vec<Entry> {
        services
            .iter()
            .map(|service| {
                let held = |bind| {
                    self.active.contains(&Forward {
                        bind,
                        port: service.port,
                        target: service.target,
                    })
                };
                let failed = |kind| {
                    self.failures
                        .get(&(kind, service.port))
                        .map(|(_, reason)| reason.clone())
                };
                let holder = |covers| holder(local, service.port, master, covers);
                let alias = if held(Bind::Alias) {
                    Status::Active
                } else if let Some(process) = holder(wildcard) {
                    Status::Busy(Some(process))
                } else if let Some(reason) = failed(Kind::Alias) {
                    Status::Failed(reason)
                } else {
                    Status::Pending
                };
                let mirror = if Bind::MIRROR.into_iter().all(held) {
                    Status::Active
                } else if let Some(process) = holder(Listener::reachable_through_loopback) {
                    Status::Busy(Some(process))
                } else if !forward::mirror_free(service.port) {
                    Status::Busy(None)
                } else if let Some(reason) = failed(Kind::Mirror) {
                    Status::Failed(reason)
                } else {
                    Status::Pending
                };
                Entry {
                    port: service.port,
                    process: service.process.clone(),
                    alias,
                    mirror,
                }
            })
            .collect()
    }
}

// Linux refuses a specific bind under a listening wildcard on the same port
fn wildcard(listener: &Listener) -> bool {
    listener.address.is_unspecified()
}

fn holder(
    local: &[Listener],
    port: u16,
    master: u32,
    covers: fn(&Listener) -> bool,
) -> Option<String> {
    local
        .iter()
        .find(|listener| listener.port == port && listener.pid != master && covers(listener))
        .map(|listener| listener.process.clone())
}

#[cfg(test)]
#[path = "../tests/unit/daemon_tests.rs"]
mod tests;

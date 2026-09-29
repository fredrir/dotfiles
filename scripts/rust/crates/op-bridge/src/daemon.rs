use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use hostkit::Host;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use crate::broker::{Broker, GRANT, Origin, Policy};
use crate::protocol::{self, Request, Response};
use crate::{onepassword, paths, presence, touchid, tunnel};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run(silent: Vec<String>, prompt: Vec<String>) -> Result<(), String> {
    let this = Host::this()?;
    if this != Host::Macie {
        return Err("the daemon runs on macie; archie reaches it through op".to_string());
    }
    let peer = this.peer();
    let policy = Policy::new(silent, prompt)?;
    let socket = paths::broker()?;
    let known_path = paths::known()?;
    let listener = bind(&socket)?;
    eprintln!(
        "op-bridge: listening on {}; {}",
        socket.display(),
        policy.describe()
    );

    let tunnel = Arc::new(AtomicU32::new(0));
    stop_on_signal(Arc::clone(&tunnel), socket.clone())?;
    {
        let tunnel = Arc::clone(&tunnel);
        thread::spawn(move || tunnel::supervise(peer, &socket, &tunnel));
    }

    let broker = Arc::new(Mutex::new(Broker::new(policy, GRANT, load_known(&known_path))));
    {
        let broker = Arc::clone(&broker);
        thread::spawn(move || watch(&broker));
    }

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("op-bridge: accept: {error}");
                continue;
            }
        };
        let (broker, tunnel, known_path) =
            (Arc::clone(&broker), Arc::clone(&tunnel), known_path.clone());
        thread::spawn(move || {
            if let Err(error) = serve(&stream, &broker, &tunnel, &known_path) {
                eprintln!("op-bridge: {error}");
            }
        });
    }
    Ok(())
}

fn bind(socket: &Path) -> Result<UnixListener, String> {
    let display = |error: std::io::Error| format!("{}: {error}", socket.display());
    let directory = socket.parent().ok_or("broker socket has no parent")?;
    fs::create_dir_all(directory).map_err(display)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(display)?;
    match fs::remove_file(socket) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(display(error)),
        _ => {}
    }
    let listener = UnixListener::bind(socket).map_err(display)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).map_err(display)?;
    Ok(listener)
}

// An orphaned ssh would keep the peer's socket pointing at a dead broker
fn stop_on_signal(tunnel: Arc<AtomicU32>, socket: PathBuf) -> Result<(), String> {
    let mut signals =
        Signals::new([SIGHUP, SIGINT, SIGTERM]).map_err(|error| format!("signals: {error}"))?;
    thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            if let Ok(pid) = i32::try_from(tunnel.load(Ordering::SeqCst))
                && pid != 0
            {
                let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
            }
            let _ = fs::remove_file(&socket);
            std::process::exit(128 + signal);
        }
    });
    Ok(())
}

// Refill waits for an unlocked screen, so 1Password's prompt meets someone at macie
fn watch(broker: &Mutex<Broker>) {
    let mut pending = true;
    loop {
        if pending && !presence::locked() {
            refill(broker);
            pending = false;
        }
        let (wall, monotonic) = (SystemTime::now(), Instant::now());
        thread::sleep(presence::TICK);
        let wall = wall.elapsed().unwrap_or_default();
        if presence::slept(wall, monotonic.elapsed()) {
            if let Ok(mut broker) = broker.lock() {
                broker.forget();
            }
            eprintln!("op-bridge: macie slept; cache cleared");
            pending = true;
        }
    }
}

fn refill(broker: &Mutex<Broker>) {
    let Ok(missing) = broker.lock().map(|broker| broker.missing()) else {
        return;
    };
    let Some(vault) = missing.first().and_then(|reference| protocol::vault(reference)) else {
        return;
    };
    if let Err(reason) = onepassword::authorize(vault) {
        eprintln!("op-bridge: refill skipped: {reason}");
        return;
    }
    let mut filled = 0;
    for reference in &missing {
        match onepassword::read(reference) {
            Ok(value) => {
                if let Ok(mut broker) = broker.lock() {
                    broker.store(reference, value);
                    filled += 1;
                }
            }
            Err(reason) => eprintln!("op-bridge: refill {reference}: {reason}"),
        }
    }
    eprintln!("op-bridge: refilled {filled}/{}", missing.len());
}

fn serve(
    stream: &UnixStream,
    broker: &Mutex<Broker>,
    tunnel: &AtomicU32,
    known_path: &Path,
) -> Result<(), String> {
    let pid = peer_pid(stream)?;
    let origin = if pid != 0 && pid == tunnel.load(Ordering::SeqCst) {
        Origin::Peer
    } else {
        Origin::Local
    };
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| error.to_string())?;
    let request: Request = protocol::receive(stream)?;
    let now = Instant::now();
    let mut broker = broker.lock().map_err(|_| "broker state poisoned")?;
    let cached = broker.cached(&request.read, now);
    let response = broker.resolve(
        &request.read,
        origin,
        now,
        touchid::approve,
        onepassword::read,
    );
    let known = broker.take_known();
    drop(broker);
    if let Some(known) = known {
        save_known(known_path, &known)?;
    }
    eprintln!(
        "op-bridge: {} {}: {}",
        origin.name(),
        request.read,
        outcome(&response, cached)
    );
    protocol::send(stream, &response)
}

fn outcome(response: &Response, cached: bool) -> String {
    match response {
        Response::Value(_) if cached => "sent from memory".to_string(),
        Response::Value(_) => "fetched".to_string(),
        Response::Denied(reason) => format!("denied: {reason}"),
        Response::Refused(reason) => format!("refused: {reason}"),
    }
}

pub fn load_known(path: &Path) -> BTreeSet<String> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_known(path: &Path, known: &[String]) -> Result<(), String> {
    let display = |error: std::io::Error| format!("{}: {error}", path.display());
    let text = serde_json::to_vec_pretty(known).map_err(|error| error.to_string())?;
    let staged = path.with_extension("json.new");
    fs::write(&staged, text).map_err(display)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).map_err(display)?;
    fs::rename(&staged, path).map_err(display)
}

#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> Result<u32, String> {
    let pid = nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::LocalPeerPid)
        .map_err(|error| format!("peer pid: {error}"))?;
    u32::try_from(pid).map_err(|_| format!("peer pid: {pid}"))
}

#[cfg(target_os = "linux")]
fn peer_pid(stream: &UnixStream) -> Result<u32, String> {
    let credentials =
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
            .map_err(|error| format!("peer pid: {error}"))?;
    u32::try_from(credentials.pid()).map_err(|_| format!("peer pid: {}", credentials.pid()))
}

#[cfg(test)]
#[path = "../tests/unit/daemon_tests.rs"]
mod tests;

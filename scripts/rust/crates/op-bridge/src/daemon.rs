use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hostkit::Host;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use crate::broker::{Broker, GRANT};
use crate::protocol::{self, Request, Response};
use crate::{onepassword, paths, touchid, tunnel};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run(vaults: Vec<String>) -> Result<(), String> {
    let this = Host::this()?;
    if this != Host::Macie {
        return Err("the daemon runs on macie; archie reaches it through op".to_string());
    }
    let peer = this.peer();
    let socket = paths::broker()?;
    let listener = bind(&socket)?;
    eprintln!(
        "op-bridge: listening on {}; {} may read {}",
        socket.display(),
        peer.name(),
        vaults.join(", ")
    );

    let tunnel = Arc::new(AtomicU32::new(0));
    stop_on_signal(Arc::clone(&tunnel), socket.clone())?;
    {
        let tunnel = Arc::clone(&tunnel);
        thread::spawn(move || tunnel::supervise(peer, &socket, &tunnel));
    }

    let broker = Arc::new(Mutex::new(Broker::new(vaults, GRANT)));
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("op-bridge: accept: {error}");
                continue;
            }
        };
        let (broker, tunnel) = (Arc::clone(&broker), Arc::clone(&tunnel));
        thread::spawn(move || {
            if let Err(error) = serve(&stream, peer, &broker, &tunnel) {
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

fn serve(
    stream: &UnixStream,
    peer: Host,
    broker: &Mutex<Broker>,
    tunnel: &AtomicU32,
) -> Result<(), String> {
    let pid = peer_pid(stream)?;
    if pid == 0 || pid != tunnel.load(Ordering::SeqCst) {
        return Err(format!("rejected pid {pid}: only the tunnel may ask"));
    }
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .map_err(|error| error.to_string())?;
    let request: Request = protocol::receive(stream)?;
    let now = Instant::now();
    let mut broker = broker.lock().map_err(|_| "broker state poisoned")?;
    let granted = broker.granted(&request.read, now);
    let response = broker.resolve(
        &request.read,
        peer.name(),
        now,
        touchid::approve,
        onepassword::read,
    );
    drop(broker);
    eprintln!(
        "op-bridge: {} {}: {}",
        peer.name(),
        request.read,
        outcome(&response, granted)
    );
    protocol::send(stream, &response)
}

fn outcome(response: &Response, granted: bool) -> String {
    match response {
        Response::Value(_) if granted => "sent within grant".to_string(),
        Response::Value(_) => "approved".to_string(),
        Response::Denied(reason) => format!("denied: {reason}"),
        Response::Refused(reason) => format!("refused: {reason}"),
    }
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

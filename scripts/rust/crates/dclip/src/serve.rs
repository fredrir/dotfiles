use std::io::{self, Read};
use std::net::{SocketAddrV4, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use hostkit::{Host, Route};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use crate::{endpoint, native, proto, tls};

const RETRY: Duration = Duration::from_secs(5);
const BACKOFF: Duration = Duration::from_millis(100);
const STALL: Duration = Duration::from_secs(3);
const CONNECTIONS: usize = 32;

struct Shared {
    config: Arc<ServerConfig>,
    user: String,
    active: AtomicUsize,
}

struct Slot {
    route: Route,
    address: SocketAddrV4,
    live: Arc<AtomicBool>,
    failing: bool,
}

struct Admitted(Arc<Shared>);

impl Drop for Admitted {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn serve(this: Host) -> Result<(), String> {
    let shared = Arc::new(Shared {
        config: tls::server()?,
        user: tls::user()?,
        active: AtomicUsize::new(0),
    });
    let mut slots: Vec<Slot> = Route::every()
        .into_iter()
        .map(|route| Slot {
            route,
            address: endpoint::listen(this, route),
            live: Arc::new(AtomicBool::new(false)),
            failing: false,
        })
        .collect();
    loop {
        for slot in &mut slots {
            if !slot.live.load(Ordering::Acquire) {
                bind(slot, &shared);
            }
        }
        thread::sleep(RETRY);
    }
}

fn bind(slot: &mut Slot, shared: &Arc<Shared>) {
    let listener = match TcpListener::bind(slot.address) {
        Ok(listener) => listener,
        Err(error) => {
            if !slot.failing {
                eprintln!("dclip: {} {}: {error}", slot.route.name(), slot.address);
                slot.failing = true;
            }
            return;
        }
    };
    if slot.failing {
        eprintln!("dclip: {} {}: listening", slot.route.name(), slot.address);
        slot.failing = false;
    }
    slot.live.store(true, Ordering::Release);
    let (live, shared, label) = (
        Arc::clone(&slot.live),
        Arc::clone(shared),
        format!("{} {}", slot.route.name(), slot.address),
    );
    let spawned = thread::Builder::new().spawn(move || {
        let error = accept(&listener, &shared);
        eprintln!("dclip: {label}: {error}");
        live.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        slot.live.store(false, Ordering::Release);
    }
}

fn accept(listener: &TcpListener, shared: &Arc<Shared>) -> io::Error {
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if transient(&error) => {
                thread::sleep(BACKOFF);
                continue;
            }
            Err(error) => return error,
        };
        if shared.active.fetch_add(1, Ordering::Relaxed) >= CONNECTIONS {
            shared.active.fetch_sub(1, Ordering::Relaxed);
            continue;
        }
        let admitted = Admitted(Arc::clone(shared));
        let _ = thread::Builder::new().spawn(move || {
            if let Err(reason) = answer(stream, &admitted.0) {
                eprintln!("dclip: {reason}");
            }
            drop(admitted);
        });
    }
}

fn transient(error: &io::Error) -> bool {
    use nix::errno::Errno;
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
    ) || error.raw_os_error().is_some_and(|code| {
        [
            Errno::EMFILE,
            Errno::ENFILE,
            Errno::ENOBUFS,
            Errno::ENOMEM,
            Errno::EPROTO,
        ]
        .contains(&Errno::from_raw(code))
    })
}

fn answer(tcp: TcpStream, shared: &Shared) -> Result<(), String> {
    let peer = tcp
        .peer_addr()
        .map_or_else(|_| "?".into(), |peer| peer.to_string());
    let deadline = Instant::now() + STALL;
    let setup = tcp
        .set_nodelay(true)
        .and_then(|()| tcp.set_read_timeout(Some(STALL)))
        .and_then(|()| tcp.set_write_timeout(Some(STALL)));
    if setup.is_err() {
        return Ok(());
    }
    let mut tcp = tcp;
    let Ok(mut connection) = ServerConnection::new(Arc::clone(&shared.config)) else {
        return Ok(());
    };
    while connection.is_handshaking() {
        if Instant::now() >= deadline {
            return Ok(());
        }
        if let Err(error) = connection.complete_io(&mut tcp) {
            return match rejected(&error) {
                Some(reason) => Err(format!("{peer}: {reason}")),
                None => Ok(()),
            };
        }
    }
    tls::authorize(&connection, &shared.user).map_err(|reason| format!("{peer}: {reason}"))?;
    let mut stream = StreamOwned::new(connection, tcp);
    let mut request = [0_u8; 2];
    if stream.read_exact(&mut request).is_err() {
        return Ok(());
    }
    proto::check_request(request).map_err(|reason| format!("{peer}: {reason}"))?;
    let reply = native::read_for_peer();
    if let Err(reason) = &reply {
        eprintln!("dclip: {reason}");
    }
    if proto::write_reply(&mut stream, &reply).is_ok() {
        stream.conn.send_close_notify();
        let _ = stream.conn.complete_io(&mut stream.sock);
    }
    Ok(())
}

fn rejected(error: &io::Error) -> Option<String> {
    let tls = error.get_ref()?.downcast_ref::<rustls::Error>()?;
    match tls {
        rustls::Error::InvalidCertificate(_)
        | rustls::Error::NoCertificatesPresented
        | rustls::Error::AlertReceived(_)
        | rustls::Error::PeerIncompatible(_) => Some(tls.to_string()),
        _ => None,
    }
}

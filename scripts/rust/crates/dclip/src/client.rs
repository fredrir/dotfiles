use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hostkit::{Host, Route};
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use crate::{PEER_READ, endpoint, proto, tls};

pub const BUDGET: Duration = Duration::from_millis(300);
const STAGGER: Duration = Duration::from_millis(100);
const HANDSHAKE: Duration = Duration::from_secs(1);
const REPLY: Duration = PEER_READ.saturating_add(Duration::from_millis(500));
const STALL: Duration = Duration::from_secs(2);

type Stream = StreamOwned<ClientConnection, TcpStream>;

struct Ready {
    route: Route,
    stream: Stream,
}

enum Event {
    Answered,
    Ready(Box<Ready>),
    Failed,
}

pub fn waves(preferred: &[Route]) -> (Vec<Route>, Vec<Route>) {
    let mut first: Vec<Route> = Vec::new();
    for route in preferred {
        if !first.contains(route) {
            first.push(*route);
        }
    }
    let rest = Route::every()
        .into_iter()
        .filter(|route| !first.contains(route))
        .collect();
    (first, rest)
}

pub fn paste(peer: Host, preferred: &[Route]) -> Result<(Route, String), String> {
    let config = tls::client()?;
    let started = Instant::now();
    let budget = started + BUDGET;
    let (first, rest) = waves(preferred);
    let (sender, receiver) = mpsc::channel();
    let launch = |routes: Vec<Route>| {
        for route in &routes {
            let (config, sender, route) = (Arc::clone(&config), sender.clone(), *route);
            thread::spawn(move || {
                let event = attempt(&config, peer, route, budget, &sender)
                    .map_or(Event::Failed, |ready| Event::Ready(Box::new(ready)));
                let _ = sender.send(event);
            });
        }
        routes.len()
    };
    let mut pending = launch(first);
    let mut rest = Some(rest);
    let stagger = started + STAGGER;
    let mut deadline = budget;
    loop {
        if (pending == 0 || Instant::now() >= stagger)
            && let Some(routes) = rest.take()
        {
            pending += launch(routes);
        }
        if pending == 0 {
            break;
        }
        let until = if rest.is_some() { stagger } else { deadline };
        match receiver.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok(Event::Ready(ready)) => return request(peer, *ready),
            Ok(Event::Answered) => deadline = deadline.max(Instant::now() + HANDSHAKE),
            Ok(Event::Failed) => pending -= 1,
            Err(_) if Instant::now() >= deadline => break,
            Err(_) => {}
        }
    }
    Err(format!("{} unreachable", peer.name()))
}

fn left(deadline: Instant) -> Option<Duration> {
    Some(deadline.saturating_duration_since(Instant::now())).filter(|left| !left.is_zero())
}

fn attempt(
    config: &Arc<ClientConfig>,
    peer: Host,
    route: Route,
    budget: Instant,
    events: &mpsc::Sender<Event>,
) -> Option<Ready> {
    let address = endpoint::dial(peer, route);
    let mut tcp = hostkit::socket::connect(None, address, left(budget)?).ok()?;
    // A local relay accepts whether or not the peer is up, so only a direct answer earns more time.
    let deadline = if address.ip().is_loopback() {
        budget
    } else {
        let _ = events.send(Event::Answered);
        Instant::now() + HANDSHAKE
    };
    tcp.set_nodelay(true).ok()?;
    let mut connection =
        ClientConnection::new(Arc::clone(config), tls::server_name(peer).ok()?).ok()?;
    while connection.is_handshaking() {
        let left = left(deadline)?;
        tcp.set_read_timeout(Some(left)).ok()?;
        tcp.set_write_timeout(Some(left)).ok()?;
        connection.complete_io(&mut tcp).ok()?;
    }
    Some(Ready {
        route,
        stream: StreamOwned::new(connection, tcp),
    })
}

fn request(peer: Host, mut ready: Ready) -> Result<(Route, String), String> {
    let silent = |_| format!("{} did not reply", peer.name());
    let stream = &mut ready.stream;
    stream.sock.set_read_timeout(Some(REPLY)).map_err(silent)?;
    stream.sock.set_write_timeout(Some(REPLY)).map_err(silent)?;
    stream.write_all(&proto::REQUEST).map_err(silent)?;
    stream.flush().map_err(silent)?;
    let mut header = [0_u8; proto::HEADER];
    stream.read_exact(&mut header).map_err(silent)?;
    let header = proto::parse_header(header)?;
    stream.sock.set_read_timeout(Some(STALL)).map_err(silent)?;
    let text = proto::read_body(stream, &header)?;
    Ok((ready.route, text))
}

#[cfg(test)]
#[path = "../tests/unit/client_tests.rs"]
mod tests;

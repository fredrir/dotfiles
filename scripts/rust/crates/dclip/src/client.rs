use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hostkit::{Host, Route};
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use crate::proto::{self, Header};
use crate::{endpoint, tls};

pub const BUDGET: Duration = Duration::from_millis(300);
const STAGGER: Duration = Duration::from_millis(100);
const STALL: Duration = Duration::from_secs(2);

type Stream = StreamOwned<ClientConnection, TcpStream>;

struct Won {
    route: Route,
    stream: Stream,
    header: Header,
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
    let deadline = started + BUDGET;
    let (first, rest) = waves(preferred);
    let (sender, receiver) = mpsc::channel();
    let launch = |routes: Vec<Route>| {
        for route in &routes {
            let (config, sender, route) = (Arc::clone(&config), sender.clone(), *route);
            thread::spawn(move || {
                let _ = sender.send(attempt(&config, peer, route, deadline));
            });
        }
        routes.len()
    };
    let mut pending = launch(first);
    let mut rest = Some(rest);
    let stagger = started + STAGGER;
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
            Ok(Some(won)) => return finish(won),
            Ok(None) => pending -= 1,
            Err(_) if Instant::now() >= deadline => break,
            Err(_) => {}
        }
    }
    Err(format!("{} unreachable", peer.name()))
}

fn attempt(config: &Arc<ClientConfig>, peer: Host, route: Route, deadline: Instant) -> Option<Won> {
    let left =
        || Some(deadline.saturating_duration_since(Instant::now())).filter(|left| !left.is_zero());
    let tcp = hostkit::socket::connect(None, endpoint::dial(peer, route), left()?).ok()?;
    let left = left()?;
    tcp.set_nodelay(true).ok()?;
    tcp.set_read_timeout(Some(left)).ok()?;
    tcp.set_write_timeout(Some(left)).ok()?;
    let connection =
        ClientConnection::new(Arc::clone(config), tls::server_name(peer).ok()?).ok()?;
    let mut stream = StreamOwned::new(connection, tcp);
    stream.write_all(&proto::REQUEST).ok()?;
    stream.flush().ok()?;
    let mut header = [0_u8; proto::HEADER];
    stream.read_exact(&mut header).ok()?;
    let header = proto::parse_header(header).ok()?;
    Some(Won {
        route,
        stream,
        header,
    })
}

fn finish(mut won: Won) -> Result<(Route, String), String> {
    let _ = won.stream.sock.set_read_timeout(Some(STALL));
    let text = proto::read_body(&mut won.stream, &won.header)?;
    Ok((won.route, text))
}

#[cfg(test)]
#[path = "../tests/unit/client_tests.rs"]
mod tests;

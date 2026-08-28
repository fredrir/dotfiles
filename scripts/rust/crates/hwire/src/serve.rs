//! The peer's half: accept a connection, do what its header says, repeat.
//!
//! Normally nobody starts this by hand — the measuring side runs it over ssh
//! and stops it when it is done. It still has to survive the other endings:
//! the client dying mid-transfer, or its ssh being killed, would otherwise
//! leave a process listening on the peer forever. Hence the idle timeout,
//! which is the only thing standing between a stray run and a stray daemon.

use std::io::{self, Write};
use std::net::{SocketAddrV4, TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::proto::{self, Header, Mode};

/// Printed on stdout as soon as the listener is up, and parsed by the side
/// that started this over ssh: with `--port 0` the port is not known until
/// the bind, and the address confirms which route the peer is listening on.
pub const BANNER: &str = "hwire serve";

/// How long a listener with a deadline sleeps between asking for a connection,
/// which is the most an accept can be late by.
const TICK: Duration = Duration::from_millis(5);

pub fn serve(
    bind: SocketAddrV4,
    token: Option<[u8; 16]>,
    idle: Option<Duration>,
) -> io::Result<()> {
    let listener = TcpListener::bind(bind)?;
    let address = listener.local_addr()?;
    println!("{BANNER} {} {}", address.ip(), address.port());
    io::stdout().flush()?;
    answer(listener, token, idle)
}

/// Take one connection at a time until a client says to stop or `idle` passes
/// with nobody there.
fn answer(
    listener: TcpListener,
    token: Option<[u8; 16]>,
    idle: Option<Duration>,
) -> io::Result<()> {
    // Only a server holding a deadline has to wake up and look at it; one told
    // to wait forever blocks in accept, which costs nothing while it waits.
    listener.set_nonblocking(idle.is_some())?;

    let mut phases: Vec<JoinHandle<()>> = Vec::new();
    let mut deadline = idle.map(|idle| Instant::now() + idle);
    while let Some(mut stream) = accept(&listener, deadline)? {
        deadline = idle.map(|idle| Instant::now() + idle);

        // The header is read here rather than in the phase thread: it is 32
        // bytes the client sends immediately, and reading it is what tells
        // this loop whether there is another phase coming at all.
        stream.set_read_timeout(Some(proto::STALL))?;
        let header = match proto::read_exactly::<{ proto::HEADER }>(&mut stream)
            .map_err(|error| error.to_string())
            .and_then(|bytes| Header::decode(&bytes))
        {
            Ok(header) if token.is_none_or(|token| header.token == token) => header,
            Ok(_) => {
                eprintln!("hwire: a connection arrived without this run's token");
                continue;
            }
            Err(reason) => {
                eprintln!("hwire: {reason}");
                continue;
            }
        };
        if header.mode == Mode::Bye {
            break;
        }
        phases.push(std::thread::spawn(move || {
            if let Err(error) = phase(&mut stream, header) {
                eprintln!("hwire: {error}");
            }
        }));
    }

    for phase in phases {
        let _ = phase.join();
    }
    Ok(())
}

/// The next connection, or `None` once `deadline` has passed without one.
fn accept(listener: &TcpListener, deadline: Option<Instant>) -> io::Result<Option<TcpStream>> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // BSD hands an accepted socket the listener's non-blocking
                // flag and Linux does not, and every read below wants a
                // blocking one.
                stream.set_nonblocking(false)?;
                return Ok(Some(stream));
            }
            // Only a listener with a deadline is non-blocking, so only one
            // with a deadline gets here; `TICK` is a bound, not a wait.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let left = deadline.map_or(TICK, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                });
                if left.is_zero() {
                    return Ok(None);
                }
                std::thread::sleep(TICK.min(left));
            }
            Err(error) => return Err(error),
        }
    }
}

fn phase(stream: &mut TcpStream, header: Header) -> io::Result<()> {
    stream.set_write_timeout(Some(proto::STALL))?;
    match header.mode {
        Mode::Ping => {
            stream.set_nodelay(true)?;
            echo(stream)
        }
        // The client sends, so the count that matters is the one taken here.
        Mode::Send => {
            let counted = proto::drain(stream)?;
            stream.write_all(&counted.encode())
        }
        Mode::Recv => proto::blast(stream, header.window),
        Mode::Bye => Ok(()),
    }
}

/// Bounce every payload straight back until the client hangs up, which is
/// how the latency phase ends.
fn echo(stream: &mut TcpStream) -> io::Result<()> {
    loop {
        match proto::read_exactly::<{ proto::PING }>(stream) {
            Ok(payload) => stream.write_all(&payload)?,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::Ipv4Addr;

    fn listener() -> (TcpListener, u16) {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    fn header(mode: Mode) -> [u8; proto::HEADER] {
        Header {
            mode,
            token: [0u8; 16],
            window: Duration::ZERO,
        }
        .encode()
    }

    fn dial(port: u16) -> TcpStream {
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
    }

    /// The idle timeout is what keeps a stray run from leaving a daemon, so it
    /// has to fire with nothing connecting to wake the loop up.
    #[test]
    fn nobody_connecting_ends_the_server_when_the_idle_time_is_up() {
        let (listener, _) = listener();
        let started = Instant::now();
        answer(listener, None, Some(Duration::from_millis(150))).unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(150), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }

    /// A listener that polls for its deadline is a non-blocking one and BSD
    /// hands that flag to every socket it accepts, so the header read has to
    /// wait for a client that has not written yet rather than give up on it.
    #[test]
    fn a_phase_is_answered_on_a_listener_that_is_watching_its_deadline() {
        let (listener, port) = listener();
        let served =
            std::thread::spawn(move || answer(listener, None, Some(Duration::from_secs(10))));

        // Connect well before saying anything, so the server is waiting on an
        // empty socket at the point a non-blocking one would return instead.
        let mut client = dial(port);
        std::thread::sleep(Duration::from_millis(50));
        client.write_all(&header(Mode::Ping)).unwrap();
        client.write_all(&7u64.to_be_bytes()).unwrap();
        let mut echoed = [0u8; proto::PING];
        client.read_exact(&mut echoed).unwrap();
        assert_eq!(u64::from_be_bytes(echoed), 7);

        dial(port).write_all(&header(Mode::Bye)).unwrap();
        drop(client);
        served.join().unwrap().unwrap();
    }

    /// A connection carrying the wrong token is discarded rather than answered,
    /// and the server is still there for the one after it.
    #[test]
    fn a_connection_without_the_token_is_dropped_and_the_server_stays_up() {
        let (listener, port) = listener();
        let token = [9u8; 16];
        let served = std::thread::spawn(move || {
            answer(listener, Some(token), Some(Duration::from_secs(10)))
        });

        dial(port).write_all(&header(Mode::Ping)).unwrap();
        let bye = Header {
            mode: Mode::Bye,
            token,
            window: Duration::ZERO,
        };
        dial(port).write_all(&bye.encode()).unwrap();
        served.join().unwrap().unwrap();
    }
}

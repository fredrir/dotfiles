//! The peer's half: accept a connection, do what its header says, repeat.
//!
//! Normally nobody starts this by hand — the measuring side runs it over ssh
//! and stops it when it is done. It still has to survive the other endings:
//! the client dying mid-transfer, or its ssh being killed, would otherwise
//! leave a process listening on the peer forever. Hence the idle timeout,
//! which is the only thing standing between a stray run and a stray daemon.

use std::io::{self, Write};
use std::net::{SocketAddrV4, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::proto::{self, Header, Mode};
use crate::socket;

/// Printed on stdout as soon as the listener is up, and parsed by the side
/// that started this over ssh: with `--port 0` the port is not known until
/// the bind, and the address confirms which route the peer is listening on.
pub const BANNER: &str = "hwire serve";

pub fn serve(
    bind: SocketAddrV4,
    token: Option<[u8; 16]>,
    idle: Option<Duration>,
) -> io::Result<()> {
    let listener = TcpListener::bind(bind)?;
    let address = listener.local_addr()?;
    println!("{BANNER} {} {}", address.ip(), address.port());
    io::stdout().flush()?;

    let mut phases: Vec<JoinHandle<()>> = Vec::new();
    let mut deadline = idle.map(|idle| Instant::now() + idle);
    loop {
        if let Some(deadline) = deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !socket::wait(listener.as_raw_fd(), libc::POLLIN, left)? {
                break;
            }
        }
        let (mut stream, _) = listener.accept()?;
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

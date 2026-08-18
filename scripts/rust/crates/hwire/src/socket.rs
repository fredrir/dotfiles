//! The two socket operations std does not offer: connecting from a chosen
//! local address, and waiting on a descriptor with a deadline.
//!
//! The local bind is what makes a route claim mean something. Both machines
//! can reach each other over the cable and over Tailscale at the same time,
//! and the routing table alone decides which — so a measurement that only
//! names a destination is measuring whichever path the kernel liked. Binding
//! this side's address for the route under test forces the packets onto it,
//! the way `ssh`'s `BindInterface` does for the cabled hosts.

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::os::fd::{FromRawFd, RawFd};
use std::time::Duration;

/// A descriptor that closes itself, so every early return on the way to a
/// `TcpStream` is a closed socket rather than a leaked one.
struct Owned(RawFd);

impl Owned {
    fn release(self) -> RawFd {
        let fd = self.0;
        std::mem::forget(self);
        fd
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: closing the descriptor this value owns, exactly once.
        unsafe { libc::close(self.0) };
    }
}

/// Connect to `peer`, optionally from `local`, giving up after `timeout`.
///
/// The connect is made non-blocking only to bound it; the stream handed back
/// is an ordinary blocking one.
pub fn connect(
    local: Option<Ipv4Addr>,
    peer: SocketAddrV4,
    timeout: Duration,
) -> io::Result<TcpStream> {
    let length = size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let peer_address = sockaddr(*peer.ip(), peer.port());
    // SAFETY: plain socket syscalls on a descriptor owned by `socket`; every
    // sockaddr passed in is a fully initialised sockaddr_in of the length
    // given alongside it, and the fd outlives each call.
    unsafe {
        let socket = Owned(libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0));
        if socket.0 < 0 {
            return Err(io::Error::last_os_error());
        }
        if let Some(local) = local {
            let local_address = sockaddr(local, 0);
            if libc::bind(socket.0, (&raw const local_address).cast(), length) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let flags = libc::fcntl(socket.0, libc::F_GETFL);
        if flags < 0 || libc::fcntl(socket.0, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::connect(socket.0, (&raw const peer_address).cast(), length) != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
            if !wait(socket.0, libc::POLLOUT, timeout)? {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "connection timed out",
                ));
            }
            let mut pending: libc::c_int = 0;
            let mut pending_length = size_of::<libc::c_int>() as libc::socklen_t;
            let asked = libc::getsockopt(
                socket.0,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut pending).cast(),
                &raw mut pending_length,
            );
            if asked != 0 {
                return Err(io::Error::last_os_error());
            }
            if pending != 0 {
                return Err(io::Error::from_raw_os_error(pending));
            }
        }
        if libc::fcntl(socket.0, libc::F_SETFL, flags) < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the descriptor is a connected stream socket, and `release`
        // gives up this function's claim to closing it.
        Ok(TcpStream::from_raw_fd(socket.release()))
    }
}

/// Whether `fd` is ready for `events` before `timeout` runs out.
pub fn wait(fd: RawFd, events: libc::c_short, timeout: Duration) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    let millis = timeout.as_millis().min(libc::c_int::MAX as u128) as libc::c_int;
    loop {
        // SAFETY: one initialised pollfd, and the count matches.
        let ready = unsafe { libc::poll(&mut poll, 1, millis) };
        if ready >= 0 {
            return Ok(ready == 1);
        }
        let error = io::Error::last_os_error();
        // A signal during the wait is not an answer about the socket.
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn sockaddr(address: Ipv4Addr, port: u16) -> libc::sockaddr_in {
    // SAFETY: sockaddr_in is plain data; zero is a valid value for every
    // field, including the BSD-only sin_len this avoids naming.
    let mut sockaddr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    sockaddr.sin_family = libc::AF_INET as libc::sa_family_t;
    sockaddr.sin_port = port.to_be();
    sockaddr.sin_addr = libc::in_addr {
        s_addr: u32::from(address).to_be(),
    };
    sockaddr
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn connects_to_a_listener_and_reports_the_bound_address() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stream = connect(
            Some(Ipv4Addr::LOCALHOST),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(stream.peer_addr().unwrap().port(), port);
        assert_eq!(stream.local_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
        assert!(listener.accept().is_ok());
    }

    #[test]
    fn a_closed_port_is_refused_rather_than_waited_on() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let started = Instant::now();
        let answer = connect(
            None,
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            Duration::from_secs(5),
        );
        assert!(answer.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn an_unroutable_peer_gives_up_when_the_timeout_says_to() {
        let started = Instant::now();
        let answer = connect(
            None,
            // TEST-NET-1: reserved for documentation, so nothing answers.
            SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 1), 9),
            Duration::from_millis(150),
        );
        assert!(answer.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_local_address_this_machine_does_not_have_fails_at_the_bind() {
        let answer = connect(
            Some(Ipv4Addr::new(192, 0, 2, 3)),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9),
            Duration::from_millis(150),
        );
        assert!(answer.is_err());
    }
}

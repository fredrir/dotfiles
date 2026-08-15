//! The two machines and the cable between them.
//!
//! Hardcoded on purpose: this is personal infrastructure for exactly two
//! hosts, and the addresses already live in `wez/remote/mux.lua` and the ssh
//! configs. The usb probe binds its local socket to this machine's usb
//! address so the connection is forced over the cable — an answer proves the
//! link, not just that the peer is reachable somehow.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use clap::ValueEnum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Host {
    Macie,
    Archie,
}

impl Host {
    pub fn this() -> Result<Host, String> {
        match std::env::consts::OS {
            "macos" => Ok(Host::Macie),
            "linux" => Ok(Host::Archie),
            other => Err(format!("unsupported operating system: {other}")),
        }
    }

    pub fn peer(self) -> Host {
        match self {
            Host::Macie => Host::Archie,
            Host::Archie => Host::Macie,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Host::Macie => "macie",
            Host::Archie => "archie",
        }
    }

    pub fn usb_address(self) -> Ipv4Addr {
        match self {
            Host::Macie => Ipv4Addr::new(10, 77, 77, 1),
            Host::Archie => Ipv4Addr::new(10, 77, 77, 2),
        }
    }

    pub fn ts_address(self) -> Ipv4Addr {
        match self {
            Host::Macie => Ipv4Addr::new(100, 75, 71, 79),
            Host::Archie => Ipv4Addr::new(100, 126, 231, 24),
        }
    }
}

/// What every verb decides transport from.
pub struct Context {
    pub host: Host,
    pub local: bool,
    pub inside_wezterm: bool,
    pub inside_tmux: bool,
}

impl Context {
    pub fn resolve(requested: Option<Host>) -> Result<Context, String> {
        let this = Host::this()?;
        let host = requested.unwrap_or(this);
        Ok(Context {
            host,
            local: host == this,
            inside_wezterm: std::env::var_os("WEZTERM_UNIX_SOCKET").is_some(),
            inside_tmux: std::env::var_os("TMUX").is_some(),
        })
    }
}

pub const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// Latency of a TCP connect to the peer's ssh port over the cable, `None`
/// when the link is down. A failure to bind the local usb address means the
/// interface is not up, which is the same answer.
pub fn usb_latency(timeout: Duration) -> Option<Duration> {
    let this = Host::this().ok()?;
    connect_latency(this.usb_address(), this.peer().usb_address(), 22, timeout)
}

struct Socket(libc::c_int);

impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: closing the descriptor this struct owns.
        unsafe { libc::close(self.0) };
    }
}

/// std's `TcpStream` cannot bind before connecting, so this is the classic
/// nonblocking connect: bind, connect, poll for writability, read `SO_ERROR`.
fn connect_latency(
    local: Ipv4Addr,
    peer: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Option<Duration> {
    let start = Instant::now();
    let length = size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let local_address = sockaddr(local, 0);
    let peer_address = sockaddr(peer, port);
    // SAFETY: plain socket syscalls on a descriptor this function owns; every
    // sockaddr passed in is a fully initialised sockaddr_in of the length
    // given alongside it.
    unsafe {
        let socket = Socket(libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0));
        if socket.0 < 0 {
            return None;
        }
        if libc::bind(socket.0, (&raw const local_address).cast(), length) != 0 {
            return None;
        }
        let flags = libc::fcntl(socket.0, libc::F_GETFL);
        if flags < 0 || libc::fcntl(socket.0, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return None;
        }
        if libc::connect(socket.0, (&raw const peer_address).cast(), length) != 0 {
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
                return None;
            }
            let mut poll = libc::pollfd {
                fd: socket.0,
                events: libc::POLLOUT,
                revents: 0,
            };
            if libc::poll(&mut poll, 1, timeout.as_millis() as libc::c_int) != 1 {
                return None;
            }
            let mut error: libc::c_int = 0;
            let mut error_length = size_of::<libc::c_int>() as libc::socklen_t;
            let asked = libc::getsockopt(
                socket.0,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut error).cast(),
                &raw mut error_length,
            );
            if asked != 0 || error != 0 {
                return None;
            }
        }
        Some(start.elapsed())
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

    #[test]
    fn the_peers_are_symmetric() {
        assert_eq!(Host::Macie.peer(), Host::Archie);
        assert_eq!(Host::Archie.peer(), Host::Macie);
        assert_eq!(Host::Macie.peer().peer(), Host::Macie);
    }

    #[test]
    fn addresses_match_the_wezterm_config() {
        assert_eq!(Host::Macie.usb_address(), Ipv4Addr::new(10, 77, 77, 1));
        assert_eq!(Host::Archie.usb_address(), Ipv4Addr::new(10, 77, 77, 2));
        assert_eq!(Host::Macie.ts_address(), Ipv4Addr::new(100, 75, 71, 79));
        assert_eq!(Host::Archie.ts_address(), Ipv4Addr::new(100, 126, 231, 24));
    }

    #[test]
    fn an_unroutable_probe_comes_back_down_quickly() {
        let started = Instant::now();
        let answer = connect_latency(
            Ipv4Addr::new(192, 0, 2, 1),
            Ipv4Addr::new(192, 0, 2, 2),
            9,
            Duration::from_millis(100),
        );
        assert!(answer.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

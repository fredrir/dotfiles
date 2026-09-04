use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Duration;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

pub fn connect(
    local: Option<Ipv4Addr>,
    peer: SocketAddrV4,
    timeout: Duration,
) -> io::Result<TcpStream> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    if let Some(local) = local {
        socket.bind(&SockAddr::from(SocketAddrV4::new(local, 0)))?;
    }
    socket.connect_timeout(&SockAddr::from(peer), timeout)?;
    if let Some(refused) = socket.take_error()? {
        return Err(refused);
    }
    Ok(TcpStream::from(socket))
}

#[cfg(test)]
#[path = "../tests/unit/socket_tests.rs"]
mod tests;

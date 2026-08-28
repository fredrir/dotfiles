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
    fn the_stream_is_blocking_rather_than_however_the_connect_left_it() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut stream = connect(
            None,
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            Duration::from_secs(1),
        )
        .unwrap();
        let (mut accepted, _) = listener.accept().unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            std::io::Write::write_all(&mut accepted, b"hi").unwrap();
        });
        let mut answer = [0u8; 2];
        std::io::Read::read_exact(&mut stream, &mut answer).unwrap();
        assert_eq!(&answer, b"hi");
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
        assert_eq!(answer.unwrap_err().kind(), io::ErrorKind::ConnectionRefused);
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
        // Whatever the kernel decides that address is, it is not a refusal,
        // which is the distinction every caller here reads the error for.
        assert_ne!(answer.unwrap_err().kind(), io::ErrorKind::ConnectionRefused);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_local_address_this_machine_does_not_have_fails_at_the_bind() {
        let started = Instant::now();
        let answer = connect(
            Some(Ipv4Addr::new(192, 0, 2, 3)),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9),
            Duration::from_millis(150),
        );
        assert_eq!(answer.unwrap_err().kind(), io::ErrorKind::AddrNotAvailable);
        assert!(started.elapsed() < Duration::from_millis(50));
    }
}

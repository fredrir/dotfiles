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

#[test]
fn nobody_connecting_ends_the_server_when_the_idle_time_is_up() {
    let (listener, _) = listener();
    let started = Instant::now();
    answer(listener, None, Some(Duration::from_millis(150))).unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(150), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

#[test]
fn a_phase_is_answered_on_a_listener_that_is_watching_its_deadline() {
    let (listener, port) = listener();
    let served = std::thread::spawn(move || answer(listener, None, Some(Duration::from_secs(10))));

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

#[test]
fn a_connection_without_the_token_is_dropped_and_the_server_stays_up() {
    let (listener, port) = listener();
    let token = [9u8; 16];
    let served =
        std::thread::spawn(move || answer(listener, Some(token), Some(Duration::from_secs(10))));

    dial(port).write_all(&header(Mode::Ping)).unwrap();
    let bye = Header {
        mode: Mode::Bye,
        token,
        window: Duration::ZERO,
    };
    dial(port).write_all(&bye.encode()).unwrap();
    served.join().unwrap().unwrap();
}

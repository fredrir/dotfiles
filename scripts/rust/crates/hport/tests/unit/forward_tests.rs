use super::*;
use std::net::TcpListener;

fn forward(bind: Bind, target: &str) -> Forward {
    let target: SocketAddr = target.parse().unwrap();
    Forward {
        bind,
        port: target.port(),
        target,
    }
}

#[test]
fn the_alias_forward_binds_127_0_0_2() {
    assert_eq!(
        forward(Bind::Alias, "127.0.0.1:5173").spec(),
        "127.0.0.2:5173:127.0.0.1:5173"
    );
}

#[test]
fn the_mirror_binds_both_loopback_families() {
    assert_eq!(
        forward(Bind::Loopback4, "127.0.0.1:5173").spec(),
        "127.0.0.1:5173:127.0.0.1:5173"
    );
    assert_eq!(
        forward(Bind::Loopback6, "127.0.0.1:5173").spec(),
        "[::1]:5173:127.0.0.1:5173"
    );
}

#[test]
fn ipv6_targets_are_bracketed() {
    assert_eq!(
        forward(Bind::Alias, "[::1]:3000").spec(),
        "127.0.0.2:3000:[::1]:3000"
    );
}

#[test]
fn a_port_held_on_ipv4_loopback_is_not_free_for_the_mirror() {
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    assert!(!mirror_free(held.local_addr().unwrap().port()));
}

#[test]
fn a_wildcard_listener_blocks_the_mirror() {
    let held = TcpListener::bind("0.0.0.0:0").unwrap();
    assert!(!mirror_free(held.local_addr().unwrap().port()));
}

#[test]
fn a_released_port_is_free_again() {
    let port = {
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        held.local_addr().unwrap().port()
    };
    assert!(mirror_free(port));
}

#[test]
fn an_ipv6_loopback_listener_blocks_the_mirror() {
    let held = TcpListener::bind("[::1]:0").unwrap();
    assert!(!mirror_free(held.local_addr().unwrap().port()));
}

#[test]
fn time_wait_from_a_closed_connection_does_not_block_the_mirror() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let (server, _) = listener.accept().unwrap();
    drop(server);
    drop(listener);
    std::thread::sleep(std::time::Duration::from_millis(50));
    drop(client);
    assert!(mirror_free(port));
}

#[test]
fn the_alias_on_the_same_port_does_not_block_the_mirror() {
    let Ok(alias) = TcpListener::bind((ALIAS, 0)) else {
        return;
    };
    assert!(mirror_free(alias.local_addr().unwrap().port()));
}

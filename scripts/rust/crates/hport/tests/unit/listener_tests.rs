use super::*;

fn listener(port: u16, address: &str, process: &str) -> Listener {
    Listener {
        port,
        address: address.parse().unwrap(),
        process: process.into(),
        pid: 1,
    }
}

#[test]
fn loopback_and_wildcard_listeners_are_exported() {
    let found = exportable([
        listener(5173, "127.0.0.1", "bun"),
        listener(8080, "0.0.0.0", "java"),
        listener(3000, "::1", "node"),
        listener(4000, "::", "python"),
    ]);
    assert_eq!(
        found
            .iter()
            .map(|listener| listener.port)
            .collect::<Vec<_>>(),
        [3000, 4000, 5173, 8080]
    );
}

#[test]
fn a_listener_bound_to_one_interface_is_not_reachable_through_localhost() {
    assert!(exportable([listener(8443, "10.77.77.2", "wezterm-mux-server")]).is_empty());
}

#[test]
fn ssh_owned_listeners_are_never_exported_so_forwards_cannot_loop() {
    assert!(
        exportable([
            listener(5173, "127.0.0.2", "ssh"),
            listener(5173, "127.0.0.1", "ssh")
        ])
        .is_empty()
    );
}

#[test]
fn duplicates_collapse_and_the_result_is_sorted() {
    let found = exportable([
        listener(8080, "0.0.0.0", "java"),
        listener(5173, "127.0.0.1", "bun"),
        listener(8080, "0.0.0.0", "java"),
    ]);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].port, 5173);
}

#[test]
fn a_wildcard_service_is_reached_over_ipv4_loopback() {
    let found = services(&[listener(8080, "0.0.0.0", "java")]);
    assert_eq!(found[0].target, "127.0.0.1:8080".parse().unwrap());
}

#[test]
fn an_ipv6_only_service_is_reached_over_ipv6_loopback() {
    assert_eq!(
        services(&[listener(3000, "::", "node")])[0].target,
        "[::1]:3000".parse().unwrap()
    );
    assert_eq!(
        services(&[listener(3000, "::1", "node")])[0].target,
        "[::1]:3000".parse().unwrap()
    );
}

#[test]
fn ipv4_wins_when_a_port_listens_on_both_families() {
    let found = services(&[
        listener(5173, "::1", "bun"),
        listener(5173, "127.0.0.1", "bun"),
    ]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].target, "127.0.0.1:5173".parse().unwrap());
}

#[test]
fn services_are_grouped_by_port_even_from_an_unsorted_snapshot() {
    let found = services(&[
        listener(8080, "0.0.0.0", "java"),
        listener(5173, "127.0.0.1", "bun"),
        listener(8080, "::", "java"),
    ]);
    assert_eq!(
        found
            .iter()
            .map(|service| (service.port, service.process.as_str()))
            .collect::<Vec<_>>(),
        [(5173, "bun"), (8080, "java")]
    );
}

#[test]
fn the_json_shape_is_what_the_peer_parses() {
    let text = serde_json::to_string(&[listener(5173, "127.0.0.1", "bun")]).unwrap();
    assert_eq!(
        text,
        r#"[{"port":5173,"address":"127.0.0.1","process":"bun","pid":1}]"#
    );
    let parsed: Vec<Listener> = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed[0], listener(5173, "127.0.0.1", "bun"));
}

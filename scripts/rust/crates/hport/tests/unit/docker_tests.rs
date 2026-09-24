use super::*;

const PROXY_ARGS: [&str; 11] = [
    "/usr/bin/docker-proxy",
    "-proto",
    "tcp",
    "-host-ip",
    "0.0.0.0",
    "-host-port",
    "5432",
    "-container-ip",
    "172.17.0.2",
    "-container-port",
    "5432",
];

#[test]
fn a_tcp_proxy_publishes_its_host_address_and_port() {
    assert_eq!(
        published(&PROXY_ARGS),
        Some(("0.0.0.0".parse().unwrap(), 5432))
    );
}

#[test]
fn an_ipv6_host_address_is_kept() {
    let mut args = PROXY_ARGS;
    args[4] = "::";
    assert_eq!(published(&args), Some(("::".parse().unwrap(), 5432)));
}

#[test]
fn udp_proxies_are_not_listeners() {
    let mut args = PROXY_ARGS;
    args[2] = "udp";
    assert_eq!(published(&args), None);
}

#[test]
fn missing_or_malformed_flags_are_ignored() {
    assert_eq!(published(&PROXY_ARGS[..5]), None);
    let mut args = PROXY_ARGS;
    args[6] = "not-a-port";
    assert_eq!(published(&args), None);
    assert_eq!(published(&[]), None);
}

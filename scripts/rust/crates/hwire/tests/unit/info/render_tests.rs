use std::net::{IpAddr, Ipv4Addr};

use hostkit::Host;

use super::*;
use crate::info::model::{MasterInfo, Session};

fn snapshot() -> Snapshot {
    Snapshot {
        context: Context::Local,
        this: Host::Macie,
        peer: Host::Archie,
        session: None,
        preferred: Some(Route::Lan),
        routes: vec![
            RouteState {
                route: Route::Lan,
                local: Some(Ipv4Addr::new(192, 168, 1, 10)),
                peer: Some(Ipv4Addr::new(192, 168, 1, 20)),
                available: true,
                elapsed: Duration::from_millis(2),
                error: None,
            },
            RouteState {
                route: Route::Tailscale,
                local: Some(Ipv4Addr::new(100, 75, 71, 79)),
                peer: Some(Ipv4Addr::new(100, 126, 231, 24)),
                available: true,
                elapsed: Duration::from_millis(3),
                error: None,
            },
        ],
        targets: Vec::new(),
        warnings: Vec::new(),
    }
}

#[test]
fn local_routes_are_reversed_and_the_preferred_one_is_last() {
    assert_eq!(
        compact(&snapshot(), ColorMode::Never, true),
        "TAILSCALE | LAN"
    );
}

#[test]
fn remote_output_has_one_route_and_tls_overlay() {
    let mut snapshot = snapshot();
    snapshot.context = Context::Tls;
    snapshot.session = Some(Session {
        from: Host::Macie,
        to: Host::Archie,
        route: Some(Route::Cable),
        tls: true,
        client_address: None,
        client_port: None,
        server_address: Some(IpAddr::V4(Ipv4Addr::new(10, 77, 77, 2))),
        server_port: Some(8443),
        domain: Some("archie-cable".into()),
        evidence: "test",
    });
    let line = compact(&snapshot, ColorMode::Never, false);
    assert_eq!(line, "CABLE - TLS macie --> archie");
}

#[test]
fn redirected_output_has_no_padding_or_ansi() {
    let line = compact(&snapshot(), ColorMode::Auto, false);
    assert!(!line.contains("\x1b"));
    assert!(!line.ends_with(' '));
}

#[test]
fn remote_rows_fill_the_cap_and_compact_on_narrow_terminals() {
    let mut snapshot = snapshot();
    snapshot.context = Context::Ssh;
    snapshot.session = Some(Session {
        from: Host::Archie,
        to: Host::Macie,
        route: Some(Route::Tailscale),
        tls: false,
        client_address: None,
        client_port: None,
        server_address: None,
        server_port: None,
        domain: None,
        evidence: "test",
    });
    for width in [24, 40, 80] {
        let line = compact_at_width(&snapshot, ColorMode::Never, true, width);
        assert_eq!(line.chars().count(), width, "{line:?}");
        assert!(line.starts_with("TAILSCALE"), "{line:?}");
    }
}

#[test]
fn json_carries_master_diagnostics() {
    let mut snapshot = snapshot();
    snapshot.context = Context::Query;
    snapshot.targets.push(TargetInfo {
        input: "archie".into(),
        hostname: "10.77.77.2".into(),
        route: Some(Route::Cable),
        bound: Some("10.77.77.1".into()),
        proxy: None,
        user: Some("fredrir".into()),
        port: Some(22),
        master: MasterInfo {
            running: true,
            control_path: Some("/tmp/master".into()),
            age: Some(Duration::from_secs(4)),
            detail: None,
        },
        error: None,
    });
    let document = json_document(&snapshot);
    assert_eq!(document["targets"][0]["master"]["running"], true);
    assert_eq!(document["targets"][0]["master"]["age_seconds"], 4.0);
}

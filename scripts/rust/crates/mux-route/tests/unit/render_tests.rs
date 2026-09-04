use super::*;
use hostkit::{MUX_PORT, Route};
use std::time::Duration;

fn answer(route: Route, up: bool) -> Answer {
    Answer {
        route,
        local_address: Some(Host::Macie.address(route).unwrap()),
        peer_address: Some(Host::Archie.address(route).unwrap()),
        port: MUX_PORT,
        up,
        elapsed: Duration::ZERO,
        error: (!up).then(|| "refused".into()),
    }
}

#[test]
fn a_route_that_answered_carries_the_domain_to_attach() {
    let printed = list(
        &Style::plain(),
        Host::Archie,
        &[answer(Route::Cable, true), answer(Route::Wifi, false)],
    );
    let lines: Vec<&str> = printed.lines().collect();
    assert!(lines[0].starts_with("up    cable"), "{printed}");
    assert!(lines[0].ends_with("archie-cable"), "{printed}");
    assert!(lines[1].starts_with("down  wifi"), "{printed}");
    assert!(!lines[1].contains("archie"), "{printed}");
    assert!(!lines[1].ends_with(' '), "{printed}");
}

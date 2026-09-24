use super::*;
use std::net::SocketAddr;

fn service(port: u16, target: &str) -> Service {
    Service {
        port,
        process: "bun".into(),
        target: target.parse::<SocketAddr>().unwrap(),
    }
}

fn forward(bind: Bind, service: &Service) -> Forward {
    Forward {
        bind,
        port: service.port,
        target: service.target,
    }
}

fn held(service: &Service) -> BTreeSet<Forward> {
    [Bind::Alias, Bind::Loopback4, Bind::Loopback6]
        .into_iter()
        .map(|bind| forward(bind, service))
        .collect()
}

#[test]
fn a_new_service_gets_the_alias_and_the_mirror() {
    let bun = service(5173, "127.0.0.1:5173");
    let actions = plan(
        std::slice::from_ref(&bun),
        &BTreeSet::new(),
        |_, _| true,
        |_| true,
    );
    assert_eq!(
        actions,
        [
            Action::Alias(forward(Bind::Alias, &bun)),
            Action::Mirror([
                forward(Bind::Loopback4, &bun),
                forward(Bind::Loopback6, &bun)
            ]),
        ]
    );
}

#[test]
fn a_service_already_forwarded_needs_nothing() {
    let bun = service(5173, "127.0.0.1:5173");
    assert!(
        plan(
            std::slice::from_ref(&bun),
            &held(&bun),
            |_, _| true,
            |_| false
        )
        .is_empty()
    );
}

#[test]
fn a_busy_localhost_port_still_gets_the_alias() {
    let bun = service(5173, "127.0.0.1:5173");
    let actions = plan(
        std::slice::from_ref(&bun),
        &BTreeSet::new(),
        |_, _| true,
        |_| false,
    );
    assert_eq!(actions, [Action::Alias(forward(Bind::Alias, &bun))]);
}

#[test]
fn a_service_that_stopped_is_cancelled_everywhere() {
    let bun = service(5173, "127.0.0.1:5173");
    let actions = plan(&[], &held(&bun), |_, _| true, |_| true);
    assert_eq!(actions.len(), 3);
    assert!(
        actions
            .iter()
            .all(|action| matches!(action, Action::Cancel(_)))
    );
}

#[test]
fn a_service_that_moved_to_ipv6_is_reforwarded() {
    let before = service(3000, "127.0.0.1:3000");
    let after = service(3000, "[::1]:3000");
    let actions = plan(
        std::slice::from_ref(&after),
        &held(&before),
        |_, _| true,
        |_| true,
    );
    assert_eq!(
        actions
            .iter()
            .filter(|action| matches!(action, Action::Cancel(_)))
            .count(),
        3
    );
    assert!(actions.contains(&Action::Alias(forward(Bind::Alias, &after))));
}

#[test]
fn half_a_mirror_is_torn_down() {
    let bun = service(5173, "127.0.0.1:5173");
    let active = [forward(Bind::Alias, &bun), forward(Bind::Loopback4, &bun)]
        .into_iter()
        .collect();
    assert_eq!(
        plan(std::slice::from_ref(&bun), &active, |_, _| true, |_| true),
        [Action::Cancel(forward(Bind::Loopback4, &bun))]
    );
}

#[test]
fn a_recent_failure_is_not_retried() {
    let bun = service(5173, "127.0.0.1:5173");
    assert!(
        plan(
            std::slice::from_ref(&bun),
            &BTreeSet::new(),
            |_, _| false,
            |_| true
        )
        .is_empty()
    );
    let only_mirror = plan(
        &[bun],
        &BTreeSet::new(),
        |kind, _| kind == Kind::Mirror,
        |_| true,
    );
    assert!(matches!(only_mirror.as_slice(), [Action::Mirror(_)]));
}

#[test]
fn the_localhost_probe_runs_only_when_a_mirror_is_wanted() {
    let bun = service(5173, "127.0.0.1:5173");
    let probed = std::cell::Cell::new(0);
    plan(
        std::slice::from_ref(&bun),
        &held(&bun),
        |_, _| true,
        |_| {
            probed.set(probed.get() + 1);
            true
        },
    );
    assert_eq!(probed.get(), 0);
}

#[test]
fn every_bind_belongs_to_one_kind() {
    assert_eq!(Kind::from(Bind::Alias), Kind::Alias);
    assert_eq!(Kind::from(Bind::Loopback4), Kind::Mirror);
    assert_eq!(Kind::from(Bind::Loopback6), Kind::Mirror);
}

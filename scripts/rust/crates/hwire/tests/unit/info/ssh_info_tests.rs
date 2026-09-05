use hostkit::Route;

use super::*;

#[test]
fn a_resolved_config_becomes_a_target_that_has_not_been_checked_for_a_master() {
    let info = describe(
        "archie",
        Resolved {
            hostname: "10.77.77.2".into(),
            port: Some(22),
            user: Some("fredrir".into()),
            proxy: None,
            bound: Some("10.77.77.1".into()),
            control_path: Some("/tmp/master".into()),
            route: Route::Cable,
        },
    );
    assert_eq!(info.input, "archie");
    assert_eq!(info.hostname, "10.77.77.2");
    assert_eq!(info.route, Some(Route::Cable));
    assert_eq!(info.bound.as_deref(), Some("10.77.77.1"));
    assert_eq!(info.master.control_path.as_deref(), Some("/tmp/master"));
    assert!(!info.master.running);
    assert_eq!(info.error, None);
}

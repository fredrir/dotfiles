use super::*;

#[test]
fn the_stamp_and_the_cached_route_go_first_without_repeats() {
    let (first, rest) = waves(&[Route::Tailscale, Route::Tailscale]);
    assert_eq!(first, vec![Route::Tailscale]);
    assert_eq!(rest, vec![Route::Cable, Route::Wifi, Route::Lan]);
}

#[test]
fn every_route_is_tried_exactly_once() {
    let (first, rest) = waves(&[Route::Lan, Route::Cable]);
    assert_eq!(first, vec![Route::Lan, Route::Cable]);
    let mut all: Vec<_> = first.into_iter().chain(rest).map(Route::name).collect();
    all.sort_unstable();
    assert_eq!(all, ["cable", "lan", "tailscale", "wifi"]);
}

#[test]
fn the_race_fits_the_budget() {
    assert!(STAGGER < BUDGET);
}

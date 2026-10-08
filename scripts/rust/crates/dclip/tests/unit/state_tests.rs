use super::*;

#[test]
fn a_saved_route_loads_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dclip/route");
    save(&path, Route::Tailscale).unwrap();
    assert_eq!(load(&path), Some(Route::Tailscale));
    save(&path, Route::Cable).unwrap();
    assert_eq!(load(&path), Some(Route::Cable));
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn a_missing_or_foreign_cache_is_no_preference() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("route");
    assert_eq!(load(&path), None);
    fs::write(&path, "carrier pigeon\n").unwrap();
    assert_eq!(load(&path), None);
}

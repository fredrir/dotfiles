use super::*;
use serde_json::json;

fn write_palette(path: &Path, profile: &str, color: &str) {
    std::fs::write(
        path,
        json!({
            "version": 1, "profile": profile, "colors": {"fg": color}, "roles": {}
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn missing_theme_uses_terminal_defaults_and_recovers_when_created() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("theme.json");
    let mut theme = ThemeHandle::from_path(&path);
    assert_eq!(theme.source(), &ThemeSource::Fallback);
    assert_eq!(theme.palette().profile, "terminal");
    write_palette(&path, "created", "#112233");
    assert!(theme.poll_at(Instant::now() + Duration::from_secs(2)));
    assert_eq!(theme.palette().profile, "created");
    assert_eq!(theme.source(), &ThemeSource::File(path));
}

#[test]
fn theme_changes_are_bounded_and_previous_session_snapshots_stay_immutable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("theme.json");
    write_palette(&path, "first", "#112233");
    let mut theme = ThemeHandle::from_path(&path);
    let initial = Arc::clone(theme.palette());
    let now = Instant::now();
    write_palette(&path, "second-profile", "#445566");
    assert!(!theme.poll_at(now));
    assert_eq!(theme.palette().profile, "first");
    assert!(theme.poll_at(now + Duration::from_secs(2)));
    assert_eq!(theme.palette().profile, "second-profile");
    assert_eq!(initial.profile, "first");
    assert!(!theme.poll_at(now + Duration::from_secs(4)));
}

#[test]
fn malformed_reloads_preserve_last_good_theme_and_report_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("theme.json");
    write_palette(&path, "valid", "#112233");
    let mut theme = ThemeHandle::from_path(&path);
    std::fs::write(&path, "broken").unwrap();
    let now = Instant::now();
    assert!(!theme.poll_at(now + Duration::from_secs(2)));
    assert_eq!(theme.palette().profile, "valid");
    assert!(theme.error().unwrap().contains("theme.json"));
    write_palette(&path, "recovered", "#445566");
    assert!(theme.poll_at(now + Duration::from_secs(4)));
    assert_eq!(theme.palette().profile, "recovered");
    assert!(theme.error().is_none());
}

#[test]
fn explicit_file_order_selects_the_first_existing_valid_palette() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.json");
    let second = directory.path().join("second.json");
    write_palette(&second, "installed", "#112233");
    let mut theme = ThemeHandle::from_paths(vec![first.clone(), second]);
    assert_eq!(theme.palette().profile, "installed");
    write_palette(&first, "preferred", "#445566");
    assert!(theme.poll_at(Instant::now() + Duration::from_secs(2)));
    assert_eq!(theme.palette().profile, "preferred");
}

#[test]
fn oversized_palette_is_rejected_before_parsing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("theme.json");
    std::fs::write(&path, vec![b' '; 256 * 1024 + 1]).unwrap();
    assert!(Palette::from_path(&path).unwrap_err().contains("exceeds"));
}

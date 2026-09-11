use super::*;
use crate::ColorDepth;
use serde_json::json;

#[test]
fn plain_output_never_contains_styling_sequences() {
    let style = Style::plain();
    for role in Role::ALL {
        assert_eq!(style.paint(role, "value"), "value");
    }
    assert_eq!(style.bold("value"), "value");
    assert_eq!(style.code("31", "value"), "value");
}

#[test]
fn themed_output_uses_semantic_foreground_and_selection_background() {
    let palette = Palette::from_value(&json!({
        "version":1,"profile":"test","colors":{"fg":"#ffffff"},"roles":{},
        "ui":{"danger":"#aa1122","selection_foreground":"#ffffff","selection_background":"#123456"}
    }))
    .unwrap()
    .with_depth(ColorDepth::TrueColor);
    let style = Style::from_palette(Arc::new(palette), ColorMode::Always, false);
    assert_eq!(style.red("failure"), "\x1b[38;2;170;17;34mfailure\x1b[0m");
    let selection = style.paint(Role::Selection, "selected");
    assert!(selection.contains("38;2;255;255;255"));
    assert!(selection.contains("48;2;18;52;86"));
    assert_eq!(style.paint(Role::Selection, ""), "");
}

#[test]
fn live_styles_reload_once_and_keep_color_policy() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("theme.json");
    let write = |profile: &str| {
        std::fs::write(
            &path,
            json!({
                "version":1,"profile":profile,"colors":{"fg":"#ffffff"},"roles":{}
            })
            .to_string(),
        )
        .unwrap()
    };
    write("first");
    let mut style = LiveStyle::from_path(&Style::plain(), &path);
    write("second-profile");
    assert!(!style.poll());
    assert!(style.poll_at(Instant::now() + std::time::Duration::from_secs(2)));
    assert_eq!(style.style().palette().profile, "second-profile");
    assert_eq!(style.style().paint(Role::Strong, "text"), "text");
}

#[test]
fn explicit_preview_styles_do_not_follow_runtime_theme() {
    let style = Style::from_palette(Arc::new(Palette::default()), ColorMode::Always, true);
    let mut live = LiveStyle::new(&style);
    assert!(!live.poll_at(Instant::now() + std::time::Duration::from_secs(2)));
    assert_eq!(live.style().palette(), style.palette());
}

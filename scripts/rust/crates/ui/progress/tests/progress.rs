use ui_progress::{Progress, Spinner};

#[test]
fn unknown_progress_has_no_percentage_and_overruns_are_clamped() {
    assert_eq!(
        Progress {
            completed: 7,
            total: None
        }
        .fraction(),
        None
    );
    assert_eq!(
        Progress {
            completed: 12,
            total: Some(10)
        }
        .fraction(),
        Some(1.0)
    );
    assert_eq!(
        Progress {
            completed: 0,
            total: Some(0)
        }
        .fraction(),
        Some(0.0)
    );
}

#[test]
fn reduced_motion_has_a_stable_indicator() {
    assert_eq!(
        Spinner::Braille.frame(0, false),
        Spinner::Braille.frame(8, false)
    );
    assert_ne!(
        Spinner::Braille.frame(0, true),
        Spinner::Braille.frame(8, true)
    );
}

#[cfg(feature = "ratatui")]
#[test]
fn filled_labels_use_the_configured_canvas_and_no_color_stays_unpainted() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};
    use ui_theme::{Palette, Role};

    let area = Rect::new(0, 0, 40, 1);
    let palette = Palette::from_json(
        r##"{"version":1,"profile":"fixture","dark":true,"colors":{"fg":"#eeeeee"},"ui":{"background":"#112233","accent":"#ddddff"}}"##,
    )
    .unwrap();
    for color in [true, false] {
        let mut buffer = Buffer::empty(area);
        ui_progress::ProgressBar {
            progress: Progress {
                completed: 10,
                total: Some(10),
            },
            frame: 0,
            palette: &palette,
            color,
        }
        .render(area, &mut buffer);
        let label = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == "%")
            .expect("percentage label");
        if color {
            assert_eq!(label.fg, palette.background(Role::Background).ratatui());
            assert_eq!(label.bg, palette.foreground(Role::Accent).ratatui());
            assert_ne!(label.fg, Color::Reset);
        } else {
            assert!(
                buffer
                    .content()
                    .iter()
                    .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
            );
        }
    }
}

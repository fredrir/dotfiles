use super::*;

#[test]
fn pretty_output_respects_16_256_and_truecolor_terminals() {
    let view = crate::presentation::build_view(&crate::model::Snapshot::default());
    let colors = Colors::from_theme(&ui_theme::Palette::default()).unwrap();
    for depth in [
        ui_theme::ColorDepth::Ansi16,
        ui_theme::ColorDepth::Ansi256,
        ui_theme::ColorDepth::TrueColor,
    ] {
        let render = |colored| {
            render_with_depth(
                &view,
                &[],
                RenderOptions {
                    full: true,
                    health: false,
                },
                PrettyContext {
                    colors: &colors,
                    width: 80,
                    username: "tester",
                    hostname: "example",
                    colored,
                },
                depth,
            )
        };
        let output = render(true);
        assert!(output.contains("TESTER"));
        match depth {
            ui_theme::ColorDepth::Ansi16 => {
                assert!(!output.contains("\x1b[38;"));
                assert!(!output.contains("\x1b[48;"));
                assert!(output.contains("\x1b[37m"));
            }
            ui_theme::ColorDepth::Ansi256 => {
                assert!(!output.contains("\x1b[38;2;"));
                assert!(output.contains("\x1b[38;5;"));
            }
            ui_theme::ColorDepth::TrueColor => assert!(output.contains("\x1b[38;2;")),
        }
        assert!(!render(false).contains('\x1b'));
    }
}

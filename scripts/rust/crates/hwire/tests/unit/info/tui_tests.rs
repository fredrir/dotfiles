use std::net::Ipv4Addr;

use hostkit::{Host, Route};
use ratatui::backend::TestBackend;

use super::*;
use crate::info::model::{Context, RouteState};

fn snapshot() -> Snapshot {
    Snapshot {
        context: Context::Local,
        this: Host::Macie,
        peer: Host::Archie,
        session: None,
        preferred: Some(Route::Cable),
        routes: vec![RouteState {
            route: Route::Cable,
            local: Some(Ipv4Addr::new(10, 77, 77, 1)),
            peer: Some(Ipv4Addr::new(10, 77, 77, 2)),
            available: true,
            elapsed: Duration::from_millis(2),
            error: None,
        }],
        targets: Vec::new(),
        warnings: Vec::new(),
    }
}

#[test]
fn test_backend_renders_the_verbose_snapshot_and_deterministic_effect_ticks() {
    let backend = TestBackend::new(72, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    let snapshot = snapshot();
    let palette = Palette::default();
    let mut effect = reveal_effect(&palette, true);
    for tick in [90, 90] {
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(Paragraph::new(styled_text(&snapshot, &palette, true)), area);
                frame.render_effect(&mut effect, area, tachyonfx::Duration::from_millis(tick));
            })
            .unwrap();
    }
    assert!(effect.done());
    let symbols = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(symbols.contains("hwire info"), "{symbols:?}");
    assert!(symbols.contains("routes"), "{symbols:?}");
    assert!(symbols.contains("CABLE"), "{symbols:?}");
}

#[test]
fn long_diagnostics_are_scrollable_within_the_inline_viewport() {
    let mut snapshot = snapshot();
    snapshot.warnings = (0..30).map(|index| format!("warning {index}")).collect();
    let paragraph = Paragraph::new(styled_text(&snapshot, &Palette::default(), false))
        .wrap(Wrap { trim: false });
    let height = measured_height(&paragraph, 78);
    assert!(scroll_limit(height, 10) > 0);
    assert_eq!(scroll_limit(height, 80), 0);
}

#[test]
fn wrapped_wide_diagnostics_remain_reachable_at_narrow_widths() {
    let paragraph = Paragraph::new("界界界界界界\ntail").wrap(Wrap { trim: false });
    assert_eq!(measured_height(&paragraph, 4), 4);
    assert_eq!(scroll_limit(measured_height(&paragraph, 4), 4), 2);
}

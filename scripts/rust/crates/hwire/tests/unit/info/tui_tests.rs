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
    let mut effect = reveal_effect(true);
    for tick in [90, 90] {
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(Paragraph::new(styled_text(&snapshot, true)), area);
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
    assert!(scroll_limit(Some(&snapshot), 10) > 0);
    assert_eq!(scroll_limit(Some(&snapshot), 80), 0);
}

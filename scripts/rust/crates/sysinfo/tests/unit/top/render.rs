use super::*;
use crate::top::SCHEMA;
use std::path::Path;
use std::sync::Arc;
use ui_theme::{ColorMode, Palette};

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

fn row(pid: u32, user: &str, command: &str, count: usize) -> Row {
    Row {
        pid,
        user: user.into(),
        command: command.into(),
        count,
        ..Row::default()
    }
}

fn report(rows: Vec<Row>) -> Report {
    Report {
        schema: SCHEMA,
        host: "macie".into(),
        cores: 15,
        memory: 24 * GIB,
        gpu: true,
        rows,
    }
}

fn fixture() -> Report {
    report(vec![
        Row {
            cpu: 1.4,
            cores: 0.21,
            memory: GIB * 16 / 10,
            memory_share: 6.7,
            gpu: Some(8.2),
            age: 5_303,
            ..row(2804, "fredrir", "Zen", 3)
        },
        Row {
            cpu: 6.6,
            cores: 0.99,
            memory: 376 * MIB,
            memory_share: 1.5,
            age: 2_040,
            ..row(51080, "fredrir", "shucked-server", 1)
        },
        Row {
            cpu: 2.0,
            cores: 0.3,
            memory: 93 * MIB,
            memory_share: 0.4,
            gpu: Some(3.1),
            age: 5_303,
            ..row(431, "_windowserver", "WindowServer", 1)
        },
    ])
}

#[test]
fn values_stay_short() {
    assert_eq!(percent(0.0), "0.0%");
    assert_eq!(percent(6.64), "6.6%");
    assert_eq!(percent(99.96), "100%");
    assert_eq!(cores(0.04), "0.0c");
    assert_eq!(cores(1.0), "1.0c");
    assert_eq!(cores(12.3), "12c");
    assert_eq!(size(0), "0K");
    assert_eq!(size(512 * 1024), "512K");
    assert_eq!(size(376 * MIB), "376M");
    assert_eq!(size(1000 * MIB), "1.0G");
    assert_eq!(size(GIB * 16 / 10), "1.6G");
    assert_eq!(size(24 * GIB), "24G");
    assert_eq!(size(2048 * GIB), "2.0T");
}

#[test]
fn ages_use_the_largest_whole_unit() {
    for (seconds, expected) in [
        (45, "45s"),
        (1_800, "30m"),
        (7_200, "2h"),
        (3 * 86_400, "3d"),
        (15 * 86_400, "2w"),
        (400 * 86_400, "1y"),
    ] {
        assert_eq!(age(seconds), expected);
    }
}

#[test]
fn truncation_counts_terminal_cells() {
    assert_eq!(truncate("claude", 10), "claude");
    assert_eq!(truncate("hport listeners --watch", 10), "hport lis…");
    assert_eq!(truncate("日本語テキスト", 7), "日本語…");
}

#[test]
fn table_aligns_every_column() {
    let text = render(&fixture(), Sort::Total, &Style::plain(), None);
    assert_eq!(
        text,
        "USER             PID   CPU        MEM        GPU  TIME  COMMAND\n\
         fredrir         2804  1.4% 0.2c  6.7% 1.6G  8.2%    1h  Zen ×3\n\
         fredrir        51080  6.6% 1.0c  1.5% 376M     —   34m  shucked-server\n\
         _windowserver    431  2.0% 0.3c  0.4%  93M  3.1%    1h  WindowServer\n"
    );
}

#[test]
fn narrow_terminals_cut_the_command_but_keep_the_count() {
    let wide = report(vec![row(1, "fredrir", "Visual Studio Code", 18)]);
    let text = render(&wide, Sort::Total, &Style::plain(), Some(40));
    assert!(text.ends_with("  Visual … ×18\n"), "{text}");
    let text = render(&fixture(), Sort::Total, &Style::plain(), Some(60));
    assert!(text.contains("  shucked-ser…\n"), "{text}");
}

#[test]
fn rows_emphasize_the_share_they_rank_by() {
    let rows = fixture().rows;
    assert_eq!(emphasis(&rows[0], Sort::Total), Some(Metric::Gpu));
    assert_eq!(emphasis(&rows[1], Sort::Total), Some(Metric::Cpu));
    assert_eq!(emphasis(&rows[1], Sort::Memory), Some(Metric::Memory));
    assert_eq!(emphasis(&Row::default(), Sort::Total), None);
}

#[test]
fn shares_cores_and_sizes_each_get_their_own_color() {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../shared/ui/theme.json");
    let palette = Arc::new(Palette::from_path(&theme).unwrap());
    let style = Style::from_palette(palette, ColorMode::Always, true);
    let text = render(&fixture(), Sort::Cpu, &style, None);
    for (role, value) in [(SHARE, "6.6%"), (CORES, "1.0c"), (SIZE, "376M")] {
        assert!(
            text.contains(&style.paint(role, value)),
            "{value}: {text:?}"
        );
    }
    let colors = [SHARE, CORES, SIZE].map(|role| style.paint(role, "x"));
    assert!(colors[0] != colors[1] && colors[1] != colors[2] && colors[0] != colors[2]);
    assert!(
        text.contains(&style.bold(&style.paint(SHARE, "6.6%"))),
        "{text:?}"
    );
}

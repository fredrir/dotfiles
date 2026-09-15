#![forbid(unsafe_code)]

use dotfile_cli::doctor::report::{Manager, Package, Report, Row, Status, render};
use workstation::Style;

fn package(manager: Option<Manager>, name: &str, optional: bool) -> Package {
    Package {
        manager,
        name: name.into(),
        optional,
    }
}

fn plain(rows: &[Row], show_all: bool) -> String {
    render(
        &Report {
            profile: "arch-linux/kde",
            rows,
            show_all,
        },
        &Style::plain(),
    )
}

fn rows_with_optional() -> Vec<Row> {
    let mut tools = Row::missing(
        "tools",
        4,
        vec![
            package(Some(Manager::Pacman), "fd", false),
            package(Some(Manager::Aur), "fan2go-git", false),
            package(Some(Manager::Pacman), "bat", false),
            package(None, "mystery", false),
        ],
    );
    tools.details.push(("~/.config/hypr/wallpaper.png".into(), String::new()));
    let mut optional = Row::new(Status::Note, "optional", "1 not installed", 0);
    optional
        .packages
        .push(package(Some(Manager::Cargo), "kondo", true));
    vec![tools, optional]
}

#[test]
fn install_block_groups_packages_by_manager_and_skips_empty_managers() {
    let output = plain(&rows_with_optional(), false);
    assert!(output.starts_with("\nIssues:\n"), "{output}");
    assert!(
        output.contains("\nPackages:\nbat\nfan2go-git\nfd\nmystery\n"),
        "{output}"
    );
    assert!(
        output.contains(
            "\nInstall:\nsudo pacman -S --needed \\\n  bat \\\n  fd\n\nyay -S --needed \\\n  fan2go-git\n"
        ),
        "{output}"
    );
    assert!(!output.contains("cargo install"), "{output}");
    assert!(!output.contains("kondo"), "{output}");
    assert!(!output.contains("of 4 missing"), "{output}");
}

#[test]
fn show_all_includes_optional_packages() {
    let output = plain(&rows_with_optional(), true);
    assert!(output.contains("\nkondo\n"), "{output}");
    assert!(
        output.contains("cargo install --locked \\\n  kondo\n"),
        "{output}"
    );
}

#[test]
fn package_row_details_without_a_package_are_issues() {
    let output = plain(&rows_with_optional(), false);
    assert!(
        output.contains("\nIssues:\ntools  ~/.config/hypr/wallpaper.png\n"),
        "{output}"
    );
}

#[test]
fn healthy_report_says_nothing_missing() {
    let rows = [
        Row::missing("tools", 4, Vec::new()),
        Row::new(Status::Ok, "links", "63 linked", 0),
        Row::new(Status::Note, "optional", "2 not installed", 0),
    ];
    let output = plain(&rows, false);
    assert_eq!(output, "nothing missing\n\n");
}

#[test]
fn issues_align_columns_and_truncate_without_all() {
    let mut commands = Row::new(Status::Warn, "commands", "13 need attention", 1);
    commands
        .details
        .push(("a".into(), "shadowed on PATH".into()));
    commands
        .details
        .push(("longer-name".into(), "not installed by uv".into()));
    for index in 0..11 {
        commands
            .details
            .push((format!("cmd{index}"), "missing".into()));
    }
    let rows = [
        commands,
        Row::new(Status::Bad, "oh-my-zsh", "not installed at ~/.oh-my-zsh", 1),
    ];
    let output = plain(&rows, false);
    assert!(
        output.contains("\nIssues:\ncommands   a             shadowed on PATH\n"),
        "{output}"
    );
    assert!(
        output.contains("commands   longer-name   not installed by uv\n"),
        "{output}"
    );
    assert!(
        output.contains("commands   … and 1 more  --all to list\n"),
        "{output}"
    );
    assert!(
        output.contains("oh-my-zsh  not installed at ~/.oh-my-zsh\n"),
        "{output}"
    );
    assert!(!output.contains("Packages:"), "{output}");
    let output = plain(&rows, true);
    assert!(!output.contains("more"), "{output}");
    assert!(output.contains("commands   cmd10        missing\n"), "{output}");
}

use super::*;
use std::time::Duration;

const ZEN: &str = "/Applications/Zen.app/Contents/MacOS";

fn process(pid: u32, parent: Option<u32>, uid: u32, exe: &str, command: &[&str]) -> Process {
    Process {
        pid,
        parent,
        uid: Some(uid),
        name: exe.rsplit('/').next().unwrap_or(exe).into(),
        exe: (!exe.is_empty()).then(|| exe.into()),
        command: command.iter().map(|part| part.to_string()).collect(),
        age: 10_000 - u64::from(pid),
        ..Process::default()
    }
}

fn sample(processes: Vec<Process>) -> Sample {
    Sample {
        host: "macie".into(),
        cores: 4,
        memory: 4 << 30,
        gpu: true,
        window: Duration::from_millis(200),
        processes,
    }
}

fn commands(rows: &[Row]) -> Vec<String> {
    let mut commands: Vec<_> = rows.iter().map(|row| row.command.clone()).collect();
    commands.sort();
    commands
}

#[test]
fn bundles_resolve_to_the_outermost_app() {
    assert_eq!(
        bundle(&format!(
            "{ZEN}/gpu-helper.app/Contents/MacOS/Zen GPU Helper"
        )),
        Some("/Applications/Zen.app")
    );
    assert_eq!(bundle("/usr/bin/zsh"), None);
    assert_eq!(bundle("/opt/web.application/bin/web"), None);
}

#[test]
fn apps_fold_helpers_and_same_app_siblings_but_not_other_programs() {
    let processes = vec![
        process(1, None, 0, "/sbin/launchd", &["/sbin/launchd"]),
        process(2804, Some(1), 501, &format!("{ZEN}/zen"), &[]),
        process(
            2838,
            Some(2804),
            501,
            &format!("{ZEN}/gpu-helper.app/Contents/MacOS/Zen GPU Helper"),
            &[],
        ),
        process(
            5348,
            Some(2804),
            501,
            &format!("{ZEN}/plugin-container.app/Contents/MacOS/plugin-container"),
            &[],
        ),
        process(
            4805,
            Some(1369),
            501,
            "/Users/me/.local/bin/claude",
            &["claude", "--resume"],
        ),
        process(1687, Some(4805), 501, "/Users/me/go/bin/gopls", &["gopls"]),
        process(
            1699,
            Some(1),
            501,
            "/Users/me/dotfiles/.bin/hport",
            &["hport", "listeners"],
        ),
        process(
            1572,
            Some(1),
            501,
            "/Users/me/dotfiles/.bin/hport",
            &["hport", "daemon"],
        ),
        process(
            700,
            Some(1),
            0,
            "/Users/me/dotfiles/.bin/hport",
            &["hport", "daemon"],
        ),
    ];
    let rows = rows(&sample(processes), false);
    assert_eq!(
        commands(&rows),
        [
            "Zen",
            "claude --resume",
            "gopls",
            "hport",
            "hport daemon",
            "launchd"
        ]
    );
    let zen = rows.iter().find(|row| row.command == "Zen").unwrap();
    assert_eq!((zen.pid, zen.count), (2804, 3));
    let hport = rows.iter().find(|row| row.command == "hport").unwrap();
    assert_eq!(
        (hport.pid, hport.count),
        (1572, 2),
        "the oldest sibling represents the group"
    );
}

#[test]
fn split_keeps_one_row_per_process() {
    let processes = vec![
        process(10, None, 501, &format!("{ZEN}/zen"), &[]),
        process(11, Some(10), 501, &format!("{ZEN}/zen"), &[]),
    ];
    let rows = rows(&sample(processes), true);
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.count == 1 && row.command == "zen")
    );
}

#[test]
fn parent_cycles_from_reused_pids_terminate() {
    let processes = vec![
        process(20, Some(21), 501, "/bin/zsh", &["zsh"]),
        process(21, Some(20), 501, "/bin/zsh", &["zsh"]),
    ];
    let rows = rows(&sample(processes), false);
    assert_eq!(rows.iter().map(|row| row.count).sum::<usize>(), 2);
}

#[test]
fn shares_are_fractions_of_the_whole_machine() {
    let mut app = process(30, None, 501, "/usr/bin/app", &["app"]);
    app.cpu_ms = 300.0;
    app.memory = 1 << 30;
    app.gpu = Some(70.0);
    let mut helper = process(31, Some(30), 501, "/usr/bin/app", &["app"]);
    helper.cpu_ms = 100.0;
    helper.gpu = Some(40.0);
    let rows = rows(&sample(vec![app, helper]), false);
    let row = &rows[0];
    assert_eq!(row.cores, 2.0);
    assert_eq!(row.cpu, 50.0);
    assert_eq!(row.memory_share, 25.0);
    assert_eq!(row.gpu, Some(100.0), "summed GPU shares cap at every GPU");

    let idle = rows_of(process(32, None, 501, "/usr/bin/idle", &["idle"]));
    assert_eq!(idle.gpu, None);
}

fn rows_of(process: Process) -> Row {
    rows(&sample(vec![process]), false).remove(0)
}

#[test]
fn command_lines_drop_paths_but_keep_arguments() {
    let line = |exe: &str, command: &[&str]| command_line(&process(1, None, 0, exe, command));
    assert_eq!(
        line(
            "/opt/google/chrome/chrome",
            &["/opt/google/chrome/chrome --type=renderer --lang=en"]
        ),
        "chrome --type=renderer --lang=en"
    );
    assert_eq!(
        line(
            "/Users/me/.local/bin/claude",
            &["claude", "--dangerously-skip-permissions"]
        ),
        "claude --dangerously-skip-permissions"
    );
    assert_eq!(line("", &["sshd: fredrir@pts/0"]), "sshd: fredrir@pts/0");
    assert_eq!(
        line("/repo/target/debug/foo", &["./target/debug/foo", "-x"]),
        "foo -x"
    );
    let helper = format!("{ZEN}/gpu-helper.app/Contents/MacOS/Zen GPU Helper");
    assert_eq!(
        line(&helper, &[&helper, "-parentPid", "2804"]),
        "Zen GPU Helper -parentPid 2804"
    );
    let mut kernel = process(2, None, 0, "", &[]);
    kernel.name = "kworker/0:1".into();
    kernel.kernel = true;
    assert_eq!(command_line(&kernel), "[kworker/0:1]");
}

#[test]
fn ranking_follows_the_chosen_resource_then_the_largest_share() {
    let row = |pid, cpu, memory_share, gpu| Row {
        pid,
        cpu,
        memory_share,
        gpu,
        ..Row::default()
    };
    let mut rows = vec![
        row(1, 5.0, 1.0, None),
        row(2, 1.0, 9.0, None),
        row(3, 2.0, 1.0, Some(20.0)),
        row(4, 1.0, 9.0, None),
    ];
    let order = |rows: &[Row]| rows.iter().map(|row| row.pid).collect::<Vec<_>>();
    rank(&mut rows, Sort::Total);
    assert_eq!(order(&rows), [3, 2, 4, 1]);
    rank(&mut rows, Sort::Cpu);
    assert_eq!(order(&rows), [1, 3, 2, 4]);
    rank(&mut rows, Sort::Memory);
    assert_eq!(
        order(&rows),
        [2, 4, 3, 1],
        "equal memory falls back to the largest share"
    );
    rank(&mut rows, Sort::Gpu);
    assert_eq!(order(&rows), [3, 2, 4, 1]);
}

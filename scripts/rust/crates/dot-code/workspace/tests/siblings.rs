#![forbid(unsafe_code)]

use std::fs::File;
use std::time::{Duration, SystemTime};

use dot_code_workspace::siblings::{Explorer, Params, sibling};
use serde_json::{Map, json};
use testkit::{TempDir, tree};

fn params(root: &TempDir, path: &str) -> Params {
    Params {
        path: root.path().join(path),
        offset: 1,
        root: Some(root.path().to_path_buf()),
        locale: "en".into(),
        explorer: Explorer::default(),
        exclude: Map::new(),
    }
}

fn step(params: &Params, offset: isize) -> Option<String> {
    let params = Params {
        offset,
        ..params.clone()
    };
    sibling(&params)
        .expect("directory is readable")
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
}

/// Every file reached by stepping forward from `params.path` until it wraps around.
fn order(params: &Params) -> Vec<String> {
    let start = params
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut seen = vec![start.clone()];
    let mut current = params.clone();
    while let Some(next) = step(&current, 1) {
        if next == start {
            break;
        }
        current.path = current.path.with_file_name(&next);
        seen.push(next);
    }
    seen
}

fn explorer(sort_order: &str, lexicographic: &str, reverse: bool) -> Explorer {
    Explorer {
        sort_order: sort_order.into(),
        sort_order_lexicographic_options: lexicographic.into(),
        sort_order_reverse: reverse,
    }
}

#[test]
fn neighbours_wrap_around_and_skip_folders() {
    let root = tree(&[
        "src/client.ts",
        "src/commands.ts",
        "src/config.ts",
        "src/extension.ts",
        "src/lib/constants.ts",
        "src/rename.ts",
    ]);
    let commands = params(&root, "src/commands.ts");
    assert_eq!(step(&commands, 1).as_deref(), Some("config.ts"));
    assert_eq!(step(&commands, -1).as_deref(), Some("client.ts"));
    let client = params(&root, "src/client.ts");
    assert_eq!(step(&client, -1).as_deref(), Some("rename.ts"));
    assert_eq!(
        step(&params(&root, "src/rename.ts"), 1).as_deref(),
        Some("client.ts")
    );
    assert_eq!(step(&params(&root, "src/lib/constants.ts"), 1), None);
}

#[test]
fn excluded_files_are_skipped_unless_current() {
    let root = tree(&[
        "d/.DS_Store",
        "d/a.ts",
        "d/a.js",
        "d/b.js",
        "d/c.ts",
        "d/debug.log",
        "debug.log",
    ]);
    let exclude = json!({
        "**/.DS_Store": true,
        "**/*.js": { "when": "$(basename).ts" },
        "**/c.ts": false,
        "d/*.log": true,
    });
    let base = Params {
        exclude: exclude.as_object().unwrap().clone(),
        ..params(&root, "d/a.ts")
    };
    assert_eq!(order(&base), ["a.ts", "b.js", "c.ts"]);
    let from_hidden = Params {
        path: root.path().join("d/a.js"),
        ..base.clone()
    };
    assert_eq!(step(&from_hidden, 1).as_deref(), Some("a.ts"));
    let outside_folder = Params { root: None, ..base };
    assert_eq!(
        order(&outside_folder),
        ["a.ts", "b.js", "c.ts", "debug.log"]
    );
}

#[test]
fn names_sort_naturally_per_lexicographic_option() {
    let root = tree(&["d/b.ts", "d/B2.ts", "d/a10.ts", "d/a2.ts", "d/_x.ts"]);
    let with = |lexicographic: &str, start: &str| {
        order(&Params {
            explorer: explorer("default", lexicographic, false),
            ..params(&root, start)
        })
    };
    assert_eq!(
        with("default", "d/_x.ts"),
        ["_x.ts", "a2.ts", "a10.ts", "b.ts", "B2.ts"]
    );
    assert_eq!(
        with("upper", "d/_x.ts"),
        ["_x.ts", "B2.ts", "a2.ts", "a10.ts", "b.ts"]
    );
    assert_eq!(
        with("lower", "d/_x.ts"),
        ["_x.ts", "a2.ts", "a10.ts", "b.ts", "B2.ts"]
    );
    assert_eq!(
        with("unicode", "d/B2.ts"),
        ["B2.ts", "_x.ts", "a10.ts", "a2.ts", "b.ts"]
    );
}

#[test]
fn locale_tailoring_matches_the_display_language() {
    let root = tree(&["d/a.ts", "d/z.ts", "d/æ.ts", "d/å.ts"]);
    let with = |locale: &str| {
        order(&Params {
            locale: locale.into(),
            ..params(&root, "d/a.ts")
        })
    };
    assert_eq!(with("en"), ["a.ts", "å.ts", "æ.ts", "z.ts"]);
    assert_eq!(with("nb"), ["a.ts", "z.ts", "æ.ts", "å.ts"]);
    assert_eq!(with("not a locale"), with("en"));
}

#[test]
fn type_modified_and_reverse_orders() {
    let root = tree(&[
        "d/z.css",
        "d/a.ts",
        "d/m.css",
        "d/.eslintrc.json",
        "d/b.json",
    ]);
    let by_type = Params {
        explorer: explorer("type", "default", false),
        ..params(&root, "d/.eslintrc.json")
    };
    assert_eq!(
        order(&by_type),
        [".eslintrc.json", "m.css", "z.css", "b.json", "a.ts"]
    );

    let now = SystemTime::now();
    for (age, name) in ["a.ts", "b.json", "m.css", "z.css", ".eslintrc.json"]
        .iter()
        .enumerate()
    {
        File::options()
            .write(true)
            .open(root.path().join("d").join(name))
            .unwrap()
            .set_modified(now - Duration::from_secs(age as u64 * 100))
            .unwrap();
    }
    let by_modified = Params {
        explorer: explorer("modified", "default", false),
        ..params(&root, "d/a.ts")
    };
    assert_eq!(
        order(&by_modified),
        ["a.ts", "b.json", "m.css", "z.css", ".eslintrc.json"]
    );

    let reversed = Params {
        explorer: explorer("default", "default", true),
        ..params(&root, "d/z.css")
    };
    assert_eq!(
        order(&reversed),
        ["z.css", "m.css", "b.json", "a.ts", ".eslintrc.json"]
    );
}

#[cfg(unix)]
#[test]
fn symlinked_files_count_as_files() {
    let root = tree(&["d/a.ts", "d/c.ts", "target/b.ts", "target/dir/"]);
    std::os::unix::fs::symlink(root.path().join("target/b.ts"), root.path().join("d/b.ts"))
        .unwrap();
    std::os::unix::fs::symlink(root.path().join("target/dir"), root.path().join("d/b.dir"))
        .unwrap();
    assert_eq!(order(&params(&root, "d/a.ts")), ["a.ts", "b.ts", "c.ts"]);
}

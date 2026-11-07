#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use dotfmt_core::config::Setting;
use dotfmt_core::edit::{Analysis, apply_fixes};
use dotfmt_typescript::analyze::{self, Options};
use dotfmt_typescript::config::Config;
use dotfmt_typescript::dialect::Dialect;
use dotfmt_typescript::format;

fn configured(settings: &str) -> Config {
    let settings = settings
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(line, text)| {
            let (key, value) = text.split_once('=').unwrap();
            (
                key.trim().to_owned(),
                Setting {
                    value: value.trim().to_owned(),
                    source: "dotfmt.dotfile".into(),
                    line: line + 1,
                    global: false,
                },
            )
        })
        .collect();
    Config::from_settings(&settings).unwrap()
}

fn analyze(path: &Path, input: &str, config: &Config) -> Analysis {
    analyze::analyze(
        path,
        input,
        config,
        Dialect::Auto,
        &Options {
            fix: true,
            unsafe_fixes: false,
            type_aware: false,
            organize: true,
            project_analysis: false,
            type_diagnostics: Vec::new(),
        },
    )
    .unwrap()
}

/// Apply the fixes dotfmt would apply while formatting `input`.
fn organized(path: &Path, input: &str, config: &Config) -> String {
    let analysis = analyze(path, input, config);
    let fixes: Vec<_> = analysis.fixes.iter().collect();
    apply_fixes(input, &fixes).unwrap()
}

/// Apply the layout the formatter produces for the same options with `sort_imports` on.
fn sorted(path: &Path, input: &str, config: &Config) -> String {
    let mut sort = config.clone();
    sort.sort_imports = true;
    sort.organize_imports = false;
    sort.remove_unused_imports = false;
    sort.convert_type_only_imports = false;
    format(path, input, &sort, Dialect::Auto).unwrap()
}

/// The imported module specifiers, in source order.
fn sources(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("import ")
                .and_then(|rest| rest.split_once('"'))
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(source, _)| source)
        })
        .collect()
}

fn temporary() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c", "z", "types"] {
        fs::write(
            directory.path().join(format!("{name}.ts")),
            "export const value = 1;\n",
        )
        .unwrap();
    }
    let path = directory.path().join("main.ts");
    (directory, path)
}

#[test]
fn organize_keeps_comments_attached_to_removed_imports() {
    let (_directory, path) = temporary();
    let input = "import { a } from \"./a\";\nimport { b } from \"./b\";\n\n// Types section\n\nimport type { Unused } from \"./types\";\n\nexport { a, b };\n";
    let config = configured("organize_imports = true\nremove_unused_imports = true\n");
    assert_eq!(
        organized(&path, input, &config),
        "import { a } from \"./a\";\nimport { b } from \"./b\";\n\n// Types section\n\nexport { a, b };\n"
    );
}

#[test]
fn organize_keeps_section_comments_and_blank_line_partitions() {
    let (_directory, path) = temporary();
    let input = "import { c } from \"./c\";\n\n// section two\n\nimport { a } from \"./a\";\n\nimport type { Unused } from \"./types\";\n\nc; a;\n";
    let config = configured("organize_imports = true\nremove_unused_imports = true\n");
    let output = organized(&path, input, &config);
    assert!(output.contains("// section two"), "{output}");
    assert!(output.contains("import { a }"), "{output}");
    assert!(!output.contains("Unused"), "{output}");
    assert_eq!(
        output,
        "import { a } from \"./a\";\n\n// section two\n\nimport { c } from \"./c\";\n\nc; a;\n"
    );
}

#[test]
fn organize_keeps_trailing_and_leading_comments_with_their_imports() {
    let (_directory, path) = temporary();
    let input = "import { z } from \"./z\"; // keep z\n// a belongs here\nimport { a } from \"./a\";\nz; a;\n";
    let config = configured("organize_imports = true\n");
    let output = organized(&path, input, &config);
    assert!(output.contains("// keep z"), "{output}");
    assert!(output.contains("// a belongs here"), "{output}");
    assert_eq!(
        output,
        "// a belongs here\nimport { a } from \"./a\";\nimport { z } from \"./z\"; // keep z\nz; a;\n",
        "comments travel with the import that owns them"
    );
}

#[test]
fn organize_never_drops_prettier_ignore_comments() {
    let (_directory, path) = temporary();
    let input =
        "// prettier-ignore\nimport { z } from \"./z\";\nimport { a } from \"./a\";\nz; a;\n";
    let config = configured("organize_imports = true\nremove_unused_imports = true\n");
    assert_eq!(organized(&path, input, &config), input);
}

#[test]
fn organize_keeps_a_removed_imports_comment_ahead_of_the_next_run() {
    let (_directory, path) = temporary();
    let input = "import { a } from \"./a\";\nimport { b } from \"./b\";\n\n// trailing note\n\nimport type { Unused } from \"./types\";\n\na; b;\n";
    let config = configured("organize_imports = true\nremove_unused_imports = true\n");
    let output = organized(&path, input, &config);
    assert_eq!(
        output,
        "import { a } from \"./a\";\nimport { b } from \"./b\";\n\n// trailing note\n\na; b;\n"
    );
    assert!(
        analyze(&path, &output, &config).fixes.is_empty(),
        "{output}"
    );
}

#[test]
fn organize_follows_import_groups_and_internal_prefixes() {
    let (_directory, path) = temporary();
    let input = "import aaa from \"./aaa\";\nimport z from \"~/z\";\naaa; z;\n";
    let config = configured(
        "organize_imports = true\nimport_groups = internal|external\nimport_internal = ~/\n",
    );
    let output = organized(&path, input, &config);
    assert_eq!(
        output,
        "import z from \"~/z\";\n\nimport aaa from \"./aaa\";\naaa; z;\n"
    );
    assert_eq!(sources(&output), sources(&sorted(&path, input, &config)));
}

#[test]
fn organize_sorts_by_the_default_groups_like_sort_imports() {
    let (_directory, path) = temporary();
    let input = "import sibling from \"./sibling\";\nimport external from \"external\";\nimport builtin from \"node:fs\";\nimport parent from \"../parent\";\nsibling; external; builtin; parent;\n";
    let config = configured("organize_imports = true\n");
    let output = organized(&path, input, &config);
    assert_eq!(
        sources(&output),
        ["node:fs", "external", "../parent", "./sibling"]
    );
    assert_eq!(sources(&output), sources(&sorted(&path, input, &config)));
}

#[test]
fn organize_parity_with_sort_imports_across_settings() {
    let (_directory, path) = temporary();
    let cases: &[(&str, &str)] = &[
        (
            "organize_imports = true\n",
            "import z from \"z\";\nimport a from \"a\";\nz; a;\n",
        ),
        (
            "organize_imports = true\nimport_order = desc\n",
            "import z from \"z\";\nimport a from \"a\";\nz; a;\n",
        ),
        (
            "organize_imports = true\nimport_groups = external|sibling\n",
            "import z from \"./z\";\nimport a from \"a\";\nimport b from \"b\";\nz; a; b;\n",
        ),
        (
            "organize_imports = true\nimport_internal = @app/\nimport_groups = internal|external\n",
            "import z from \"z\";\nimport y from \"@app/y\";\nimport x from \"x\";\nz; y; x;\n",
        ),
        (
            "organize_imports = true\nimport_groups = external|sibling\nimport_newlines = false\n",
            "import z from \"./z\";\nimport a from \"a\";\nz; a;\n",
        ),
        (
            "organize_imports = true\nimport_side_effects = true\n",
            "import \"./side\";\nimport { a } from \"./a\";\nimport { z } from \"./z\";\na; z;\n",
        ),
        (
            "organize_imports = true\nimport_partition_by_comment = true\n",
            "import c from \"c\";\n// section\nimport a from \"a\";\nc; a;\n",
        ),
        (
            "organize_imports = true\nimport_partition_by_newline = true\n",
            "import c from \"c\";\n\nimport a from \"a\";\nc; a;\n",
        ),
        (
            "organize_imports = true\n",
            "import z from \"a10\";\nimport a from \"a2\";\nz; a;\n",
        ),
    ];
    for (settings, input) in cases {
        let config = configured(settings);
        let output = organized(&path, input, &config);
        assert_eq!(
            sources(&output),
            sources(&sorted(&path, input, &config)),
            "{settings}{input}\n=> {output}"
        );
        assert!(
            analyze(&path, &output, &config).fixes.is_empty(),
            "not idempotent for {settings}{input}: {output}"
        );
    }
}

#[test]
fn organize_partitions_imports_like_sort_imports() {
    let (_directory, path) = temporary();
    let input = "import c from \"c\";\n// section\nimport a from \"a\";\nc; a;\n";
    let partitioned = configured("organize_imports = true\nimport_partition_by_comment = true\n");
    assert_eq!(organized(&path, input, &partitioned), input);
    let joined = configured("organize_imports = true\n");
    assert_eq!(
        organized(&path, input, &joined),
        "// section\nimport a from \"a\";\nimport c from \"c\";\nc; a;\n"
    );
    let by_newline = configured("organize_imports = true\nimport_partition_by_newline = true\n");
    let blank = "import c from \"c\";\n\nimport a from \"a\";\nc; a;\n";
    assert_eq!(organized(&path, blank, &by_newline), blank);

    // A blank line inside one group keeps its slot instead of being dropped.
    let inside_group = "import { b } from \"./b\";\n\nimport { a } from \"./a\";\nb; a;\n";
    assert_eq!(
        organized(&path, inside_group, &joined),
        "import { a } from \"./a\";\n\nimport { b } from \"./b\";\nb; a;\n"
    );
}

#[test]
fn partitioned_removals_keep_their_comments() {
    let (_directory, path) = temporary();
    let input = "import { a } from \"./a\";\nimport { b } from \"./b\";\n\n// note\n\nimport type { Unused } from \"./types\";\n\na; b;\n";
    let config = configured(
        "organize_imports = true\nremove_unused_imports = true\nimport_partition_by_newline = true\n",
    );
    let output = organized(&path, input, &config);
    assert!(output.contains("// note"), "{output}");
    assert!(!output.contains("Unused"), "{output}");
}

#[test]
fn organize_keeps_unsorted_side_effect_imports_in_place() {
    let (_directory, path) = temporary();
    let input =
        "import { z } from \"./z\";\nimport \"./side\";\nimport { a } from \"./a\";\nz; a;\n";
    let config = configured("organize_imports = true\n");
    assert_eq!(organized(&path, input, &config), input);
    let sorted_side_effects = configured("organize_imports = true\nimport_side_effects = true\n");
    let output = organized(&path, input, &sorted_side_effects);
    assert_eq!(
        output,
        "import { a } from \"./a\";\nimport \"./side\";\nimport { z } from \"./z\";\nz; a;\n"
    );
}

#[test]
fn organize_keeps_the_module_marker_a_removal_left_behind() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("env.ts");
    fs::write(directory.path().join("tsconfig.json"), "{}").unwrap();
    let input = "import type { Unused } from \"./types\";\ndeclare module \"./bar\" {}\n";
    let config = configured("organize_imports = true\nremove_unused_imports = true\n");
    assert_eq!(
        organized(&path, input, &config),
        "export {};\ndeclare module \"./bar\" {}\n"
    );
}

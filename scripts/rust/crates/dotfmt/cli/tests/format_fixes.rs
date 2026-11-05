#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use dotfmt_core::config::{Setting, Settings};
use dotfmt_typescript::config::{Config, FixMode, FixSelection};
use std::fs;
use testkit::tree_pairs;

fn settings(key: &str, value: &str) -> Settings {
    [(
        key.to_owned(),
        Setting {
            value: value.to_owned(),
            source: "project/dotfmt.dotfile".into(),
            line: 3,
            global: false,
        },
    )]
    .into()
}

fn selection(key: &str, value: &str) -> Option<FixSelection> {
    Config::from_settings(&settings(key, value))
        .unwrap()
        .format_fix_selection()
}

fn enabled(config: &Config, key: &str) -> bool {
    match key {
        "remove_unused_imports" => config.remove_unused_imports,
        "remove_unused_exports" => config.remove_unused_exports,
        "remove_unused_locals" => config.remove_unused_locals,
        "remove_unused_parameters" => config.remove_unused_parameters,
        "convert_type_only_imports" => config.convert_type_only_imports,
        "organize_imports" => config.organize_imports,
        other => panic!("unknown key: {other}"),
    }
}

#[test]
fn config_parses_every_format_fix_key() {
    let config = Config::default();
    for enabled in [
        config.remove_unused_imports,
        config.remove_unused_exports,
        config.remove_unused_locals,
        config.remove_unused_parameters,
        config.convert_type_only_imports,
        config.organize_imports,
    ] {
        assert!(!enabled);
    }

    for key in [
        "remove_unused_imports",
        "remove_unused_exports",
        "remove_unused_locals",
        "remove_unused_parameters",
        "convert_type_only_imports",
        "organize_imports",
    ] {
        let config = Config::from_settings(&settings(key, "true"))
            .unwrap_or_else(|error| panic!("{key}: {error}"));
        assert!(enabled(&config, key), "{key}");
        let config = Config::from_settings(&settings(key, "false"))
            .unwrap_or_else(|error| panic!("{key}: {error}"));
        assert!(!enabled(&config, key), "{key}");
        let error = Config::from_settings(&settings(key, "yes")).unwrap_err();
        assert!(
            error.message.contains(key) && error.message.contains("true or false"),
            "{key}: {error}"
        );
    }

    assert_eq!(
        Config::from_settings(&settings("fixes", "safe"))
            .unwrap()
            .fixes,
        FixMode::Safe
    );
    assert_eq!(
        Config::from_settings(&settings("fixes", "unsafe"))
            .unwrap()
            .fixes,
        FixMode::Unsafe
    );
    assert!(Config::from_settings(&settings("fixes", "sometimes")).is_err());
}

#[test]
fn format_fix_selection_follows_modes_and_individual_flags() {
    assert_eq!(
        Config::from_settings(&settings("fixes", "off"))
            .unwrap()
            .format_fix_selection(),
        None
    );

    let safe = selection("fixes", "safe").expect("safe selection");
    assert!(safe.safe);
    assert!(!safe.r#unsafe);
    assert!(safe.remove_unused_imports);
    assert!(safe.convert_type_only_imports);
    assert!(safe.organize_imports);
    assert!(!safe.remove_unused_exports);
    assert!(!safe.remove_unused_locals);
    assert!(!safe.remove_unused_parameters);

    let unsafe_selection = selection("fixes", "unsafe").expect("unsafe selection");
    assert!(unsafe_selection.safe);
    assert!(unsafe_selection.r#unsafe);
    assert!(unsafe_selection.remove_unused_imports);
    assert!(unsafe_selection.convert_type_only_imports);
    assert!(unsafe_selection.organize_imports);
    assert!(unsafe_selection.remove_unused_exports);
    assert!(unsafe_selection.remove_unused_locals);
    assert!(unsafe_selection.remove_unused_parameters);

    let imports = selection("remove_unused_imports", "true").expect("import selection");
    assert!(imports.safe);
    assert!(!imports.r#unsafe);
    assert!(imports.remove_unused_imports);
    assert!(!imports.convert_type_only_imports);
    assert!(!imports.organize_imports);

    let locals = selection("remove_unused_locals", "true").expect("local selection");
    assert!(!locals.safe);
    assert!(locals.r#unsafe);
    assert!(locals.remove_unused_locals);
    assert!(!locals.remove_unused_imports);

    let mut forced = Config::from_settings(&settings("fixes", "safe")).unwrap();
    forced.remove_unused_locals = true;
    let forced = forced.format_fix_selection().expect("forced selection");
    assert!(forced.safe);
    assert!(forced.r#unsafe);
    assert!(forced.remove_unused_locals);
}

#[test]
fn fixes_off_is_byte_identical_to_plain_formatting() {
    let input = "import { unused } from \"./side\";\nconst value={a:1};\nexport { value };\n";
    let expected =
        "import { unused } from \"./side\";\nconst value = { a: 1 };\nexport { value };\n";

    let off = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript { fixes = off\n tailwind = off\n type_aware = false }\n",
        ),
        ("a.ts", input),
    ]);
    let plain = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript { tailwind = off\n type_aware = false }\n",
        ),
        ("a.ts", input),
    ]);

    for root in [off.path(), plain.path()] {
        let output = dotfmt(root).arg("a.ts").run();
        assert!(output.success(), "{output:?}");
        assert_eq!(
            fs::read_to_string(root.join("a.ts")).unwrap(),
            expected,
            "{}",
            root.display()
        );
    }
}

#[test]
fn fixes_safe_removes_unused_imports_and_is_idempotent() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript { fixes = safe\n tailwind = off\n type_aware = false }\n",
        ),
        (
            "a.ts",
            "import { unused } from \"./side\";\nconst value={a:1};\nexport { value };\n",
        ),
    ]);
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert!(output.success(), "{output:?}");
    let expected = "import \"./side\";\nconst value = { a: 1 };\nexport { value };\n";
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        expected
    );

    let check = dotfmt(root.path()).args(["--check", "a.ts"]).run();
    assert!(check.success(), "{check:?}");
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        expected
    );
}

#[test]
fn fixes_off_and_safe_leave_an_unselected_category_alone() {
    let input = "const dead = 1;\nexport const live = 1;\n";
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript { fixes = off\n tailwind = off\n type_aware = false }\n",
        ),
        ("a.ts", input),
    ]);
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(fs::read_to_string(root.path().join("a.ts")).unwrap(), input);

    fs::write(
        root.path().join("dotfmt.dotfile"),
        "typescript { fixes = safe\n tailwind = off\n type_aware = false }\n",
    )
    .unwrap();
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(fs::read_to_string(root.path().join("a.ts")).unwrap(), input);

    fs::write(
        root.path().join("dotfmt.dotfile"),
        "typescript { fixes = off\n remove_unused_locals = true\n tailwind = off\n type_aware = false }\n",
    )
    .unwrap();
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "export const live = 1;\n"
    );
}

#[test]
fn individual_import_flag_removes_imports_with_fixes_off() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript { fixes = off\n remove_unused_imports = true\n tailwind = off\n type_aware = false }\n",
        ),
        (
            "a.ts",
            "import { unused } from \"./side\";\nconst value={a:1};\nexport { value };\n",
        ),
    ]);
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "import \"./side\";\nconst value = { a: 1 };\nexport { value };\n"
    );
}

#[test]
fn type_only_import_conversion_is_selectable_on_its_own() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript { fixes = off\n convert_type_only_imports = true\n tailwind = off\n type_aware = false }\n",
        ),
        (
            "a.ts",
            "import { A } from \"m\";\nlet value: A;\nconsole.log(value);\n",
        ),
    ]);
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "import type { A } from \"m\";\nlet value: A;\nconsole.log(value);\n"
    );
}

#[test]
fn invalid_format_fix_values_fail_before_writing() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript { remove_unused_imports = sometimes }\n",
        ),
        ("a.ts", "import { unused } from \"./side\";\n"),
    ]);
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(
        output.stderr.contains("remove_unused_imports"),
        "{output:?}"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "import { unused } from \"./side\";\n"
    );
}

#[test]
fn format_fixes_preserve_bom_and_windows_line_endings() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript { fixes = safe\n line_endings = windows\n tailwind = off\n type_aware = false }\n",
        ),
        (
            "a.ts",
            "\u{feff}import { unused } from \"./side\";\r\nconst value={a:1};\r\nexport { value };\r\n",
        ),
    ]);
    let output = dotfmt(root.path()).arg("a.ts").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "\u{feff}import \"./side\";\r\nconst value = { a: 1 };\r\nexport { value };\r\n"
    );
}

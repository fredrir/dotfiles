#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

const CONFIG: &str = "env {}\nignore { format_comments = true }\nini {}\nplist {}\n";

fn plist() -> &'static str {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
     <plist version=\"1.0\">\n\
     <dict>\n\
     <key>Label</key>\n\
     <string>com.example.test</string>\n\
     <key>RunAtLoad</key>\n\
     <true/>\n\
     </dict>\n\
     </plist>\n"
}

#[test]
fn config_family_paths_are_owned_by_the_new_languages() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG)]);
    let paths = [
        ".gitconfig",
        ".git/config",
        ".git/info/exclude",
        ".git/info/attributes",
        ".gitignore",
        ".gitattributes",
        ".editorconfig",
        "ignore",
        ".prettierignore",
        ".vscodeignore",
        "unit.service",
        "unit.timer",
        "unit.path",
        ".ssh/config",
        "ssh/config.d/50-servers",
        "ssh_config",
        "Info.plist",
        ".env",
        ".env.local",
        "prod.env",
    ];
    let mut input = String::new();
    for path in paths {
        input.push_str(path);
        input.push('\0');
    }
    input.push_str(".envrc\0");
    let output = dotfmt(root.path()).arg("--owns").stdin(&input).run();
    assert!(output.success(), "{output:?}");
    let mut expected = String::new();
    for path in paths {
        if path.starts_with(".git/") {
            continue;
        }
        expected.push_str(path);
        expected.push('\0');
    }
    assert_eq!(output.stdout, expected);
}

#[test]
fn config_family_files_are_formatted_and_settle() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        (
            ".gitconfig",
            "[user]\n\tname   =   fredrir\n\n[core]\n\teditor   =   nvim\n",
        ),
        (".gitignore", "node_modules   \n\n\n#comment\n"),
        (
            ".gitattributes",
            "*.enc   -text   -merge   diff=sops\n*.enc.* -text -merge diff=sops\n",
        ),
        (
            ".editorconfig",
            "root   =   true\n\n[*.rs]\nindent_size   =   4\n",
        ),
        (
            "unit.service",
            "[Unit]\nDescription   =   Test\n\n[Service]\nExecStart   =   /bin/true\n",
        ),
        ("unit.timer", "[Timer]\nOnCalendar   =   daily\n"),
        ("unit.path", "[Path]\nPathExists   =   /tmp\n"),
        (
            ".ssh/config",
            "Host example\n  HostName    example.com\n  User    root\n",
        ),
        ("Info.plist", plist()),
        (".env", "A  =  1\nB=\"two words\"\n"),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");

    let gitconfig = fs::read_to_string(root.path().join(".gitconfig")).unwrap();
    assert!(gitconfig.contains("  name = fredrir"), "{gitconfig}");
    assert!(gitconfig.contains("  editor = nvim"), "{gitconfig}");

    let gitignore = fs::read_to_string(root.path().join(".gitignore")).unwrap();
    assert!(!gitignore.contains("   \n"), "{gitignore:?}");
    assert!(gitignore.contains("# comment"), "{gitignore:?}");

    let attributes = fs::read_to_string(root.path().join(".gitattributes")).unwrap();
    let lines: Vec<&str> = attributes.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].find("-text"),
        lines[1].find("-text"),
        "{attributes}"
    );

    let editorconfig = fs::read_to_string(root.path().join(".editorconfig")).unwrap();
    assert!(editorconfig.contains("root = true"), "{editorconfig}");
    assert!(editorconfig.contains("indent_size = 4"), "{editorconfig}");

    for (path, expected) in [
        ("unit.service", "Description=Test"),
        ("unit.service", "ExecStart=/bin/true"),
        ("unit.timer", "OnCalendar=daily"),
        ("unit.path", "PathExists=/tmp"),
    ] {
        let text = fs::read_to_string(root.path().join(path)).unwrap();
        assert!(text.contains(expected), "{path}: {text}");
    }

    let ssh = fs::read_to_string(root.path().join(".ssh/config")).unwrap();
    assert!(ssh.contains("    HostName example.com"), "{ssh}");
    assert!(ssh.contains("    User     root"), "{ssh}");

    let env = fs::read_to_string(root.path().join(".env")).unwrap();
    assert!(env.contains("A  =  1"), "{env}");
    assert!(env.contains("B=\"two words\""), "{env}");

    let plist = fs::read_to_string(root.path().join("Info.plist")).unwrap();
    assert!(plist.contains("  <key>Label</key>"), "{plist}");
    assert!(
        plist.contains("  <string>com.example.test</string>"),
        "{plist}"
    );
    assert!(plist.contains("  <true/>"), "{plist}");

    let check = dotfmt(root.path()).args(["--check", "."]).run();
    assert!(check.success(), "{check:?}");
}

#[test]
fn binary_plists_pass_through_unchanged() {
    let root = tree_pairs(&[("dotfmt.dotfile", "plist {}\n")]);
    let binary = root.path().join("binary.plist");
    fs::write(&binary, b"bplist00\x00\x01\x02").unwrap();
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(fs::read(&binary).unwrap(), b"bplist00\x00\x01\x02");
    let check = dotfmt(root.path()).args(["--check", "."]).run();
    assert!(check.success(), "{check:?}");
}

#[test]
fn dialect_overrides_are_available_for_ini_and_ignore() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "ini {}\nignore { final_newline = true }\n",
        ),
        ("hosts.ini", "Host example\n  HostName    example.com\n"),
        ("patterns.ignore", "node_modules\n"),
    ]);
    let output = dotfmt(root.path())
        .args(["-l", "ini", "--dialect", "ssh", "hosts.ini"])
        .run();
    assert!(output.success(), "{output:?}");
    let hosts = fs::read_to_string(root.path().join("hosts.ini")).unwrap();
    assert!(hosts.contains("    HostName example.com"), "{hosts}");

    let output = dotfmt(root.path())
        .args(["-l", "ignore", "--dialect", "attributes", "patterns.ignore"])
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("patterns.ignore")).unwrap(),
        "node_modules\n"
    );
}

#[test]
fn env_and_plist_reject_dialect_overrides() {
    let root = tree_pairs(&[("dotfmt.dotfile", "env {}\nplist {}\n")]);
    for (path, language) in [("a.env", "env"), ("a.plist", "plist")] {
        let output = dotfmt(root.path())
            .args(["--dialect", "xml", "--stdin", path])
            .stdin("A=1")
            .run();
        assert_eq!(output.code(), Some(1), "{output:?}");
        assert!(
            output.stderr.contains(&format!("{language} has no")),
            "{output:?}"
        );
    }
}

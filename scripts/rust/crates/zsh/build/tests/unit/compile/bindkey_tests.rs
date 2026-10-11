use super::*;

fn names(values: &[&str]) -> Vec<(Touch, String)> {
    let values: Vec<String> = values.iter().map(|value| value.to_string()).collect();
    touches(&values)
        .into_iter()
        .map(|(touch, (_, name))| (touch, name))
        .collect()
}

fn define(name: &str) -> (Touch, String) {
    (Touch::Define, name.to_string())
}

fn read(name: &str) -> (Touch, String) {
    (Touch::Call, name.to_string())
}

#[test]
fn spellings_of_one_key_sequence_share_a_name() {
    assert_eq!(key_name("^R"), "^R");
    assert_eq!(key_name("\\C-r"), key_name("^R"));
    assert_eq!(key_name("\\ec"), "^[c");
    assert_eq!(key_name("\u{1b}[1;5D"), key_name("\\e[1;5D"));
    assert_eq!(key_name("\\t"), key_name("^I"));
    assert_eq!(key_name("^?"), "^?");
    assert_eq!(key_name("\\M-x"), "\\M-x");
    assert_eq!(key_name("\\033"), "^[");
    assert_eq!(key_name("$key"), "*");
}

#[test]
fn bindings_name_their_keymap_and_keys() {
    assert_eq!(names(&["^T", "fzf-file-widget"]), [define("main ^T")]);
    assert_eq!(names(&["-M", "emacs", "-r", "^R"]), [define("main ^R")]);
    assert_eq!(
        names(&["-rM", "vicmd", "^R", "^S"]),
        [define("vicmd ^R"), define("vicmd ^S")]
    );
    assert_eq!(
        names(&["-s", "^X", "text", "^Y", "more"]),
        [define("main ^X"), define("main ^Y")]
    );
    assert_eq!(
        names(&["-M", "motion", "-R", "a-z", "w"]),
        [define("motion *")]
    );
    assert_eq!(names(&["-N", "motion"]), [define("motion *")]);
    assert_eq!(names(&["-e"]), [define("main *")]);
    assert_eq!(
        names(&["-M", "emacs", "$@"]),
        [define("main *"), read("main *")]
    );
}

#[test]
fn a_key_alone_reads_its_binding() {
    assert_eq!(names(&["^I"]), [read("main ^I")]);
    assert_eq!(names(&["-M", "vicmd"]), [read("vicmd *")]);
    assert!(names(&["-l"]).is_empty());
}

#[test]
fn a_runtime_keymap_or_key_covers_every_one() {
    assert!(covers("main *", "main ^R"));
    assert!(covers("* ^R", "vicmd ^R"));
    assert!(!covers("main ^R", "main ^T"));
    assert!(!covers("vicmd *", "main ^R"));
}

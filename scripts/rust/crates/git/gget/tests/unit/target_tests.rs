use super::*;

fn read(input: &str) -> Target {
    parse(input, None).expect("the target is read")
}

fn mine(input: &str) -> Target {
    parse(input, Some("fredrir")).expect("the target is read")
}

fn shape(target: &Target) -> (String, Option<&str>, &str, &str) {
    (
        target.slug(),
        target.reference.as_deref(),
        target.path.as_str(),
        target.name(),
    )
}

#[test]
fn a_url_without_a_marker_is_the_default_branch() {
    let target = read("https://github.com/user/repo/folder_8/folder_9");
    assert_eq!(
        shape(&target),
        ("user/repo".into(), None, "folder_8/folder_9", "folder_9")
    );
}

#[test]
fn a_tree_marker_names_the_branch() {
    let target = read("https://github.com/user/repo/tree/dev/folder_8/folder_10");
    assert_eq!(
        shape(&target),
        (
            "user/repo".into(),
            Some("dev"),
            "folder_8/folder_10",
            "folder_10"
        )
    );
}

#[test]
fn a_blob_marker_names_it_the_same_way() {
    let target = read("https://github.com/user/repo/blob/dev/README.md");
    assert_eq!(
        shape(&target),
        ("user/repo".into(), Some("dev"), "README.md", "README.md")
    );
}

#[test]
fn the_repository_itself_is_named_after_the_repository() {
    let target = read("https://github.com/user/repo");
    assert_eq!(shape(&target), ("user/repo".into(), None, "", "repo"));
}

#[test]
fn the_owner_flag_stands_in_for_the_first_segment() {
    let target = mine("nsql/README.md");
    assert_eq!(
        shape(&target),
        ("fredrir/nsql".into(), None, "README.md", "README.md")
    );
}

#[test]
fn a_middle_segment_is_part_of_the_path() {
    let target = mine("nsql/dev/README.md");
    assert_eq!(
        shape(&target),
        ("fredrir/nsql".into(), None, "dev/README.md", "README.md")
    );
}

#[test]
fn every_address_form_names_the_same_place() {
    let same = [
        "https://github.com/user/repo/tree/main/src",
        "http://www.github.com/user/repo/tree/main/src",
        "github.com/user/repo/tree/main/src",
        "git@github.com:user/repo.git/tree/main/src",
        "ssh://git@github.com/user/repo/tree/main/src",
        "https://github.com/user/repo/tree/main/src?plain=1#L4",
        "user/repo/tree/main/src",
    ];
    for input in same {
        let target = read(input);
        assert_eq!(
            (target.url(), target.reference.as_deref(), target.path),
            (
                "https://github.com/user/repo".to_string(),
                Some("main"),
                "src".to_string()
            ),
            "{input}"
        );
    }
}

#[test]
fn a_lone_tree_is_a_folder_called_tree() {
    let target = read("user/repo/tree");
    assert_eq!(shape(&target), ("user/repo".into(), None, "tree", "tree"));
}

#[test]
fn another_host_is_refused() {
    for input in [
        "https://gitlab.com/user/repo",
        "git@gitlab.com:user/repo.git",
        "https://raw.githubusercontent.com/user/repo/main/README.md",
    ] {
        let error = parse(input, None).expect_err("the host is refused");
        assert!(error.contains("not a github.com address"), "{error}");
    }
}

#[test]
fn a_url_is_not_a_repository_of_ones_own() {
    let error =
        parse("https://github.com/user/repo", Some("fredrir")).expect_err("the flag is refused");
    assert!(error.contains("not a URL"), "{error}");
}

#[test]
fn a_target_without_a_repository_says_so() {
    for (input, owner) in [("user", None), ("", None), ("", Some("fredrir"))] {
        let error = parse(input, owner).expect_err("the target is refused");
        assert!(error.contains("expected"), "{error}");
    }
}

#[test]
fn a_path_cannot_climb_out_of_the_repository() {
    let error = parse("user/repo/../../etc", None).expect_err("the path is refused");
    assert!(error.contains(".."), "{error}");
}

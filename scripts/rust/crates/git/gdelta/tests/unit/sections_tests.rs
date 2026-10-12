use super::*;

fn sections(diff: &str) -> Vec<Section> {
    Sections::new(diff.as_bytes())
        .collect::<io::Result<_>>()
        .unwrap()
}

fn layouts(diff: &str) -> Vec<Option<Layout>> {
    sections(diff)
        .into_iter()
        .map(|section| section.layout)
        .collect()
}

fn joined(diff: &str) -> String {
    let text: Vec<u8> = sections(diff)
        .into_iter()
        .flat_map(|section| section.text)
        .collect();
    String::from_utf8(text).unwrap()
}

const ADDED: &str = "\
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+one
+two
";

const REMOVED: &str = "\
diff --git a/old.txt b/old.txt
deleted file mode 100644
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-gone
";

const CHANGED: &str = "\
diff --git a/both.txt b/both.txt
--- a/both.txt
+++ b/both.txt
@@ -1,2 +1,2 @@
 kept
-before
+after
";

const RENAMED: &str = "\
diff --git a/from.txt b/to.txt
similarity index 100%
rename from from.txt
rename to to.txt
";

#[test]
fn only_added_lines_are_unified() {
    assert_eq!(layouts(ADDED), [Some(Layout::Unified)]);
}

#[test]
fn only_removed_lines_are_unified() {
    assert_eq!(layouts(REMOVED), [Some(Layout::Unified)]);
}

#[test]
fn added_and_removed_lines_are_split() {
    assert_eq!(layouts(CHANGED), [Some(Layout::Split)]);
}

#[test]
fn a_file_without_hunks_has_no_layout() {
    assert_eq!(layouts(RENAMED), [None]);
}

#[test]
fn each_file_is_its_own_section() {
    let diff = [ADDED, CHANGED, RENAMED, REMOVED].concat();
    assert_eq!(
        layouts(&diff),
        [
            Some(Layout::Unified),
            Some(Layout::Split),
            None,
            Some(Layout::Unified),
        ]
    );
}

#[test]
fn sections_keep_the_diff_text() {
    let diff = [
        "commit abc\n\n    message\n\n",
        ADDED,
        CHANGED,
        "no newline at the end",
    ]
    .concat();
    assert_eq!(joined(&diff), diff);
}

#[test]
fn hunk_headers_lose_the_enclosing_code() {
    let diff = "\
diff --git a/a.txt b/a.txt
@@ -14,3 +14,4 @@ gui:
 kept
+added
";
    assert_eq!(joined(diff), diff.replace(" gui:", ""));
}

#[test]
fn colored_hunk_headers_lose_their_escapes_with_the_code() {
    let diff = "diff --git a/a b/a\n\x1b[36m@@ -1 +1 @@\x1b[m fn main() {\n-a\n+b\n";
    assert_eq!(joined(diff), "diff --git a/a b/a\n@@ -1 +1 @@\n-a\n+b\n");
}

#[test]
fn combined_hunk_headers_are_kept() {
    let diff = "diff --cc a.txt\n@@@ -1,1 -1,1 +1,1 @@@ fn main() {\n++merged\n";
    assert_eq!(joined(diff), diff);
}

#[test]
fn text_before_the_first_file_has_no_layout() {
    let diff = ["commit abc\nAuthor: someone\n\n    message\n\n", CHANGED].concat();
    assert_eq!(layouts(&diff), [None, Some(Layout::Split)]);
}

#[test]
fn header_lines_are_not_changes() {
    let diff = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1,2 @@
 kept
+added
";
    assert_eq!(layouts(diff), [Some(Layout::Unified)]);
}

#[test]
fn a_removed_line_may_look_like_a_header() {
    let diff = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
--- dashes
+++ pluses
";
    assert_eq!(layouts(diff), [Some(Layout::Split)]);
}

#[test]
fn colored_lines_are_read_through_their_escapes() {
    let diff = "\
\x1b[1mdiff --git a/a.txt b/a.txt\x1b[m
\x1b[1m--- a/a.txt\x1b[m
\x1b[1m+++ b/a.txt\x1b[m
\x1b[36m@@ -1 +1 @@\x1b[m
\x1b[31m-before\x1b[m
\x1b[32m+after\x1b[m
\x1b[1mdiff --git a/b.txt b/b.txt\x1b[m
\x1b[36m@@ -0,0 +1 @@\x1b[m
\x1b[32m+new\x1b[m
";
    assert_eq!(layouts(diff), [Some(Layout::Split), Some(Layout::Unified)]);
}

#[test]
fn combined_diffs_are_unified() {
    let diff = "\
diff --cc a.txt
index 1,2..3
--- a/a.txt
+++ b/a.txt
@@@ -1,1 -1,1 +1,1 @@@
- ours
 -theirs
++merged
";
    assert_eq!(layouts(diff), [Some(Layout::Unified)]);
}

#[test]
fn empty_input_has_no_sections() {
    assert!(sections("").is_empty());
}

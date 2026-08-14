work_git() {
  git -C "$WORK" -c user.email=test@example.com -c user.name=test "$@"
}

setup_work_tree() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"
  command -v git >/dev/null 2>&1 || fail "git is required"

  WORK="$SANDBOX/work"
  unset ANSWER
  mkdir -p "$WORK"
  work_git init -q . 2>/dev/null || fail "git init failed"

  printf 'one\ntwo\n' > "$WORK/mod.txt"
  printf 'gone\n' > "$WORK/del.txt"
  printf 'keep\n' > "$WORK/keep.txt"
  printf 'ignored.log\n' > "$WORK/.gitignore"
  work_git add -A
  work_git commit -qm init
}

run_gdd() {
  local real_zsh
  real_zsh="$(command -v zsh)"
  OUTPUT="$(cd "$WORK" && printf '%s\n' "${ANSWER-y}" |
    "$real_zsh" -f -c \
      "source '$SOURCE_ROOT/shared/zsh/conf.d/92-git-discard.zsh'; gdd $*" 2>&1)"
  STATUS=$?
  return 0
}

assert_tree_is_clean() {
  local left
  left="$(work_git status --porcelain)"
  [ -z "$left" ] || fail "expected nothing left to discard, got:
$left"
}

test_gdd_restores_tracked_changes() {
  setup_work_tree
  printf 'one\ntwo\nthree\n' > "$WORK/mod.txt"
  rm -f "$WORK/del.txt"
  printf 'staged\n' > "$WORK/keep.txt"
  work_git add keep.txt

  run_gdd
  assert_ok

  assert_file_is "$WORK/mod.txt" 'one
two'
  assert_file_is "$WORK/del.txt" 'gone'
  assert_file_is "$WORK/keep.txt" 'keep'
  assert_tree_is_clean
}

test_gdd_deletes_untracked_files_and_directories() {
  setup_work_tree
  printf 'loose\n' > "$WORK/untracked.txt"
  mkdir -p "$WORK/untracked_dir/sub"
  printf 'x\n' > "$WORK/untracked_dir/sub/f.txt"
  printf 'added\n' > "$WORK/added.txt"
  work_git add added.txt

  run_gdd
  assert_ok

  assert_absent "$WORK/untracked.txt"
  assert_absent "$WORK/untracked_dir"
  assert_absent "$WORK/added.txt"
  assert_tree_is_clean
}

test_gdd_keeps_ignored_files() {
  setup_work_tree
  printf 'log\n' > "$WORK/ignored.log"
  printf 'loose\n' > "$WORK/untracked.txt"

  run_gdd
  assert_ok

  assert_output_lacks 'ignored.log'
  assert_file_is "$WORK/ignored.log" 'log'
  assert_absent "$WORK/untracked.txt"
}

test_gdd_keeps_a_nested_repository() {
  setup_work_tree
  mkdir -p "$WORK/nested"
  git -C "$WORK/nested" init -q . 2>/dev/null
  printf 'inside\n' > "$WORK/nested/f.txt"
  printf 'loose\n' > "$WORK/untracked.txt"

  run_gdd
  assert_ok

  assert_output_has 'nested repository'
  assert_file_is "$WORK/nested/f.txt" 'inside'
  assert_absent "$WORK/untracked.txt"
}

test_gdd_answered_no_discards_nothing() {
  setup_work_tree
  ANSWER=n
  printf 'changed\n' > "$WORK/mod.txt"
  printf 'loose\n' > "$WORK/untracked.txt"

  run_gdd
  assert_ok

  assert_output_has 'cancelled'
  assert_file_is "$WORK/mod.txt" 'changed'
  assert_exists "$WORK/untracked.txt"
}

test_gdd_dry_run_discards_nothing() {
  setup_work_tree
  printf 'changed\n' > "$WORK/mod.txt"
  printf 'loose\n' > "$WORK/untracked.txt"

  run_gdd --dry-run
  assert_ok

  assert_output_has 'restore to HEAD'
  assert_output_has 'delete permanently'
  assert_file_is "$WORK/mod.txt" 'changed'
  assert_exists "$WORK/untracked.txt"
}

test_gdd_limits_itself_to_the_given_paths() {
  setup_work_tree
  mkdir -p "$WORK/docs"
  printf 'doc\n' > "$WORK/docs/guide.md"
  work_git add -A
  work_git commit -qm docs

  printf 'edited\n' > "$WORK/docs/guide.md"
  printf 'changed\n' > "$WORK/mod.txt"

  run_gdd docs
  assert_ok

  assert_file_is "$WORK/docs/guide.md" 'doc'
  assert_file_is "$WORK/mod.txt" 'changed'
}

test_gdd_counts_the_lines_it_would_throw_away() {
  setup_work_tree
  printf 'ONE\ntwo\nthree\n' > "$WORK/mod.txt"
  printf 'a\nb\n' > "$WORK/untracked.txt"

  run_gdd --dry-run
  assert_ok

  assert_output_has '+2 -1'
  assert_output_has '1 restored, 1 deleted   +4  -1'
}

test_gdd_lists_twelve_entries_of_a_section_unless_asked_for_all() {
  setup_work_tree
  local i
  for i in 01 02 03 04 05 06 07 08 09 10 11 12 13 14 15; do
    printf 'x\n' > "$WORK/loose-$i.txt"
  done

  run_gdd --dry-run
  assert_ok
  assert_output_has 'and 3 more'
  assert_output_lacks 'loose-15.txt'

  run_gdd --dry-run --all
  assert_ok
  assert_output_has 'loose-15.txt'
  assert_output_lacks 'and 3 more'
}

test_gdd_discards_staged_files_before_the_first_commit() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"
  WORK="$SANDBOX/fresh"
  unset ANSWER
  mkdir -p "$WORK"
  work_git init -q . 2>/dev/null || fail "git init failed"
  printf 'x\n' > "$WORK/staged.txt"
  work_git add staged.txt
  printf 'y\n' > "$WORK/loose.txt"

  run_gdd
  assert_ok

  assert_absent "$WORK/staged.txt"
  assert_absent "$WORK/loose.txt"
  assert_tree_is_clean
}

test_gdd_reports_a_tree_with_nothing_to_discard() {
  setup_work_tree

  run_gdd
  assert_ok
  assert_output_has 'nothing to discard'
}

test_gdd_outside_a_repository_fails() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"
  WORK="$SANDBOX/plain"
  unset ANSWER
  mkdir -p "$WORK"

  run_gdd
  assert_fails
  assert_output_has 'not a git repository'
}

test_gdd_rejects_an_unknown_option() {
  setup_work_tree

  run_gdd --bogus
  assert_fails
  assert_output_has 'unknown option'
  assert_output_has 'usage: gdd'
}

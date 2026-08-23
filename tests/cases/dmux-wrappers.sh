#!/usr/bin/env bash

# `ssa`/`ssm` are narrow create-or-connect shortcuts, not alternate CLIs
# (plan §17, ADR 010 §4). The review's non-Rust sweep listed the wrapper's
# `new` expansion (91-tmux-attach.zsh:45) among the sites that carry a user
# to an epoch-sensitive verb without verifying anything themselves (report
# 08 §7). These cases are that finding made executable (ADR 012 WS-E.2):
# the wrapper expands a lone bare word to `dmux --host HOST new NAME`,
# forwards everything else verbatim, and adds no backend flag, no policy
# variable, no seam and no default of its own — so every plan it produces
# is byte-identical to the direct `dmux` invocation (acceptance case 45,
# "wrappers expand to the same plans as direct dmux") and every refusal the
# verified crate path returns reaches the shell unmasked. The allowlist
# itself is held equal to the CLI by
# `cli::the_wrapper_verb_allowlist_matches_the_cli`; nothing here re-derives
# it. A recording `dmux` shim stands in for the binary; nothing touches a
# mux server, the registry, or the live runtime directory.

WRAPPER="$SOURCE_ROOT/shared/zsh/conf.d/91-tmux-attach.zsh"

setup_wrapper_fixtures() {
  command -v zsh >/dev/null 2>&1 || fail "zsh is required"
  SHIMS="$SANDBOX/wrapper-bin"
  TRACE="$SANDBOX/dmux.argv"
  ENV_TRACE="$SANDBOX/dmux.env"
  mkdir -p "$SHIMS"
  # Records argv one line per call, and the two policy variables plus the
  # flag-shaped words exactly as the child saw them.
  printf '%s\n' \
    '#!/bin/sh' \
    'printf "%s\n" "$*" >> "$TRACE"' \
    'printf "wez_first=%s legacy_policy=%s\n" "${DMUX_WEZ_FIRST-unset}" "${DMUX_LEGACY_POLICY-unset}" >> "$ENV_TRACE"' \
    'exit "${DMUX_TEST_EXIT:-0}"' \
    > "$SHIMS/dmux"
  chmod +x "$SHIMS/dmux"
  export TRACE ENV_TRACE WRAPPER
}

# run_wrapper SCRIPT: source the wrapper into a fresh `zsh -f` with the
# shim first on PATH and both policy variables scrubbed from the parent,
# then run SCRIPT. STATUS is the script's exit status; OUTPUT its stderr.
run_wrapper() {
  OUTPUT="$(env -u DMUX_WEZ_FIRST -u DMUX_LEGACY_POLICY \
    PATH="$SHIMS:$PATH" zsh -f -c "source \$WRAPPER; $1" 2>&1 >/dev/null)"
  STATUS=$?
  return 0
}

argv_trace() {
  [ -f "$TRACE" ] && cat "$TRACE"
  return 0
}

assert_trace_is() {
  [ "$(argv_trace)" = "$1" ] || fail "dmux argv trace:
--- got ---
$(argv_trace)
--- want ---
$1"
}

test_wrapper_bare_word_is_create_or_connect_on_the_named_host() {
  setup_wrapper_fixtures
  run_wrapper 'ssa dev; ssm dev'
  assert_ok
  assert_trace_is '--host archie new dev
--host macie new dev'
}

test_wrapper_forwards_every_listed_verb_verbatim_instead_of_creating() {
  setup_wrapper_fixtures
  # Iterate the array the wrapper itself declares, so a verb added to it is
  # covered the moment it lands; equality with the CLI is the Rust test's.
  run_wrapper '
    (( ${#_dmux_verbs} > 0 )) || exit 70
    [[ ${(t)_dmux_verbs} == array* ]] || exit 71
    for verb in $_dmux_verbs; do ssa $verb; done
  '
  assert_ok
  local want='' verb
  for verb in $(env -u DMUX_WEZ_FIRST zsh -f -c 'source $WRAPPER; print -rl -- $_dmux_verbs'); do
    want="${want:+$want
}--host archie $verb"
  done
  assert_trace_is "$want"
  case "$(argv_trace)" in
    *"new ls"*|*"new detach"*|*"new repair"*) fail "a verb was turned into a Space name" ;;
  esac
}

test_wrapper_allowlist_already_names_repair_so_a_new_subcommand_needs_no_edit() {
  setup_wrapper_fixtures
  # `repair retire-incarnation` (ADR 012 §10, §7.1 addition) is a
  # subcommand of a verb the list already carries; the wrapper forwards it
  # whole and never sees the second word.
  run_wrapper 'ssa repair retire-incarnation --backend tmux --epoch 0badcafe-0000-4000-8000-00000000f1f1 -y'
  assert_ok
  assert_trace_is '--host archie repair retire-incarnation --backend tmux --epoch 0badcafe-0000-4000-8000-00000000f1f1 -y'
}

test_wrapper_spelling_the_verb_reaches_a_space_named_after_it() {
  setup_wrapper_fixtures
  run_wrapper 'ssa new ls; ssm new detach'
  assert_ok
  assert_trace_is '--host archie new ls
--host macie new detach'
}

test_wrapper_forwards_flags_and_multiword_argv_untouched() {
  setup_wrapper_fixtures
  run_wrapper '
    ssa
    ssa -x
    ssa dev extra
    ssa --format json ls
    ssa con dev
    ssa rm --row 1 --yes
    ssa adopt native:tmux:JDE
    ssa migrate --commit --yes
  '
  assert_ok
  assert_trace_is '--host archie
--host archie -x
--host archie dev extra
--host archie --format json ls
--host archie con dev
--host archie rm --row 1 --yes
--host archie adopt native:tmux:JDE
--host archie migrate --commit --yes'
}

test_wrapper_injects_no_policy_variable_backend_flag_or_seam() {
  setup_wrapper_fixtures
  run_wrapper 'ssa dev; ssa ls; ssa con dev'
  assert_ok
  # The policy variables reach the child exactly as the parent had them:
  # unset here, and the wrapper never sets, clears or defaults them.
  assert_file_is "$ENV_TRACE" 'wez_first=unset legacy_policy=unset
wez_first=unset legacy_policy=unset
wez_first=unset legacy_policy=unset'
  # No backend selection, no compatibility escape, no owner-side seam:
  # every one of these would be backend logic in a wrapper (ADR 010 §4).
  local word
  for word in --backend --create -A --socket --epoch --data-dir --lock-dir --namespace --name --row --allow-name-collision; do
    case "$(argv_trace)" in
      *" $word"*|*" $word="*) fail "wrapper injected $word:
$(argv_trace)" ;;
    esac
  done

  # An inherited opt-in is passed through just as untouched.
  : > "$ENV_TRACE"
  OUTPUT="$(env -u DMUX_LEGACY_POLICY DMUX_WEZ_FIRST=1 \
    PATH="$SHIMS:$PATH" zsh -f -c 'source $WRAPPER; ssa dev' 2>&1 >/dev/null)"
  STATUS=$?
  assert_ok
  assert_file_is "$ENV_TRACE" 'wez_first=1 legacy_policy=unset'
}

test_wrapper_returns_the_exit_status_of_the_plan_it_expanded_to() {
  setup_wrapper_fixtures
  # A typed refusal from the verified path (backend_epoch_changed is 1,
  # not-found 3, partial 7) must reach the shell, not be masked as 0.
  run_wrapper '
    DMUX_TEST_EXIT=1 ssa dev; print -u2 -- "new=$?"
    DMUX_TEST_EXIT=3 ssa con dev; print -u2 -- "con=$?"
    DMUX_TEST_EXIT=7 ssa ls; print -u2 -- "ls=$?"
    DMUX_TEST_EXIT=0 ssm dev; print -u2 -- "ssm=$?"
  '
  assert_ok
  [ "$OUTPUT" = 'new=1
con=3
ls=7
ssm=0' ] || fail "exit statuses were not propagated:
$OUTPUT"
}

test_wrapper_without_dmux_fails_with_127_and_expands_nothing() {
  setup_wrapper_fixtures
  local empty_bin="$SANDBOX/empty-bin" zsh_bin
  zsh_bin="$(command -v zsh)"
  mkdir -p "$empty_bin"
  OUTPUT="$(env -u DMUX_WEZ_FIRST -u DMUX_LEGACY_POLICY PATH="$empty_bin" \
    "$zsh_bin" -f -c 'source $WRAPPER; ssa dev' 2>&1 >/dev/null)"
  STATUS=$?
  [ "$STATUS" -eq 127 ] || fail "expected 127 without dmux, got $STATUS: $OUTPUT"
  assert_output_has "ssa: dmux not installed"
  assert_absent "$TRACE"
}

# Acceptance case 45: the wrapper's plan is the direct invocation's plan.
test_wrapper_expands_to_the_same_plan_as_direct_dmux() {
  setup_wrapper_fixtures
  run_wrapper '
    ssa dev;            dmux --host archie new dev
    ssm ls;             dmux --host macie ls
    ssa new ls;         dmux --host archie new ls
    ssa con 7;          dmux --host archie con 7
    ssa --format json ls; dmux --host archie --format json ls
    ssa;                dmux --host archie
  '
  assert_ok
  local line count=0 odd='' even=''
  while IFS= read -r line; do
    count=$((count + 1))
    if [ $((count % 2)) -eq 1 ]; then odd="$odd$line
"; else even="$even$line
"; fi
  done < "$TRACE"
  [ "$count" -eq 12 ] || fail "expected 12 recorded invocations, got $count"
  [ "$odd" = "$even" ] || fail "wrapper plans differ from direct dmux plans:
--- wrapper ---
$odd--- direct ---
$even"
  # And the environment each pair carried is identical too.
  [ "$(sort -u "$ENV_TRACE" | wc -l | tr -d ' ')" -eq 1 ] \
    || fail "the wrapper changed the child's policy environment:
$(cat "$ENV_TRACE")"
}

test_wrapper_dmx_alias_is_plain_dmux_with_no_host() {
  setup_wrapper_fixtures
  # Aliases bind at parse time, so the alias the wrapper defines is only
  # visible to text parsed after the source: eval it.
  run_wrapper 'eval "dmx ls; dmx dev"'
  assert_ok
  assert_trace_is 'ls
dev'
}

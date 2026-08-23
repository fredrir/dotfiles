# Attach the peer machine's terminal from either side.
#
#   ssa   archie      ssm   macie
#
# Narrow create-or-connect shortcuts, not alternate CLIs (plan §17). A lone
# bare word that is not a dmux verb is a Space name, so `ssa dev` is
# `dmux --host archie new dev` and `new` is already idempotent
# create-or-connect. Everything else forwards verbatim, verbs included, so
# `ssa ls` lists rather than creating a Space called "ls".
#
# To reach a Space whose name collides with a verb, spell the verb yourself:
# `ssa new ls` creates-or-connects one named "ls". (§7.4's `con --name` escape
# is connect-only, so it is not the answer either.)
#
# `ssa detach`/`ssa disconnect` forward and then fail with a usage error, which
# is correct rather than unfortunate: disconnect acts on the invoking local
# client and rejects --host, so there is nothing for a host-scoped spelling of
# it to mean. What the allowlist buys is that it errors instead of silently
# creating a Space named "detach".
#
# The list is checked rather than trusted -- it had already drifted once, naming
# 14 verbs while the CLI exposed 22. `the_wrapper_verb_allowlist_matches_the_cli`
# in scripts/rust/crates/dmux/tests/cli.rs re-derives it from the built binary
# and from this array, and fails naming whichever verb moved. It is declared at
# file scope so that test can evaluate it rather than parse it.
typeset -ga _dmux_verbs=(
  ls list con attach a new disconnect detach recovery rm kill delete
  rename keys doctor group split context repair ssh host adopt migrate help
)

_dmux_wrap() {
  local _wrap_name=$1 _wrap_host=$2
  shift 2
  if ! command -v dmux >/dev/null 2>&1; then
    print -u2 -r -- "$_wrap_name: dmux not installed (run ./setup.sh)"
    return 127
  fi
  if (( $# == 1 )) && [[ "$1" != -* ]]; then
    if (( ! ${_dmux_verbs[(Ie)$1]} )); then
      dmux --host "$_wrap_host" new "$1"
      return $?
    fi
  fi
  dmux --host "$_wrap_host" "$@"
}

ssa() {
  _dmux_wrap ssa archie "$@"
}

ssm() {
  _dmux_wrap ssm macie "$@"
}

alias dmx=dmux
# Borrow dmux's completion for the wrappers — but only when a completion is
# actually registered for dmux; on a machine without the binary (or with an
# empty shim cache) a bare `compdef x=dmux` errors on every shell start.
if (( $+functions[compdef] && $+_comps[dmux] )); then
  compdef ssa=dmux ssm=dmux dmx=dmux
fi

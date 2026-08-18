# Attach the peer machine's terminal from either side.
#
#   ssa   archie      ssm   macie
#
# Narrow create-or-connect shortcuts, not alternate CLIs (plan §17). A lone
# bare word is a Space name, so `ssa dev` is `dmux --host archie new dev` and
# `new` is already idempotent create-or-connect. Everything else forwards
# verbatim, which is how every other operation is spelled: `ssa ls` would
# create a Space called "ls", so list it with `dmux --host archie ls`.
#
# There is deliberately no subcommand allowlist here. The old one had to name
# every verb the CLI grows, and it silently reinterpreted a Space whose name
# collided with a verb. The CLI owns that ambiguity instead: `--name` is the
# exact-name escape for a legacy name that looks like a ref or a subcommand.
_dmux_wrap() {
  local _wrap_name=$1 _wrap_host=$2
  shift 2
  if ! command -v dmux >/dev/null 2>&1; then
    print -u2 -r -- "$_wrap_name: dmux not installed (run ./setup.sh)"
    return 127
  fi
  if (( $# == 1 )) && [[ "$1" != -* ]]; then
    dmux --host "$_wrap_host" new "$1"
    return $?
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

# Attach the peer machine's terminal from either side.
#
#   ssa   archie      ssm   macie
#
# The logic — transport probing, session handling, the lot — lives in the
# dmux crate (scripts/rust/crates/dmux). These wrappers keep the old names
# and most of the old muscle memory: a bare name is create-or-attach
# (`ssa dev` → `dmux con -A dev`), the list/delete spellings and the `-`
# toggle still work, and the remaining old flags (`--tmux`, `-l`)
# intentionally error rather than silently change meaning.
_dmux_wrap() {
  local _wrap_name=$1 _wrap_host=$2
  shift 2
  if ! command -v dmux >/dev/null 2>&1; then
    print -u2 -r -- "$_wrap_name: dmux not installed (run ./setup.sh)"
    return 127
  fi
  # Old ssa took a bare session name; dmux spells that `con -A`. Translate
  # a lone non-flag word that is not a subcommand — keep the case list in
  # sync with the dmux CLI (subcommands and their aliases).
  if (( $# == 1 )) && [[ "$1" != -* ]]; then
    case "$1" in
      ls|list|con|attach|a|new|detach|rm|kill|delete|rename|keys|doctor|help) ;;
      *)
        dmux --host "$_wrap_host" con -A "$1"
        return $?
        ;;
    esac
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

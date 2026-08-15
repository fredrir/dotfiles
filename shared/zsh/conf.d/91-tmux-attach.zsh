# Attach the peer machine's terminal from either side.
#
#   ssa   archie      ssm   macie
#
# The logic — transport probing, session handling, the lot — lives in the
# dmux crate (scripts/rust/crates/dmux). These are compatibility wrappers
# so the old names keep working.
ssa() {
  if ! command -v dmux >/dev/null 2>&1; then
    print -u2 -r -- "ssa: dmux not installed (run ./setup.sh)"
    return 127
  fi
  dmux --host archie "$@"
}

ssm() {
  if ! command -v dmux >/dev/null 2>&1; then
    print -u2 -r -- "ssm: dmux not installed (run ./setup.sh)"
    return 127
  fi
  dmux --host macie "$@"
}

alias dmx=dmux
if (( $+functions[compdef] )); then
  compdef dmx=dmux
fi

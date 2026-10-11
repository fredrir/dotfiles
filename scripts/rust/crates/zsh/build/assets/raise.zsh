# `eval` of a statement: the error that stopped it, which `eval` cleared, stops what runs it
_zsh_build_raise() {
  local code=$?
  if (( ${_zsh_build_stopped:-0} )); then
    unset _zsh_build_stopped
    { : $(( 1 / 0 )) } 2>/dev/null
  fi
  unset _zsh_build_stopped
  return code
}

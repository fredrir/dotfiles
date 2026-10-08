zmodload zsh/parameter 2>/dev/null

_zsh_build_whence() {
  local arg out line name flags=''
  local -i bundled code
  for arg in "$@"; do
    if [[ $arg == -[[:alpha:]]* ]]; then
      flags+=${arg#-}
    elif [[ ${functions_source[$arg]-} == "@BUNDLE@" ]]; then
      bundled=1
    fi
  done
  if (( ! bundled )) || [[ $flags != *v* || $flags == *[cfw]* ]]; then
    builtin whence "$@"
    return
  fi
  out=$(builtin whence "$@")
  code=$?
  if (( ! ${+_zsh_build_origins} )); then
    typeset -gA _zsh_build_origins
    [[ -r @ORIGINS@ ]] && for line in "${(@f)$(<@ORIGINS@)}"; do
      _zsh_build_origins[${line%%$'\t'*}]=${line#*$'\t'}
    done
  fi
  for line in "${(@f)out}"; do
    name=${line% is a shell function from "@BUNDLE@"}
    if [[ $name != "$line" && -n ${_zsh_build_origins[$name]-} ]]; then
      line="$name is a shell function from ${_zsh_build_origins[$name]}"
    fi
    print -r -- "$line"
  done
  return $code
}

whence() {
  _zsh_build_whence "$@"
}

type() {
  _zsh_build_whence -v "$@"
}

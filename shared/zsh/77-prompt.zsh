zmodload zsh/datetime zsh/mathfunc
setopt prompt_subst
: ${VIRTUAL_ENV_DISABLE_PROMPT:=1}

PROMPT='${_prompt_left}'
RPROMPT='${_prompt_right}'
PROMPT2=$'%{\e[90m%}\u2219%{\e[0m%} '
PROMPT_EOL_MARK=''

typeset -g _prompt_dir_color=$THEME_DIR _prompt_char_color=$THEME_CHAR
if [[ ${HOST%%.*} == archie ]]; then
  _prompt_dir_color=$THEME_CYAN
  _prompt_char_color=$THEME_BRIGHT_RED
fi

_prompt_git_branch() {
  local dir=${PWD:A} gitdir head
  if [[ -n $GIT_DIR ]]; then
    gitdir=$GIT_DIR
  else
    while true; do
      if [[ -d $dir/.git ]]; then
        gitdir=$dir/.git
        break
      elif [[ -f $dir/.git ]]; then
        IFS= read -r head <$dir/.git || return 1
        [[ $head == 'gitdir: '* ]] || return 1
        gitdir=${head#gitdir: }
        [[ $gitdir == /* ]] || gitdir=$dir/$gitdir
        break
      elif [[ -f $dir/HEAD && -d $dir/objects && -d $dir/refs ]]; then
        gitdir=$dir
        break
      fi
      [[ $dir == / ]] && return 1
      dir=${dir:h}
    done
  fi
  IFS= read -r head <$gitdir/HEAD 2>/dev/null || return 1
  case $head in
  'ref: refs/heads/'*) REPLY=${head#ref: refs/heads/} ;;
  'ref: '*) REPLY=${head#ref: } ;;
  *) REPLY=HEAD ;;
  esac
}

_prompt_dir() {
  local dir=$PWD
  local -a parts
  if [[ $dir == "$HOME" ]]; then
    dir='~'
  elif [[ $dir == "$HOME"/* ]]; then
    dir="~${dir#"$HOME"}"
  fi
  parts=(${(s:/:)dir})
  (($#parts > 8)) && dir=${(j:/:)parts[-8,-1]}
  REPLY=$dir
}

_prompt_duration() {
  local -i total=$(($1 / 1000))
  local -i days=$((total / 86400)) hours=$((total / 3600 % 24)) minutes=$((total / 60 % 60)) seconds=$((total % 60))
  if ((days)); then
    REPLY="${days}d${hours}h${minutes}m${seconds}s"
  elif ((hours)); then
    REPLY="${hours}h${minutes}m${seconds}s"
  elif ((minutes)); then
    REPLY="${minutes}m${seconds}s"
  else
    REPLY="${seconds}s"
  fi
}

_prompt_preexec() {
  ((_prompt_started = int(rint(EPOCHREALTIME * 1000))))
}

_prompt_precmd() {
  local REPLY left=
  [[ -n $VIRTUAL_ENV ]] && left+="%{$THEME_PYTHON%}(.venv)"
  _prompt_git_branch && left+="%{$THEME_GIT%}[${REPLY//\%/%%}]"
  _prompt_dir
  left+="%{$_prompt_dir_color%}[${REPLY//\%/%%}]%{$_prompt_char_color%}\$%{$THEME_RESET%} "
  _prompt_left=$left
  _prompt_right=
  if ((${+_prompt_started})); then
    local -i elapsed=$((int(rint(EPOCHREALTIME * 1000)) - _prompt_started))
    unset _prompt_started
    if ((elapsed >= 2000)); then
      _prompt_duration $elapsed
      _prompt_right="took %{$THEME_DURATION%}$REPLY%{$THEME_RESET%}"
    fi
  fi
}

precmd_functions+=(_prompt_precmd)
preexec_functions+=(_prompt_preexec)

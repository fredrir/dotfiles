for _plugin_file in $zsh_plugin_sources; do
  defer source "$_plugin_file"
done
unset _plugin_file
cached_eval direnv-hook direnv hook zsh

if (($+functions[_direnv_hook])); then
  functions[_direnv_export]=$functions[_direnv_hook]
  _direnv_needed() {
    [[ -n $DIRENV_DIR ]] && return 0
    local dir=$PWD
    while true; do
      [[ -e $dir/.envrc ]] && return 0
      [[ $dir == / ]] && return 1
      dir=${dir:h}
    done
  }
  _direnv_hook() {
    if [[ $_direnv_ran == "$PWD" ]]; then
      unset _direnv_ran
      return 0
    fi
    _direnv_needed && _direnv_export
  }
  _direnv_hook_chpwd() {
    _direnv_needed && _direnv_export
    _direnv_ran=$PWD
  }
  chpwd_functions[${chpwd_functions[(i)_direnv_hook]}]=_direnv_hook_chpwd
fi

if (($+functions[__zoxide_hook])); then
  __zoxide_hook() {
    \command zoxide add -- "$PWD" &!
  }
fi

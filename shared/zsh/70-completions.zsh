for _bin in "$DOTFILES_COMPILED"/*(N); do
  cached_eval "${_bin:t}-completion" "$_bin" --completions zsh
done
unset _bin

cached_eval wezterm-completion wezterm shell-completion --shell zsh
cached_eval llunde-completion llunde completion zsh

compdef _gdd git-discard
compdef _mux-route attach_mux s

_copy_move_destination() {
  local -a line expl
  local -A opt_args
  local ret=1
  "_${service}" "$@" && ret=0

  comparguments -W line opt_args 0 || return $ret
  (($#line == 2 && CURRENT == $#words)) || return $ret
  [[ $PREFIX == */* && -z $SUFFIX && -z ${opt_args[(I)(-t|--target-directory|--parents)]} ]] || return $ret

  local source=${(Q)line[1]} filename destination="${PREFIX%/*}/"
  filename=${source:t}
  [[ -n $filename && $filename != (.|..) ]] || return $ret
  if [[ ! -e $source && ! -L $source ]]; then
    [[ $source != *[\*\?\[\]\{\}\(\)\`]* && $filename != *'$'* ]] || return $ret
  fi

  compquote filename
  _wanted source-filenames expl 'source filename' \
    compadd -Qf -M 'r:|/=* r:|=*' -p "$destination" -W "${(Q)destination}" -- "$filename" && ret=0
  return $ret
}
compdef _copy_move_destination cp mv

if (($+functions[_wezterm])); then
  _wezterm_cli() {
    words=(wezterm cli "${(@)words[2,-1]}")
    ((CURRENT++))
    _wezterm
  }
  compdef _wezterm_cli wez mux
fi

_just_vtabs() {
  local -a args=("${(@)words[2,-1]}")
  local index=${args[(i)--justfile]} root=$PWD removed=0
  if ((index < $#args)); then
    root=${~args[index+1]:h}
    args[index,index+1]=()
    removed=2
  else
    while [[ $root != / && ! -f $root/justfile ]]; do root=${root:h}; done
  fi
  local tool=$root/target/xtask/wez-vtabs
  if ((CURRENT - removed > 2)) && [[ -x $tool && ${args[1]} == (deploy|build|dev|test|lint|status|setup) ]]; then
    (($+functions[_clap_dynamic_completer_wez_vtabs])) || eval "$(WEZ_VTABS_COMPLETE=zsh $tool)"
    words=(wez-vtabs "${(@)args}")
    ((CURRENT -= removed))
    _clap_dynamic_completer_wez_vtabs
  else
    _just "$@"
  fi
}
compdef _just_vtabs just

_tools_comp_cache="$DOTFILES_ZSH_CACHE/tools-completion.zsh"
[[ -r "$_tools_comp_cache" ]] && source "$_tools_comp_cache"
unset _tools_comp_cache

zstyle ':completion:*' matcher-list '' 'm:{a-zA-Z}={A-Za-z}'

for _plugin_file in $zsh_plugin_sources; do
  defer source "$_plugin_file"
done
unset _plugin_file
() {
  [[ ${HOST%%.*} == archie ]] || return 0

  local config="${XDG_CONFIG_HOME:-$HOME/.config}/starship.toml"
  local cache="$DOTFILES_ZSH_CACHE/starship-archie.toml"
  [[ -n $STARSHIP_CONFIG && $STARSHIP_CONFIG != "$cache" ]] && return 0
  [[ -r $config ]] || return 0

  mkdir -p "${cache:h}" || return
  if sed -e "s/'prompt_dir'/'cyan'/g" \
    -e 's/(prompt_char)/(bright_red)/g' "$config" >"$cache.$$"; then
    mv -f "$cache.$$" "$cache" && export STARSHIP_CONFIG="$cache"
  else
    rm -f "$cache.$$"
  fi
}

cached_eval starship-init starship init zsh
PROMPT_EOL_MARK='' # Fix extra % when no new-line

cached_eval direnv-hook direnv hook zsh

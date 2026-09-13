typeset -gU path PATH
typeset -gU plugins
typeset -gaU zsh_plugin_path
typeset -gaU zsh_plugin_sources

add_path() {
  local -a valid_paths=()
  local dir
  for dir in "$@"; do
    [[ -d "$dir" ]] && valid_paths+=("$dir")
  done
  path=("${valid_paths[@]}" "${path[@]}")
}

add_plugin_path() {
  local dir
  for dir in "$@"; do
    [[ -d $dir ]] && zsh_plugin_path+=($dir)
  done
}

add_plugins() {
  local spec name dir file
  for spec in "$@"; do
    name=${spec##*:}
    ((${+commands[${spec%%:*}]})) || continue

    for dir in $zsh_plugin_path; do
      for file in $dir/$name/$name.zsh $dir/$name.zsh; do
        [[ -r $file ]] || continue
        zsh_plugin_sources+=($file)
        continue 3
      done
    done

    [[ -d $ZSH_CUSTOM/plugins/$name || -d $ZSH/plugins/$name ]] && plugins+=($name)
  done
}

has_cmd() { (($+commands[$1])); }

[[ $OSTYPE == linux* ]] && LINUX=1
[[ -e /etc/arch-release ]] && ARCHLINUX=1
[[ $OSTYPE == darwin* ]] && MACOS=1

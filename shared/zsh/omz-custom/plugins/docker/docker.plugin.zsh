(($+commands[docker])) || return

while IFS= read -r _line; do
  [[ $_line == alias\ * ]] && eval "$_line"
done <"$ZSH/plugins/docker/docker.plugin.zsh"
unset _line

_docker_comp="$ZSH_CACHE_DIR/completions/_docker"

if [[ ! -s $_docker_comp || $commands[docker] -nt $_docker_comp ]]; then
  mkdir -p "${_docker_comp:h}"
  docker completion zsh >"$_docker_comp" 2>/dev/null || command rm -f "$_docker_comp"
fi

if [[ -s $_docker_comp ]] && ! (($+_comps[docker])); then
  typeset -gA _comps
  autoload -Uz _docker
  _comps[docker]=_docker
fi

unset _docker_comp

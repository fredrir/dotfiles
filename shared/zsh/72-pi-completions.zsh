#compdef pi

# Shell completion for the pi coding agent CLI.
# `pi` ships no completion generator, so the command tree lives here while the
# slow-moving data (providers, models, packages, sessions) is resolved on demand.

typeset -ga _pi_commands=(
  'install:Install a package and add it to settings'
  'remove:Remove a package and its source from settings'
  'uninstall:Alias for remove'
  'update:Update pi, packages, or model catalogs'
  'list:List installed packages from settings'
  'config:Open the resource configuration TUI'
  'auth:Print credentials or check provider readiness'
)

typeset -ga _pi_auth_commands=(
  'print-api-key:Print the resolved API key'
  'print-bearer-token:Print a bearer token'
  'check:Check provider readiness'
)

typeset -ga _pi_tool_names=(bash edit glob grep ls read write)
typeset -ga _pi_thinking_levels=(off minimal low medium high xhigh max)

# Providers and models come from `pi --list-models`, cached to disk and refreshed
# once a day in the background so tab completion never waits on startup work.
typeset -g _pi_models_cache="$DOTFILES_ZSH_CACHE/pi-models"

_pi_models_refresh() {
  has_cmd pi || return 0
  local tmp="$_pi_models_cache.$$" out
  out=$(command pi --list-models 2>/dev/null |
    awk 'NR > 1 && NF >= 2 { printf "%s\t%s\n", $1, $2 }')
  if [[ -z $out ]]; then
    rm -f -- "$tmp"
    return 0
  fi
  mkdir -p -- "${_pi_models_cache:h}"
  print -r -- "$out" >|"$tmp" && mv -f -- "$tmp" "$_pi_models_cache"
}

_pi_models_load() {
  [[ -r $_pi_models_cache ]] || _pi_models_refresh
  [[ -r $_pi_models_cache ]] || return 0
  _pi_models_lines=("${(@f)$(<$_pi_models_cache)}")
}

typeset -ga _pi_models_lines
_pi_models_stale=(${~_pi_models_cache}(N.mh+24))
if [[ ! -f $_pi_models_cache ]] || ((${#_pi_models_stale})); then
  defer _pi_models_refresh
fi
unset _pi_models_stale

# `pi install` sources come from npm packages tagged with the `pi-package`
# keyword, which is how the pi.dev gallery discovers them. Cached once a day.
typeset -g _pi_packages_cache="$DOTFILES_ZSH_CACHE/pi-packages"

typeset -ga _pi_package_lines

_pi_packages_refresh() {
  has_cmd curl && has_cmd jq || return 0
  local tmp="$_pi_packages_cache.$$" out
  out=$(curl -fsS --max-time 10 --get \
  --data-urlencode 'text=keywords:pi-package' \
  --data-urlencode 'size=250' \
  'https://registry.npmjs.org/-/v1/search' 2>/dev/null |
  jq -r '[.objects[]? | select(.package.name) |
    {name: .package.name,
     desc: ((.package.description // "") | gsub("[\t\n\r]+"; " ")),
     dl: (.downloads.monthly // 0)}]
    | sort_by(-.dl) | .[] | "\(.name):\(.desc)"' 2>/dev/null)
  if [[ -z $out ]]; then
    rm -f -- "$tmp"
    return 0
  fi
  mkdir -p -- "${_pi_packages_cache:h}"
  print -r -- "$out" >|"$tmp" && mv -f -- "$tmp" "$_pi_packages_cache"
}

_pi_packages_load() {
  [[ -r $_pi_packages_cache ]] || _pi_packages_refresh
  [[ -r $_pi_packages_cache ]] || return 0
  _pi_package_lines=("${(@f)$(<$_pi_packages_cache)}")
}

_pi_packages_stale=(${~_pi_packages_cache}(N.mh+24))
if [[ ! -f $_pi_packages_cache ]] || ((${#_pi_packages_stale})); then
  defer _pi_packages_refresh
fi
unset _pi_packages_stale

_pi_providers() {
  _pi_models_load
  local line provider
  local -aU providers
  for line in "${_pi_models_lines[@]}"; do
    provider=${line%%$'\t'*}
    [[ -n $provider && $provider != "$line" ]] || continue
    providers+=("$provider")
  done
  ((${#providers})) && _describe -t providers 'provider' providers
}

_pi_models() {
  _pi_models_load
  local line provider model
  local -aU models
  for line in "${_pi_models_lines[@]}"; do
    [[ $line == *$'\t'* ]] || continue
    provider=${line%%$'\t'*}
    model=${line#*$'\t'}
    [[ -n $provider && -n $model ]] || continue
    models+=("$model" "$provider/$model")
  done
  ((${#models})) && _describe -t models 'model' models
}

_pi_model_arg() {
  if [[ $PREFIX == *:* ]]; then
    compset -P '*:'
    compadd -S '' -- "${_pi_thinking_levels[@]}"
    return
  fi
  _pi_models
}

_pi_models_arg() {
  compset -P '*,' 2>/dev/null
  _pi_models
}

_pi_tools() {
  compset -P '*,' 2>/dev/null
  compadd -S , -- "${_pi_tool_names[@]}"
}

_pi_themes() {
  local -a themes=(dark light 'light/dark' 'dark/light')
  local dir f
  for dir in "$HOME/.pi/agent/themes" "$PWD/.pi/themes"; do
    for f in "$dir"/*.json(N) "$dir"/*.jsonc(N); do
      themes+=("${f:t:r}")
    done
  done
  _describe -t themes 'theme' themes
}

_pi_sessions() {
  local sessions="$HOME/.pi/agent/sessions"
  local enc="--${${PWD#/}//\//-}--"
  local f
  local -a files uuids
  for f in "$sessions/$enc"/*.jsonl(N); do
    files+=("${f:t}")
    uuids+=("${${f:t}##*_}")
  done
  ((${#uuids})) && compadd -S '' -X 'session id' -- "${(@)uuids}"
  ((${#files})) && compadd -S '' -X 'session file' -- "${(@)files}"
  case $PREFIX in
  */* | .* | \~*) _files ;;
  esac
}

_pi_package_sources() {
  has_cmd jq || return 0
  local dir=$PWD file
  local -a sources
  while :; do
    file=$dir/.pi/settings.json
    [[ -r $file ]] &&
      sources+=("${(@f)$(jq -r '(.packages // [])[]' "$file" 2>/dev/null)}")
    [[ $dir == / ]] && break
    dir=${dir:h}
  done
  file=$HOME/.pi/agent/settings.json
  [[ -r $file ]] &&
    sources+=("${(@f)$(jq -r '(.packages // [])[]' "$file" 2>/dev/null)}")
  sources=("${(@)sources:#}")
  ((${#sources})) && compadd -S '' -- "${(@)sources}"
}

_pi_packages_complete() {
  local insert_prefix=$1 limit=$2
  _pi_packages_load
  ((${#_pi_package_lines})) || return 0
  local line name desc count=0
  local -a names descs opts
  for line in "${_pi_package_lines[@]}"; do
    name=${line%%:*}
    desc=${line#*:}
    ((${#desc} > 72)) && desc="${desc[1,71]}…"
    names+=("$name")
    descs+=("$name -- $desc")
    if [[ -n $limit ]] && ((++count >= limit)); then
      break
    fi
  done
  [[ -n $insert_prefix ]] && opts+=(-P "$insert_prefix")
  compadd -V 'pi package' "${opts[@]}" -d descs -- "${names[@]}"
}

_pi_install_schemes() {
  local -a vals=(npm: git: 'https://' 'ssh://')
  local -a descs=('npm package' 'git repository' 'URL' 'SSH URL') # shucked: ignore=C001
  compadd -J sources -d descs -- "${vals[@]}"
}

_pi_install_source() {
  case $PREFIX in
  npm:*)
    compset -P 'npm:'
    if [[ -n $PREFIX ]]; then
      _pi_packages_complete
    else
      _pi_packages_complete '' 40
    fi
    ;;
  git:* | https://* | ssh://*)
    return
    ;;
  .* | /* | \~*)
    _files -/
    ;;
  '')
    _pi_packages_complete 'npm:' 25
    ((${#_pi_package_lines})) || _pi_install_schemes
    ;;
  *)
    _pi_packages_complete 'npm:'
    ;;
  esac
}

_pi_update_target() {
  compadd -S '' -- self pi
  _pi_package_sources
}

_pi_pkg_cmd() {
  local sub=$1
  case $sub in
  remove | uninstall)
    _arguments -s -S \
      '(-l --local)'{-l,--local}'[Remove from project settings (.pi/settings.json)]' \
      '(-a --approve)'{-a,--approve}'[Trust project-local files for this command]' \
      '(-na --no-approve)'{-na,--no-approve}'[Ignore project-local files for this command]' \
      '1:source:_pi_package_sources'
    ;;
  *)
    _arguments -s -S \
      '(-l --local)'{-l,--local}'[Install project-locally (.pi/settings.json)]' \
      '(-a --approve)'{-a,--approve}'[Trust project-local files for this command]' \
      '(-na --no-approve)'{-na,--no-approve}'[Ignore project-local files for this command]' \
      '1:source:_pi_install_source'
    ;;
  esac
}

_pi_update() {
  _arguments -s -S \
    '--self[Update pi only]' \
    '--extensions[Update installed packages only]' \
    '--models[Refresh model catalogs only]' \
    '--all[Update pi and installed packages]' \
    '--extension=[Update one package only]:source:_pi_package_sources' \
    '--force[Reinstall pi even when already latest]' \
    '(-a --approve)'{-a,--approve}'[Trust project-local files for this command]' \
    '(-na --no-approve)'{-na,--no-approve}'[Ignore project-local files for this command]' \
    '1:target:_pi_update_target'
}

_pi_list() {
  _arguments -s -S \
    '(-a --approve)'{-a,--approve}'[Trust project-local files for this command]' \
    '(-na --no-approve)'{-na,--no-approve}'[Ignore project-local files for this command]'
}

_pi_config() {
  _arguments -s -S \
    '(-l --local)'{-l,--local}'[Edit project overrides (.pi/settings.json)]' \
    '(-a --approve)'{-a,--approve}'[Trust project-local files with -l]' \
    '(-na --no-approve)'{-na,--no-approve}'[Ignore project-local files with -l]'
}

_pi_auth() {
  local context state state_descr line ret=1
  typeset -A opt_args
  _arguments -s -S -C \
    '1: :->authcmd' \
    '*:: :->authargs' && ret=0
  case $state in
  authcmd)
    _describe -t commands 'auth command' _pi_auth_commands && ret=0
    ;;
  authargs)
    case $line[1] in
    print-api-key | print-bearer-token | check)
      _arguments -s -S \
        '--provider=[Provider name]:provider:_pi_providers' \
        '--model=[Model pattern or ID]:model:_pi_model_arg' \
        '--min-expiry=[Minimum remaining validity]:duration:' \
        '--json[Emit JSON]' \
        '--credentials[Include the credential]' \
        '--no-refresh[Do not refresh expired credentials]' && ret=0
      ;;
    esac
    ;;
  esac
  return ret
}

_pi() {
  local context state state_descr line ret=1
  typeset -A opt_args
  _arguments -s -S -C \
    '(-h --help)'{-h,--help}'[Show help]' \
    '(-v --version)'{-v,--version}'[Show version number]' \
    '--provider=[Provider name]:provider:_pi_providers' \
    '--model=[Model pattern or ID]:model:_pi_model_arg' \
    '--api-key=[API key]:key:' \
    '--system-prompt=[System prompt]:text:' \
    '--append-system-prompt=[Append text or file to the system prompt]:text:' \
    '--mode=[Output mode]:mode:(text json rpc)' \
    '(-p --print)'{-p,--print}'[Non-interactive mode: process prompt and exit]' \
    '(-c --continue)'{-c,--continue}'[Continue previous session]' \
    '(-r --resume)'{-r,--resume}'[Select a session to resume]' \
    '--session=[Session file or partial UUID]:session:_pi_sessions' \
    '--session-id=[Exact project session ID]:id:' \
    '--fork=[Fork a session file or partial UUID]:session:_pi_sessions' \
    '--session-dir=[Directory for session storage and lookup]:directory:_files -/' \
    '--no-session[Do not save the session]' \
    '(-n --name)'{-n,--name}'=[Session display name]:name:' \
    '--models=[Model patterns for Ctrl+P cycling]:model:_pi_models_arg' \
    '(-nt --no-tools)'{-nt,--no-tools}'[Disable all tools by default]' \
    '(-nbt --no-builtin-tools)'{-nbt,--no-builtin-tools}'[Disable built-in tools but keep extension tools]' \
    '(-t --tools)'{-t,--tools}'=[Comma-separated allowlist of tool names]:tools:_pi_tools' \
    '(-xt --exclude-tools)'{-xt,--exclude-tools}'=[Comma-separated denylist of tool names]:tools:_pi_tools' \
    '--thinking=[Thinking level]:level:(off minimal low medium high xhigh max)' \
    '(-e --extension)'{-e,--extension}'=[Load an extension file]:extension file:_files' \
    '(-ne --no-extensions)'{-ne,--no-extensions}'[Disable extension discovery]' \
    '--skill=[Load a skill file or directory]:skill:_files' \
    '(-ns --no-skills)'{-ns,--no-skills}'[Disable skills discovery and loading]' \
    '--prompt-template=[Load a prompt template file or directory]:template:_files' \
    '(-np --no-prompt-templates)'{-np,--no-prompt-templates}'[Disable prompt template discovery]' \
    '--theme=[Load a theme file or directory]:theme:_files' \
    '--use-theme=[Initial interactive theme for this run]:theme:_pi_themes' \
    '--no-themes[Disable theme discovery and loading]' \
    '(-nc --no-context-files)'{-nc,--no-context-files}'[Disable AGENTS.md and CLAUDE.md discovery]' \
    '--export=[Export a session file to HTML and exit]:file:_files' \
    '--list-models::search:_pi_models' \
    '--verbose[Force verbose startup]' \
    '--tui-mode=[TUI mode]:mode:(regular fullscreen)' \
    '(-a --approve)'{-a,--approve}'[Trust project-local files for this run]' \
    '(-na --no-approve)'{-na,--no-approve}'[Ignore project-local files for this run]' \
    '--offline[Disable startup network operations]' \
    '1: :->command' \
    '*:: :->args' && ret=0

  case $state in
  command)
    if [[ $words[CURRENT] == @* ]]; then
      _files -P '@'
    else
      _describe -t commands 'pi command' _pi_commands && ret=0
    fi
    ;;
  args)
    if [[ $words[CURRENT] == @* ]]; then
      _files -P '@'
      return ret
    fi
    case $line[1] in
    install | remove | uninstall) _pi_pkg_cmd "$line[1]" && ret=0 ;;
    update) _pi_update && ret=0 ;;
    list) _pi_list && ret=0 ;;
    config) _pi_config && ret=0 ;;
    auth) _pi_auth && ret=0 ;;
    esac
    ;;
  esac
  return ret
}

compdef _pi pi

# fzf-tab re-sorts every candidate alphabetically unless the `sort` style is
# false for the completion context, which would bury the most popular packages
# behind @scope/... names. Keep the order produced above.
zstyle ':completion:complete:pi:*' sort false

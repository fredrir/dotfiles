zmodload zsh/datetime 2>/dev/null
zmodload zsh/stat 2>/dev/null

typeset -gA _node_pkg_suggest_cache
typeset -gA _node_pkg_globals
typeset -gA _node_pkg_orig

typeset -g _node_pkg_cache_dir="$DOTFILES_ZSH_CACHE/node-packages"
typeset -g _node_pkg_cache_ttl=86400

typeset -ga _node_pkg_install_subs=(
  install i add a
  in ins inst insta instal
  isnt isnta isntal isntall
  install-test install-ci-test it cit
)

typeset -ga _node_pkg_flags=(
  '-g:install globally'
  '--global:install globally'
  '-S:save as a dependency'
  '--save:save as a dependency'
  '-D:save as a dev dependency'
  '--save-dev:save as a dev dependency'
  '-O:save as an optional dependency'
  '--save-optional:save as an optional dependency'
  '-P:save as a peer dependency'
  '--save-peer:save as a peer dependency'
  '-E:save exact versions'
  '--save-exact:save exact versions'
  '--no-save:do not write to package.json'
  '--no-package-lock:do not create a lockfile'
  '--dry-run:report without installing'
  '-f:force'
  '--force:force'
  '-w:add to the given workspace'
  '--workspace:add to the given workspace'
  '--ignore-scripts:skip lifecycle scripts'
  '--production:skip devDependencies'
  '--frozen-lockfile:do not update the lockfile'
  '--silent:suppress output'
  '--verbose:verbose output'
)

_node_pkg_registry_search() {
  local prefix=$1
  curl -fsS --connect-timeout 1 --max-time 2 \
    --get \
    --data-urlencode 'size=25' \
    --data-urlencode "text=$prefix" \
    'https://registry.npmjs.org/-/v1/search' 2>/dev/null |
    jq -r '.objects[]?.package.name' 2>/dev/null
}

_node_pkg_suggest() {
  local prefix=$1 file out r
  local -a results
  [[ -n $prefix ]] || return 0

  if ((${+_node_pkg_suggest_cache[$prefix]})); then
    print -r -- "${_node_pkg_suggest_cache[$prefix]}"
    return 0
  fi

  file="$_node_pkg_cache_dir/${prefix//[^A-Za-z0-9@._-]/_}"

  if [[ -r $file ]] && ((EPOCHSECONDS - $(zstat +mtime -- "$file") < _node_pkg_cache_ttl)); then
    out=$(<"$file")
  else
    if [[ $prefix != @* ]] && (($+commands[bun])); then
      results=("${(@f)$(SHELL=zsh bun getcompletes a "$prefix" 2>/dev/null)}")
    fi
    results+=("${(@f)$(_node_pkg_registry_search "$prefix")}")

    out=''
    for r in "${(@)results:#}"; do
      [[ $'\n'$out == *$'\n'"$r"* ]] && continue
      out+="$r"$'\n'
    done
    out=${out%$'\n'}

    if [[ -n $out ]]; then
      mkdir -p -- "$_node_pkg_cache_dir"
      print -r -- "$out" >"$file"
    fi
  fi

  _node_pkg_suggest_cache[$prefix]=$out
  print -r -- "$out"
}

_node_pkg_history() {
  local histfile=${HISTFILE:-$HOME/.zsh_history}
  local -x LC_ALL=C
  [[ -r $histfile ]] || return 0
  tail -n 2000 -- "$histfile" 2>/dev/null |
    awk '{ line[NR] = $0 } END { for (i = NR; i > 0; i--) print line[i] }' |
    sed -E 's/^: [0-9]+:[0-9]+;//' |
    grep -aE '^ *(npm|pnpm|pn|bun|yarn) +(i|install|add|a) ' |
    sed -E 's/^ *(npm|pnpm|pn|bun|yarn) +(i|install|add|a) +//' |
    tr ' ' '\n' |
    grep -avE '^-|^$|^[./~]|=' |
    sed -E 's/@[^@/]*$//' |
    awk '!seen[$0]++ && length($0)'
}

_node_pkg_local_deps() {
  local dir=$PWD
  while [[ $dir != / ]]; do
    if [[ -f $dir/package.json ]]; then
      jq -r '((.dependencies // {}) + (.devDependencies // {}) +
              (.optionalDependencies // {}) + (.peerDependencies // {}))
             | keys[]' "$dir/package.json" 2>/dev/null
      return 0
    fi
    dir=${dir:h}
  done
}

_node_pkg_list_modules() {
  local dir=$1 entry sub
  for entry in "$dir"/*(N); do
    [[ -d $entry ]] || continue
    if [[ ${entry:t} == @* ]]; then
      for sub in "$entry"/*(N); do
        [[ -d $sub ]] && print -r -- "${entry:t}/${sub:t}"
      done
    else
      print -r -- "${entry:t}"
    fi
  done
}

_node_pkg_globals_for() {
  local mgr=$1 root out=''
  if ((${+_node_pkg_globals[$mgr]})); then
    print -r -- "${_node_pkg_globals[$mgr]}"
    return 0
  fi

  case $mgr in
  npm) root=$(npm root -g 2>/dev/null) ;;
  pnpm) root=$(pnpm root -g 2>/dev/null) ;;
  bun) root=${BUN_INSTALL:-$HOME/.bun}/install/global/node_modules ;;
  esac

  [[ -n $root && -d $root ]] && out=$(_node_pkg_list_modules "$root")
  _node_pkg_globals[$mgr]=$out
  print -r -- "$out"
}

_node_pkg_downloaded() {
  local index=$HOME/.npm/_cacache/index-v5 file=$_node_pkg_cache_dir/.downloaded out
  local -x LC_ALL=C
  [[ -d $index ]] || return 0

  if [[ -r $file ]] && ((EPOCHSECONDS - $(zstat +mtime -- "$file") < _node_pkg_cache_ttl)); then
    cat -- "$file"
    return 0
  fi

  if has_cmd rg; then
    out=$(rg -a -o --no-filename 'registry\.npmjs\.org/[^"]*' "$index" 2>/dev/null)
  else
    out=$(grep -rhoa 'registry\.npmjs\.org/[^"]*' "$index" 2>/dev/null)
  fi
  out=$(print -r -- "$out" |
    sed 's|.*registry\.npmjs\.org/||; s|/-/.*||' |
    sed 's|%2[fF]|/|g; s|%40|@|g' |
    grep -av '^[-.]' | grep -av '^$' | sort -u)
  [[ -n $out ]] || return 0

  mkdir -p -- "$_node_pkg_cache_dir"
  print -r -- "$out" >"$file"
  print -r -- "$out"
}

_node_pkg_offer() {
  local mgr=$1 prefix=$PREFIX # shucked: ignore=C156
  local -a deps registry cached
  local global=0 word

  case $prefix in
  -*)
    _describe -t options 'option' _node_pkg_flags
    return 0
    ;;
  .* | /* | \~*)
    _files
    return 0
    ;;
  esac

  for word in "${(@)words[3,-1]}"; do
    [[ $word == -g || $word == --global ]] && global=1
  done

  if ((global)); then
    deps=("${(@f)$(_node_pkg_globals_for "$mgr")}")
  else
    deps=("${(@f)$(_node_pkg_local_deps)}")
  fi
  deps+=("${(@f)$(_node_pkg_history)}")
  deps=("${(@)deps:#}")

  if [[ -n $prefix ]]; then
    registry=("${(@f)$(_node_pkg_suggest "$prefix")}")
    registry=("${(@)registry:#}")
  elif ((!global)); then
    cached=("${(@f)$(_node_pkg_downloaded)}")
    cached=("${(@)cached:#}")
  fi

  ((${#deps})) && _describe -t dependencies "$mgr dependency" deps
  ((${#cached})) && _describe -t cached 'recently used' cached
  ((${#registry})) && _describe -t registry 'npm package' registry
}

_node_pkg_dispatch() {
  local mgr=$1 orig=${_node_pkg_orig[$1]} sub=${words[2]}

  if ((CURRENT > 2)) && ((${_node_pkg_install_subs[(I)$sub]})); then
    _node_pkg_offer "$mgr"
  elif [[ -n $orig ]]; then
    "$orig"
  fi
}

_node_pkg_npm() { _node_pkg_dispatch npm; }
_node_pkg_pnpm() { _node_pkg_dispatch pnpm; }
_node_pkg_bun() { _node_pkg_dispatch bun; }

_node_pkg_register() {
  local mgr=$1 fn
  has_cmd "$mgr" || return 0

  fn=${_comps[$mgr]:-}
  if [[ -n $fn && ${functions[$fn]:-} == *autoload* ]]; then
    { "$fn"; } 2>/dev/null
    fn=${_comps[$mgr]:-$fn}
  fi
  _node_pkg_orig[$mgr]=$fn

  compdef "_node_pkg_$mgr" "$mgr"
}

_node_pkg_register npm
_node_pkg_register pnpm
_node_pkg_register bun

if [[ -n ${_comps[pn]:-} ]]; then
  compdef _node_pkg_pnpm pn
fi

# zcomp: the binary decides what to offer; this only hands its answer to the completion system.
typeset -gA _zcomp_orig

_zcomp_search() {
  local -a rows selected args fields
  local line skip='' value answer reload
  for line in "${_zcomp_lines[@]}"; do
    fields=("${(@ps:\t:)line}")
    case $fields[1] in
    skip) skip=$fields[2] ;;
    item)
      value=$skip$fields[2]
      [[ -z $skip || $value == "$PREFIX"* ]] || continue
      rows+=("$value"$'\t'"$skip${fields[3]:-$fields[2]}")
      ;;
    esac
  done
  args=("$commands[zcomp]" complete --picker --color --command="$service" --current="$CURRENT")
  # fzf quotes {q}; quote every original shell word before building its reload command.
  reload="${(j: :)${(@q)args}} --prefix={q} -- ${(j: :)${(@q)words}}"
  answer=$({ ((${#rows})) && print -rl -- "${rows[@]}"; } | FZF_DEFAULT_OPTS='' SHELL=$commands[zsh] command fzf \
    --ansi --disabled --no-sort --delimiter=$'\t' --with-nth=2.. \
    --height=60% --layout=reverse --multi --query="$PREFIX" \
    --bind="start:reload($reload)" \
    --bind="change:unbind(enter)+reload(sleep 0.15; $reload),load:rebind(enter)" \
    --bind='ctrl-space:toggle,tab:down,btab:up')
  local ret=$?
  ((ret == 0)) && [[ -n $answer ]] || return 1
  for line in "${(@f)answer}"; do
    selected+=("${line%%$'\t'*}")
  done
  # fzf-tab still captures the chosen specs so it can apply them normally.
  compadd -U -V packages -- "${selected[@]}"
  compstate[list]=''
  compstate[insert]=all
  typeset -g _zcomp_picked=1
}

_zcomp_fzf() {
  emulate -L zsh -o extended_glob
  local -a rows selected
  local -A originals
  local row plain answer
  local -i i
  rows=("${(@f)$(command cat)}")
  if (( ${_zcomp_picked:-0} )); then
    # Multiple specs were already selected in the live picker.
    local option
    for option in "$@"; do
      [[ $option == --header-lines=* ]] && rows[1,${option#*=}]=()
    done
    print -rl -- '' '' "${rows[@]}"
    return 0
  fi
  for row in "${rows[@]}"; do
    plain=${row//$'\e'\[[0-9\;:]#m/}
    originals[$plain]=$row
  done
  answer=$(print -rl -- "${rows[@]}" | command fzf "$@")
  local ret=$?
  [[ -n $answer ]] || return ret
  selected=("${(@f)answer}")
  # fzf --ansi strips SGR codes from its answer; fzf-tab looks up the original display.
  # The first two lines are the query and expect key, not selected rows.
  for ((i = 3; i <= ${#selected}; i++)); do
    row=$selected[i]
    selected[i]=${originals[$row]:-$row}
  done
  print -rl -- "${selected[@]}"
  return ret
}

_zcomp_add() {
  ((${#_zcomp_values})) || return 1
  local -a group_flags compadd_flags expl # shucked: ignore=C001
  local option
  for option in "${_zcomp_options[@]}"; do
    case $option in
    unsorted) group_flags+=(-V) ;;
    lines) compadd_flags+=(-l) ;;
    suffix=*) compadd_flags+=(-S "${option#suffix=}") ;;
    removable) compadd_flags+=(-q) ;;
    prefix=*) compadd_flags+=(-P "${option#prefix=}") ;;
    replace) compadd_flags+=(-U) ;;
    esac
  done
  _wanted "${group_flags[@]}" "$_zcomp_tag" expl "$_zcomp_label" \
    compadd "${compadd_flags[@]}" -d _zcomp_displays -a _zcomp_values
  local ret=$?
  _zcomp_values=() _zcomp_displays=()
  return ret
}

_zcomp_complete() {
  typeset -g _zcomp_picked=0
  local -a _zcomp_lines _zcomp_values _zcomp_displays _zcomp_options fields
  local _zcomp_tag _zcomp_label line orig=${_zcomp_orig[$service]:-} ret=1
  # Colors only where they render: fzf-tab passes display strings to fzf with --ansi.
  local color=${${functions[fzf-tab-complete]:+--color}:-} # shucked: ignore=C001
  local -a initial
  if (( IN_FZF_TAB && $+commands[fzf] )); then
    case $service in
    bun|bunx|npm|npx|pnpm|pn|pnpx|pnx|yarn) initial=(--cached) ;; # shucked: ignore=C001
    esac
  fi
  _zcomp_lines=("${(@f)$(command zcomp complete --command="$service" --current="$CURRENT" \
    --prefix="$PREFIX" $color "${initial[@]}" -- "${words[@]}" 2>/dev/null)}")

  if (( IN_FZF_TAB && $+commands[fzf] )) && (( ${_zcomp_lines[(Ie)search]} )); then
    # Cancellation must not fall through to another completion attempt.
    _zcomp_search && return 0
    compstate[list]=''
    compstate[insert]=''
    _ftb_finish=1
    return 0
  fi

  for line in "${_zcomp_lines[@]}"; do
    fields=("${(@ps:\t:)line}")
    case $fields[1] in
    skip) compset -P "${(b)fields[2]}" ;;
    group)
      _zcomp_add && ret=0
      _zcomp_tag=$fields[2] _zcomp_label=$fields[3]
      _zcomp_options=("${(@)fields[4,-1]}")
      ;;
    item)
      _zcomp_values+=("$fields[2]")
      _zcomp_displays+=("${fields[3]:-$fields[2]}")
      ;;
    files)
      _zcomp_add && ret=0
      _files && ret=0
      ;;
    dirs)
      _zcomp_add && ret=0
      _files -/ && ret=0
      ;;
    message)
      _zcomp_add && ret=0
      _message -r "$fields[2]"
      ;;
    delegate)
      _zcomp_add && ret=0
      if [[ -n $orig ]]; then
        "$orig" && ret=0
        return ret
      fi
      ;;
    esac
  done
  _zcomp_add && ret=0
  return ret
}

_zcomp_register() {
  local command=$1 fn
  (($+commands[$command])) || return 0
  fn=${_comps[$command]:-}
  # Loading an autoloaded completer first lets it claim its command before zcomp takes over.
  if [[ -n $fn && $fn != _zcomp_complete && ${functions[$fn]:-} == *autoload* ]]; then
    { "$fn"; } 2>/dev/null
    fn=${_comps[$command]:-$fn}
  fi
  [[ $fn == _zcomp_complete ]] || _zcomp_orig[$command]=$fn
  compdef _zcomp_complete "$command"
  # Ranked candidates keep the order zcomp gives them, fzf-tab included.
  zstyle ":completion:*:*:$command:*:(registry|popular|dist-tags|versions|sessions|gallery|thinking|values|scripts|schemes)" sort false
  zstyle ":fzf-tab:complete:$command:*" prefix ''
  # The query is the typed word; a shared prefix of the colored display lines is only escape codes.
  zstyle ":fzf-tab:complete:$command:*" query-string input
  zstyle ":fzf-tab:complete:$command:*" fzf-command _zcomp_fzf
}
() {
  local command
  for command in {{COMMANDS}}; do
    _zcomp_register "$command"
  done
}

if (($+functions[defer])); then
  defer command zcomp warm
else
  command zcomp warm &!
fi

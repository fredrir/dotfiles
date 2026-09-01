
# clap cannot express "offer this option only after --info/--watch". Keep its
# generated function for subcommands and layer a state-aware root completer on
# top so invalid hwire modes are not suggested.
functions[_hwire_clap]=$functions[_hwire]

_hwire() {
  local word info_mode=0
  for word in "${words[@]}"; do
    if [[ $word == --info || ($word == -*i* && $word != --*) ]]; then
      info_mode=1
      break
    fi
  done

  if [[ ${words[2]-} == (serve|help) ]]; then
    _hwire_clap "$@"
    return
  fi

  if (( info_mode )); then
    local -a info_arguments
    info_arguments=(
      '(-i --info)'{-i,--info}'[Inspect the current connection or routes to HOST]'
      '(-v --verbose)'{-v,--verbose}'[Show full route, session, and SSH diagnostics]'
      '--watch[Watch connection information and report meaningful changes]'
      '--json[Print connection information as JSON]'
      '--color=[Control colored output]:when:(auto always never)'
      '(-h --help)'{-h,--help}'[Print help]'
      '(-V --version)'{-V,--version}'[Print version]'
      '*:SSH host:_hosts'
    )
    if (( ${words[(I)--watch]} )); then
      info_arguments+=(
        '--interval=[Set the watch refresh interval]:seconds:'
        '--notify[Ring the terminal bell when the preferred route changes]'
      )
    fi
    _arguments -s -S $info_arguments
    return
  fi

  local -a measure_arguments
  measure_arguments=(
    '(-r --route -a --all -b --both --at -i --info)'{-r,--route}'=[Select the route to measure]:route:(cable wifi lan tailscale)' \
    '(-a --all -r --route -b --both --at -i --info)'{-a,--all}'[Measure every available route sequentially]' \
    '(-b --both -r --route -a --all --at -i --info)'{-b,--both}'[Compatibility spelling for --all]' \
    '(-t --time -i --info)'{-t,--time}'=[Set transfer duration]:seconds:' \
    '(-P --streams -i --info)'{-P,--streams}'=[Set concurrent transfer connections]:count:' \
    '(-n --samples -i --info)'{-n,--samples}'=[Limit round trips timed]:count:' \
    '(-l --latency -u --up -d --down -i --info)'{-l,--latency}'[Measure latency without transfers]' \
    '(-u --up -d --down -l --latency -i --info)'{-u,--up}'[Transfer only to the peer]' \
    '(-d --down -u --up -l --latency -i --info)'{-d,--down}'[Transfer only from the peer]' \
    '(-r --route -a --all -b --both -i --info)--at=[Measure an existing server]:address and port:' \
    '--json[Print measurements as JSON]' \
    '--color=[Control colored output]:when:(auto always never)' \
    '(-i --info -r --route -a --all -b --both -t --time -P --streams -n --samples -l --latency -u --up -d --down --at --token)'{-i,--info}'[Inspect the current connection or routes]' \
    '--completions=[Generate shell completions]:shell:(bash elvish fish powershell zsh)' \
    '(-h --help)'{-h,--help}'[Print help]' \
    '(-V --version)'{-V,--version}'[Print version]' \
    '1:command:(serve help)'
  )
  if (( ${words[(I)--at]} )); then
    measure_arguments+=('--token=[Use the server authentication token]:hex token:')
  fi
  _arguments -s -S $measure_arguments
}

compdef _hwire hwire

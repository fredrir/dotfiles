git() {
  case "$1:$#" in
  diff:1) command lazygit ;;
  log:1) command lazygit log ;;
  *) command git "$@" ;;
  esac
}

lls() {
  local -a links=(*(ND@))

  ((${#links[@]})) || return 0

  eza -lh \
    --no-permissions \
    --no-filesize \
    --no-user \
    --no-time \
    -- "${links[@]}"
}

# Which route ssh will take, and whether a master is already up for it.
#
# Every failure in the cabled-first setup degrades quietly to a working but
# much slower path: nc missing, cable unplugged, no DHCP lease yet, sshd
# restarting. All of them look identical from the outside, which is that
# things still work. This resolves the config the same way the next ssh will,
# without opening a connection, so the fallback stops being invisible.
hpath() {
  emulate -L zsh

  local usage="usage: hpath [--json] [host ...]"
  local help="$usage

Report the route ssh would take for a host without connecting to it. The
ordered Match exec probes in ~/.ssh/config.d/05-* through 07-* run during
config resolution, so this is the same decision the next \`ssh <host>\` will
make: cable, direct Wi-Fi, regular LAN, then Tailscale.

With no host, reports the other machine: archie from macie, macie from archie.

The route column names the resolved transport and the source binding or
filtered LAN proxy that proves it. A host outside the archie/macie pair is
reported as Tailscale unless its resolved config identifies another route.

Options:
      --json  Machine-readable
  -h, --help  Show this help

See also: hwire, which measures what a route is actually worth."

  local json=0
  while (($#)); do
    case "$1" in
    --json)
      json=1
      ;;
    -h | --help)
      print -r -- "$help"
      return 0
      ;;
    --)
      shift
      break
      ;;
    -*)
      print -u2 -r -- "hpath: unknown option: $1"
      print -u2 -r -- "$usage"
      return 2
      ;;
    *)
      break
      ;;
    esac
    shift
  done

  (($+commands[ssh])) || {
    print -u2 -r -- "hpath: ssh is not installed"
    return 127
  }

  local -a hosts=("$@")
  if ((!$#hosts)); then
    case "$OSTYPE" in
    darwin*) hosts=(archie) ;;
    linux*) hosts=(macie) ;;
    *)
      print -u2 -r -- "hpath: unsupported operating system: $OSTYPE"
      return 1
      ;;
    esac
  fi

  # All declared up here: re-running `local` on an existing local inside the
  # loop makes zsh print the parameter instead of quietly redeclaring it.
  local exit_status=0
  local -a resolved names targets routes masters
  local host line key value hostname bound proxy master route route_id master_json

  for host in "${hosts[@]}"; do
    resolved=(${(f)"$(ssh -G "$host" 2>/dev/null)"})
    if ((!$#resolved)); then
      print -u2 -r -- "hpath: no config resolved for $host"
      exit_status=1
      continue
    fi

    hostname=
    bound=
    proxy=
    for line in "${resolved[@]}"; do
      key=${line%% *}
      value=${line#* }
      case "$key" in
      hostname) hostname=$value ;;
      bindinterface) bound=$value ;;
      bindaddress) [[ -n "$bound" ]] || bound=$value ;;
      proxycommand) proxy=$value ;;
      esac
    done

    case "$hostname" in
    10.77.77.*)
      route="cable via ${bound:-unknown}"
      route_id=cable
      ;;
    10.77.78.*)
      route="wifi via ${bound:-unknown}"
      route_id=wifi
      ;;
    *)
      if [[ "$proxy" == *home-lan-connect* ]]; then
        route="lan via filtered mDNS"
        route_id=lan
      else
        route=tailscale
        route_id=tailscale
      fi
      ;;
    esac

    if ssh -O check "$host" >/dev/null 2>&1; then
      master=up
      master_json=true
    else
      master=none
      master_json=false
    fi

    if ((json)); then
      printf '{"host":"%s","hostname":"%s","route":"%s","bound":"%s","master":%s}\n' \
        "$host" "$hostname" "$route_id" "$bound" "$master_json"
    else
      names+=("$host")
      targets+=("$hostname")
      routes+=("$route")
      masters+=("$master")
    fi
  done

  if ((!json)) && (($#names)); then
    local -i name_width=0 target_width=0 route_width=0
    local field
    for field in "${names[@]}"; do (($#field > name_width)) && name_width=$#field; done
    for field in "${targets[@]}"; do (($#field > target_width)) && target_width=$#field; done
    for field in "${routes[@]}"; do (($#field > route_width)) && route_width=$#field; done

    local -i i
    for ((i = 1; i <= $#names; i++)); do
      printf '%-*s  %-*s  %-*s  master %s\n' \
        "$name_width" "$names[i]" \
        "$target_width" "$targets[i]" \
        "$route_width" "$routes[i]" \
        "$masters[i]"
    done
  fi

  return $exit_status
}

unalias cd 2>/dev/null
cd() {
  if (($# != 1)) || [[ "$1" == -* ]] || [[ -d "$1" ]]; then
    builtin cd "$@"
    return
  fi

  setopt localoptions extendedglob

  local pattern="(#i)${(b)1}"
  local -a matches=( ${~pattern}(N-/) )

  case $#matches in
  1)
    builtin cd -- "$matches[1]"
    ;;
  0)
    builtin cd -- "$1"
    ;;
  *)
    print -ru2 -- "cd: ambiguous case-insensitive match: ${matches[*]}"
    return 1
    ;;
  esac
}

alias cd='nocorrect cd'
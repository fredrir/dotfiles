list_profiles() {
  ( cd "$ENVDIR" && find . -name manifest | sed 's|^\./||; s|/manifest$||' | LC_ALL=C sort )
}

detect_linux_platform() {
  local id="" id_like="" key value
  [ -r /etc/os-release ] || return 1
  while IFS='=' read -r key value; do
    value="${value#\"}"
    value="${value%\"}"
    value="${value#\'}"
    value="${value%\'}"
    case "$key" in
      ID) id="$value" ;;
      ID_LIKE) id_like="$value" ;;
    esac
  done < /etc/os-release
  case "$id" in
    arch) printf '%s\n' arch-linux; return 0 ;;
    ubuntu) printf '%s\n' ubuntu; return 0 ;;
  esac
  case " $id_like " in
    *' arch '*) printf '%s\n' arch-linux ;;
    *' ubuntu '*) printf '%s\n' ubuntu ;;
    *) return 1 ;;
  esac
}

detect_platform() {
  case "$(uname -s)" in
    Darwin) printf '%s\n' macos ;;
    Linux) detect_linux_platform ;;
    *) return 1 ;;
  esac
}

detect_installed_desktops() {
  local desktops=()
  if command -v plasmashell >/dev/null 2>&1 || command -v startplasma-wayland >/dev/null 2>&1 || command -v startplasma-x11 >/dev/null 2>&1; then
    desktops+=(kde)
  fi
  if command -v Hyprland >/dev/null 2>&1 || command -v hyprctl >/dev/null 2>&1; then
    desktops+=(hyprland)
  fi
  printf '%s\n' "${desktops[*]}"
}

has_installed_desktop() {
  case " $1 " in
    *" $2 "*) return 0 ;;
    *) return 1 ;;
  esac
}

profile_matches_host() {
  local profile="$1" platform="$2" desktops="$3" group required
  if [ "$platform" = macos ]; then
    [ "$profile" = macos ] || return 1
  else
    [ "${profile%%/*}" = "$platform" ] || return 1
  fi
  while IFS= read -r group || [ -n "$group" ]; do
    group="${group%%#*}"
    group="${group#"${group%%[![:space:]]*}"}"
    group="${group%"${group##*[![:space:]]}"}"
    case "$group" in
      linux/kde) required=kde ;;
      linux/hyprland) required=hyprland ;;
      *) continue ;;
    esac
    has_installed_desktop "$desktops" "$required" || return 1
  done < "$ENVDIR/$profile/manifest"
}

filter_profiles() {
  local platform="$1" desktops="$2" profile
  while IFS= read -r profile; do
    if profile_matches_host "$profile" "$platform" "$desktops"; then
      printf '%s\n' "$profile"
    fi
  done < <(list_profiles)
}

list_relevant_profiles() {
  local platform desktops
  platform="$(detect_platform || true)"
  [ -n "$platform" ] || return 0
  desktops="$(detect_installed_desktops)"
  filter_profiles "$platform" "$desktops"
}

normalize_profile_arg() {
  case "$1" in
    --?*) printf '%s\n' "${1#--}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

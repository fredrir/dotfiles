orphans() {
  pacman -Qtdq
}

rmorphans() {
  local -a pkgs
  pkgs=("${(@f)$(pacman -Qtdq)}")

  if (( ${#pkgs} == 0 )); then
    print "No orphan packages."
    return 0
  fi

  print "Removing ${#pkgs} orphan package(s):"
  printf '  %s\n' "${pkgs[@]}"
  sudo pacman -Rns -- "${pkgs[@]}"
}

whoowns() {
  local target=${1:-$(which "$2" 2>/dev/null)}

  if [[ -z "$target" ]]; then
    print "usage: whoowns FILE"
    return 1
  fi

  pacman -Qo "$target"
}
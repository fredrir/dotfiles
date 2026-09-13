dgpps() { # Read as dotfile workspace (d) git (g) push (pp) sync (s)
  local gppf_bin="$DOTFILES_COMPILED/gppf"
  local -a targets=()

  case $HOST in
  macie)
    targets+=("archie")
    ;;
  archie)
    targets+=("macie")
    ;;
  esac

  "$gppf_bin" || return

  local relative_dir=${PWD#"$HOME"/}

  local i
  for i in "${targets[@]}"; do
    ssh -t "$i" "cd ~/${(q)relative_dir} && git pull --autostash"
  done
}

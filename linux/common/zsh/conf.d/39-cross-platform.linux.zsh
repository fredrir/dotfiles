dclip() {
  if (("$XDG_SESSION_TYPE" == "tty")); then
    wl-copy
  else
    echo "No clipboard"
  fi
}

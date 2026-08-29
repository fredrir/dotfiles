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
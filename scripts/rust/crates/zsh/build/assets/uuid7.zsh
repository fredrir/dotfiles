# `atuin uuid`: UUIDv7 as `uuid`'s now_v7 makes it in a new process; 41-bit counter seed, 32 random bits
_zsh_build_uuid7() {
  emulate -L zsh -o no_multibyte
  typeset -g _zsh_build_uuid=
  local random byte
  local -a now bytes
  zmodload -F zsh/datetime p:epochtime && zmodload -F zsh/system b:sysread || return
  now=($epochtime)
  sysread -s 10 random </dev/urandom || return
  for byte in "${(@s::)random}"; do  # shucked: ignore=C001
    bytes+=($(( #byte )))
  done
  (( $#bytes == 10 )) || return
  printf -v _zsh_build_uuid '%012x7%x%02x%02x%02x%02x%02x%02x%02x%02x%02x' \
    $(( now[1] * 1000 + now[2] / 1000000 )) $(( bytes[1] & 7 )) $bytes[2] \
    $(( bytes[3] & 0x3f | 0x80 )) $bytes[4,10]
}
functions -M _zsh_build_uuid7 0 0

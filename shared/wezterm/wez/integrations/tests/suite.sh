#!/bin/sh
set -eu

tests_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$tests_dir/../../../../.." && pwd)
cd "$repo_root"

run() {
  printf '  %-36s' "$1"
  shift
  "$@"
  printf ' ok\n'
}

echo 'VS Code remote bridge test suite'
run 'zsh request bridge' /bin/zsh "$tests_dir/vscode_remote.zsh"
run 'WezTerm request handler' lua "$tests_dir/vscode_remote.lua"
run 'legacy integration registration' env -u DMUX_WEZ_FIRST lua "$tests_dir/init.lua"
run 'managed integration registration' env DMUX_WEZ_FIRST=1 lua "$tests_dir/init.lua"


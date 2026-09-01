package = "term-config"
version = "scm-1"

source = {
  url = "git+ssh://git@github.com/fredrir/dotfiles.git",
}

description = {
  license = "MIT",
}

dependencies = {
  "lua >= 5.4",
  "wezterm-types == 4.3.0-1",
}

build = {
  type = "none",
}

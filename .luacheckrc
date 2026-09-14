files["shared/nvim/**/*.lua"] = {
  std = "luajit",
  globals = { "vim" },
  max_line_length = 120,
}

files["shared/yazi/**/*.lua"] = {
  std = "luajit",
  globals = { 
    "ya", "Command", "Modal", "ui", "ps", "producer", "consumer1", "consumer2", 
    "fs", "Tab", "Status", "cx", "Url", "th", "Linemode"
  },
  ignore = { "212", "411", "542", "631" }
}

files["shared/wezterm/**/*.lua"] = {
  std = "lua54",
  ignore = { "211", "631", "143" }
}

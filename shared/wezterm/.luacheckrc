std = "lua54"
cache = true
codes = true

max_line_length = 150
max_comment_line_length = 200

files[".luarocks/config-*.lua"] = {
  globals = { "variables" },
}

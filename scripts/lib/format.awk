function compact(s, out, i, ch, quote, escaped, space) {
  out = ""
  quote = ""
  escaped = 0
  space = 0
  for (i = 1; i <= length(s); i++) {
    ch = substr(s, i, 1)
    if (quote != "") {
      out = out ch
      if (escaped) {
        escaped = 0
      } else if (ch == "\\") {
        escaped = 1
      } else if (ch == quote) {
        quote = ""
      }
    } else if (ch == "\"" || ch == "\047") {
      if (space && length(out)) out = out " "
      space = 0
      quote = ch
      out = out ch
    } else if (ch == " " || ch == "\t") {
      space = 1
    } else {
      if (space && length(out)) out = out " "
      space = 0
      out = out ch
    }
  }
  return out
}

function indentation(level, out, i) {
  out = ""
  for (i = 0; i < level; i++) out = out "    "
  return out
}

function padding(width, out, i) {
  out = ""
  for (i = 0; i < width; i++) out = out " "
  return out
}

function kitty_store(s, pos, key, tail, shortcut) {
  s = compact(s)
  if (kitty_blank && kitty_count) kitty_lines[++kitty_count] = ""
  kitty_blank = 0
  kitty_lines[++kitty_count] = s
  if (s ~ /^#/ || !(pos = index(s, " "))) return
  key = substr(s, 1, pos - 1)
  if (key == "map") {
    tail = substr(s, pos + 1)
    pos = index(tail, " ")
    if (!pos) return
    shortcut = substr(tail, 1, pos - 1)
    if (length(shortcut) > kitty_map_width) kitty_map_width = length(shortcut)
  } else if (length(key) > kitty_key_width) {
    kitty_key_width = length(key)
  }
}

function kitty_print(i, s, pos, key, value, tail, shortcut, action) {
  for (i = 1; i <= kitty_count; i++) {
    s = kitty_lines[i]
    if (s == "" || s ~ /^#/ || !(pos = index(s, " "))) {
      print s
      continue
    }
    key = substr(s, 1, pos - 1)
    value = substr(s, pos + 1)
    if (key != "map") {
      print key padding(kitty_key_width - length(key) + 2) value
      continue
    }
    tail = value
    pos = index(tail, " ")
    if (!pos) {
      print s
      continue
    }
    shortcut = substr(tail, 1, pos - 1)
    action = substr(tail, pos + 1)
    print "map " shortcut padding(kitty_map_width - length(shortcut) + 2) action
  }
}

function hypr_line(s, pos, lhs, rhs) {
  sub(/^[ \t]+/, "", s)
  if (s == "}") indent--
  if (indent < 0) indent = 0
  if (s !~ /^#/ && (pos = index(s, "="))) {
    lhs = substr(s, 1, pos - 1)
    rhs = substr(s, pos + 1)
    sub(/[ \t]+$/, "", lhs)
    sub(/^[ \t]+/, "", rhs)
    if (lhs ~ /^[$[:alnum:]_.:-]+$/) s = lhs " =" (length(rhs) ? " " rhs : "")
  }
  s = indentation(indent) s
  if (s !~ /^[ \t]*#/ && s ~ /\{[ \t]*$/) indent++
  return s
}

{
  sub(/[ \t]+$/, "")
  if (mode == "kitty") {
    if ($0 == "") {
      if (kitty_count) kitty_blank = 1
    } else {
      kitty_store($0)
    }
    next
  }
  if ($0 == "") {
    if (printed) blank = 1
    next
  }
  line = $0
  closing = line
  sub(/^[ \t]+/, "", closing)
  if (blank && !(mode == "hypr" && closing == "}")) print ""
  blank = 0
  if (mode == "hypr") line = hypr_line(line)
  print line
  printed = 1
}

END {
  if (mode == "kitty") kitty_print()
}

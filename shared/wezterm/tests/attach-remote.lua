local callbacks, calls, toasts = {}, {}, {}
local failure, output, pane_domain = nil, "23\n", "localmux"
local metadata = { remote_pane_id = 17 }
local origin = arg[1] == "mac" and "macie" or "archie"
local peer = origin == "macie" and "archie" or "macie"
local homes = { macie = "/Users/fredrir", archie = "/home/fredrir" }
local host = {
  origin = { hostname = origin, home = homes[origin] },
  target = { hostname = peer, home = homes[peer], ip = { { name = "lan" } } },
}
local wezterm = {
  home_dir = homes[origin],
  executable_dir = "/fixture/bin",
  log_error = function() end,
  action = {},
  action_callback = function(callback)
    return callback
  end,
  on = function(name, callback)
    callbacks[name] = callback
  end,
  run_child_process = function(args)
    table.insert(calls, args)
    if args[1]:match "mux%-route$" then
      if failure == "route" then
        return false, "", "unreachable"
      end
      return true, peer .. "-lan\n", ""
    end
    assert(
      args[1] == "/usr/bin/env"
        and args[2] == "WEZTERM_UNIX_SOCKET=" .. homes[origin] .. "/.local/share/wezterm/localmux.sock"
    )
    assert(args[3] == "/fixture/bin/wezterm")
    assert(args[5] == "--prefer-mux" and args[6] == "--no-auto-start")
    if args[7] == failure then
      return false, "", "failed"
    end
    return true, args[7] == "split-pane" and output or "", ""
  end,
}
package.loaded.wezterm = wezterm
package.loaded["domain.hosts"] = host
package.loaded["utils.hwire-session"] = nil
package.preload["utils.hwire-session"] = nil
local attach = dofile "shared/wezterm/utils/attach-remote.lua"
local pane = {
  pane_id = function()
    return 900
  end,
  get_domain_name = function()
    return pane_domain
  end,
  get_metadata = function()
    return metadata
  end,
}
local window = {
  toast_notification = function(_, _, message)
    table.insert(toasts, message)
  end,
}
local function reset()
  calls, toasts, failure, output = {}, {}, nil, "23\n"
end
local function request(target)
  callbacks["user-var-changed"](window, pane, "ATTACH_MUX", "v1:" .. target .. ":42:1.2")
end
local function split_command()
  for _, call in ipairs(calls) do
    if call[7] == "split-pane" then
      return table.concat(call, " ")
    end
  end
end

attach(window, pane)
assert(#toasts == 0 and #calls == 4)
assert(split_command():find("--pane-id 17 --domain-name " .. peer .. "-lan", 1, true))
assert(split_command():find("HWIRE_SESSION=v1:" .. origin .. ":" .. peer .. ":lan:tls", 1, true))
assert(calls[3][7] == "kill-pane" and calls[3][9] == "17")
assert(calls[4][7] == "activate-pane" and calls[4][9] == "23")
reset()
request(origin)
assert(#toasts == 0 and #calls == 3)
assert(split_command():find("--domain-name local", 1, true))
assert(split_command():find("HWIRE_SESSION= zsh", 1, true))
reset()
request "peer"
assert(#toasts == 0 and #calls == 4)
for _, reason in ipairs { "route", "split-pane" } do
  reset()
  failure = reason
  request(peer)
  assert(#toasts == 1)
  for _, call in ipairs(calls) do
    assert(call[7] ~= "kill-pane")
  end
end
for _, invalid in ipairs { "", "garbage", "17\n" } do
  reset()
  output = invalid
  request(peer)
  assert(#toasts == 1 and #calls == 2)
end
reset()
failure = "kill-pane"
request(peer)
assert(#toasts == 1 and #calls == 4)
assert(calls[4][7] == "kill-pane" and calls[4][9] == "23")
reset()
metadata = {}
request(peer)
assert(#calls == 0 and #toasts == 1)
reset()
metadata = { remote_pane_id = 17 }
pane_domain = "other"
request(peer)
assert(#calls == 0 and #toasts == 1)
reset()
request "unknown"
assert(#calls == 0 and #toasts == 0)
print("attach remote: passed (" .. origin .. ")")

local M = {}

local cache = vim.fn.stdpath "cache" .. "/clipboard"
local routes =
  { cable = true, usb = true, wifi = true, direct = true, wireless = true, lan = true, tailscale = true, ts = true }

local function set(name)
  return (vim.env[name] or "") ~= ""
end

-- mirrors scripts/rust/crates/dclip/src/mode.rs
local function mode()
  local this = vim.fn.has "mac" == 1 and "macie" or "archie"
  local peer = this == "macie" and "archie" or "macie"
  local origin, destination, route = (vim.env.HWIRE_SESSION or ""):match "^v1:([^:]*):([^:]*):([^:]*):tls$"
  if origin == peer and destination == this and routes[route] then
    return "mux"
  elseif set "SSH_CONNECTION" or set "SSH_TTY" then
    return "ssh"
  elseif vim.fn.has "mac" == 1 or set "WAYLAND_DISPLAY" or set "DISPLAY" then
    return "native"
  end
  return "terminal"
end

local function dclip(args, stdin)
  local result = vim.system(vim.list_extend({ "dclip" }, args), { stdin = stdin, text = true }):wait()
  if result.code ~= 0 then
    vim.notify(vim.trim(result.stderr), vim.log.levels.WARN)
    return nil
  end
  return result.stdout
end

local function cached()
  return vim.fn.filereadable(cache) == 1 and vim.fn.readfile(cache) or {}
end

-- a fresh yank may not have reached the origin over OSC 52 yet
local function fresh()
  local stat = vim.uv.fs_stat(cache)
  if not stat then
    return false
  end
  local now = vim.uv.clock_gettime "realtime"
  return (now.sec - stat.mtime.sec) + (now.nsec - stat.mtime.nsec) / 1e9 < 1
end

function M.setup()
  local current = mode()
  if (current == "native" or current == "mux") and vim.fn.executable "dclip" == 0 then
    return
  end

  local send = require("vim.ui.clipboard.osc52").copy "+"
  local function copy(lines)
    if current == "native" then
      -- dclip drops one trailing newline; the extra one keeps linewise yanks linewise
      dclip({}, table.concat(lines, "\n") .. "\n")
      return
    end
    pcall(vim.fn.writefile, lines, cache)
    send(lines)
  end

  local function paste()
    if current == "ssh" or current == "terminal" or (current == "mux" and fresh()) then
      return cached()
    end
    local text = dclip { "-o" }
    return text and vim.split(text, "\n", { plain = true }) or {}
  end

  vim.g.clipboard = {
    name = "dclip",
    copy = { ["+"] = copy, ["*"] = copy },
    paste = { ["+"] = paste, ["*"] = paste },
  }
end

return M

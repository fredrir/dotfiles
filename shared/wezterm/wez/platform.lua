local wezterm = require 'wezterm'

local triple = wezterm.target_triple

local M = {
  hostname = wezterm.hostname(),
  is_mac = triple:find 'darwin' ~= nil,
  is_linux = triple:find 'linux' ~= nil,
}

function M.pick(choices)
  if M.is_mac and choices.mac ~= nil then
    return choices.mac
  end
  if M.is_linux and choices.linux ~= nil then
    return choices.linux
  end
  return choices.default
end

return M

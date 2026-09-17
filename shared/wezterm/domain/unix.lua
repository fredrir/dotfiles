local wezterm = require "wezterm"
local ssh_mux = require "domain.ssh-mux"

---@diagnostic disable: missing-fields
---@type UnixDomain[]
local domains = {
  {
    name = "localmux",
    socket_path = wezterm.home_dir .. "/.local/share/wezterm/localmux.sock",
    no_serve_automatically = true,
  },
}

if ssh_mux.supported then
  for name, remote in pairs(ssh_mux.hosts) do
    table.insert(domains, {
      name = name,
      proxy_command = { "ssh", "-T", name, remote.wezterm, "cli", "--prefer-mux", "proxy" },
      no_serve_automatically = true,
      local_pane_layout = true,
      local_echo_threshold_ms = 20,
    })
  end
end

---@type Config
local unix_config = {
  unix_domains = domains,
  default_domain = "localmux",
  default_gui_startup_args = { "connect", "localmux" },
}
---@diagnostic enable: missing-fields

return unix_config

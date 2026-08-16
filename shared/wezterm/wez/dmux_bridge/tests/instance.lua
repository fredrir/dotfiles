package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local epoch = '55555555-5555-4555-8555-555555555555'
local captured

local function domain(name, state, has_panes)
  return {
    name = function()
      return name
    end,
    state = function()
      return state
    end,
    has_any_panes = function()
      return has_panes
    end,
  }
end

local function pane(domain_name, marker)
  return {
    marker = marker,
    get_domain_name = function()
      return domain_name
    end,
  }
end

local valid = pane('dmux-b-usb', {
  gui_pane_id = 91,
  gui_domain = 'dmux-b-usb',
  marker = { server_epoch = epoch },
})
local invalid = pane 'dmux-b-usb'
local sentinel = pane 'dmux-b-usb'

local function window(workspace, panes)
  return {
    get_workspace = function()
      return workspace
    end,
    tabs = function()
      return {
        {
          panes = function()
            return panes
          end,
        },
      }
    end,
  }
end

local domain_rows = {
  domain('dmux-b-usb', 'Attached', true),
  domain('dmux-b-ts', 'Detached', false),
}

local fake_wezterm = {
  json_encode = function(value)
    return value
  end,
  mux = {
    all_domains = function()
      return domain_rows
    end,
    all_windows = function()
      return {
        window('dmux:system:' .. epoch, { sentinel }),
        window('dmux:host:space', { valid, invalid }),
      }
    end,
  },
}
package.preload.wezterm = function()
  return fake_wezterm
end

package.loaded['wez.dmux_bridge.context'] = {
  from_pane = function(value)
    return value.marker
  end,
  marker_context = function(marker)
    return marker.marker
  end,
}
package.loaded['wez.dmux_bridge.fs'] = {
  write_private_atomic = function(path, body)
    captured = { path = path, body = body }
    return true
  end,
}

local instance = require 'wez.dmux_bridge.instance'
local ok, err = instance.heartbeat {
  id = 'gui-42-cafe',
  pid = 42,
  process_start_token = 'start-token',
  paths = { heartbeat = '/runtime/heartbeat.json' },
}
assert(ok and not err)
assert(captured.path == '/runtime/heartbeat.json')
local heartbeat = assert(require('wez.dmux_bridge.json').decode(captured.body))
assert(#heartbeat.panes == 1 and heartbeat.panes[1].pane_id == 91)
local attached = heartbeat.domains['dmux-b-usb']
assert(attached.state == 'Attached' and attached.has_any_panes == true)
assert(attached.pane_count == 3)
assert(attached.valid_marker_pane_count == 1)
assert(attached.system_pane_count == 1)
assert(attached.pane_count ~= attached.valid_marker_pane_count + attached.system_pane_count)
local detached = heartbeat.domains['dmux-b-ts']
assert(detached.state == 'Detached' and detached.has_any_panes == false)
assert(detached.pane_count == 0 and detached.valid_marker_pane_count == 0 and detached.system_pane_count == 0)

domain_rows[2] = domain('dmux-b-ts', 'Detached', nil)
ok, err = instance.heartbeat {
  id = 'gui-42-cafe',
  pid = 42,
  process_start_token = 'start-token',
  paths = { heartbeat = '/runtime/heartbeat.json' },
}
assert(not ok and err:match 'domain state is unavailable')

io.stdout:write 'dmux heartbeat test: exact per-domain marker/system coverage counts passed\n'

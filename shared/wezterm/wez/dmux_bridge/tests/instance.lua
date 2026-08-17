package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local epoch = '55555555-5555-4555-8555-555555555555'
local usb_instance = '66666666-6666-4666-8666-666666666666'
local ts_instance = '77777777-7777-4777-8777-777777777777'
local captured
local opened_instance
local lease_identity

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

local secure_bridge = {
  identity = function()
    return {
      gui_instance = lease_identity.gui_instance,
      pid = lease_identity.pid,
      process_start_token = lease_identity.process_start_token,
    }
  end,
  key = function()
    return '0123456789abcdef0123456789abcdef'
  end,
  write_heartbeat_atomic = function(_, body)
    captured = body
  end,
}

local fake_wezterm = {
  GLOBAL = {
    dmux_managed_persistent_domains = { 'dmux-b-ts', 'dmux-b-usb' },
    dmux_managed_persistent_domain_instances = {
      ['dmux-b-ts'] = ts_instance,
      ['dmux-b-usb'] = usb_instance,
    },
  },
  gui = {
    dmux_bridge_capabilities = function()
      return {
        version = 1,
        descriptor_backed_spool = true,
        exclusive_instance_lease = true,
        launcher_witness = true,
        checked_preflight = true,
        capability_bound_lifecycle_completion = true,
        zero_window_lifecycle = true,
        verified_mux_descriptor = true,
      }
    end,
    dmux_bridge_preflight = function()
      return {
        version = 1,
        key_bytes = 32,
        runtime_verified = true,
        verified_mux_descriptor = true,
        launcher_witness_present = true,
      }
    end,
    dmux_bridge_open = function(instance)
      opened_instance = instance
      lease_identity = {
        gui_instance = instance,
        pid = 42,
        process_start_token = 'start-token',
      }
      return secure_bridge
    end,
  },
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
  procinfo = {
    pid = function()
      return 42
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

local instance = require 'wez.dmux_bridge.instance'
assert(
  instance.configure_persistent_domains(
    fake_wezterm.GLOBAL.dmux_managed_persistent_domains,
    fake_wezterm.GLOBAL.dmux_managed_persistent_domain_instances
  )
)
fake_wezterm.GLOBAL.dmux_managed_persistent_domains = nil
fake_wezterm.GLOBAL.dmux_managed_persistent_domain_instances = nil
local state = assert(instance.create())
assert(type(opened_instance) == 'string' and opened_instance:match '^gui%-42%-[0-9a-f]+$')
assert(state.id == opened_instance and state.bridge == secure_bridge)
assert(state.persistent_domain_instances['dmux-b-usb'] == usb_instance)

local identity = assert(instance.current_identity())
assert(identity.gui_instance == opened_instance and identity.pid == 42)
assert(identity.process_start_token == 'start-token')
identity.pid = 99
assert(assert(instance.current_identity()).pid == 42, 'lease identity must be returned as a defensive copy')
assert(instance.current_bridge(opened_instance) == secure_bridge)
assert(instance.current_bridge 'gui-other' == nil)

lease_identity.process_start_token = 'replaced-token'
local stale, stale_err = instance.current_identity()
assert(stale == nil and stale_err:match 'identity changed')
lease_identity.process_start_token = 'start-token'

local ok, err = instance.heartbeat(state)
assert(ok and not err)
local heartbeat = assert(require('wez.dmux_bridge.json').decode(captured))
assert(#heartbeat.panes == 1 and heartbeat.panes[1].pane_id == 91)
local attached = heartbeat.domains['dmux-b-usb']
assert(attached.state == 'Attached' and attached.has_any_panes == true)
assert(attached.backend_instance_uid == usb_instance)
assert(attached.pane_count == 3)
assert(attached.valid_marker_pane_count == 1)
assert(attached.system_pane_count == 1)
assert(attached.system_epoch == epoch)
assert(attached.system_workspace == 'dmux:system:' .. epoch)
assert(attached.pane_count ~= attached.valid_marker_pane_count + attached.system_pane_count)
local detached = heartbeat.domains['dmux-b-ts']
assert(detached.state == 'Detached' and detached.has_any_panes == false)
assert(detached.backend_instance_uid == ts_instance)
assert(detached.pane_count == 0 and detached.valid_marker_pane_count == 0 and detached.system_pane_count == 0)

domain_rows[2] = domain('dmux-b-ts', 'Detached', nil)
ok, err = instance.heartbeat(state)
assert(not ok and err:match 'domain state is unavailable')

io.stdout:write 'dmux heartbeat/lease test: secure identity and exact marker/system coverage passed\n'

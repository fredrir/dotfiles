package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

assert(os.getenv 'DMUX_WEZ_FIRST' == '1')

local events = {}
local rendered
local fake_wezterm = {
  format = function(elements)
    return elements
  end,
  on = function(name, callback)
    events[name] = callback
  end,
  plugin = {
    require = function()
      return {
        is_synced = function()
          return false
        end,
      }
    end,
  },
}
package.preload.wezterm = function()
  return fake_wezterm
end
package.preload['wez.design'] = function()
  return {
    accent = '#accent',
    host = 'gui-host-must-not-render',
    palette = {
      blue = '#blue',
      crust = '#crust',
      lavender = '#lavender',
      mantle = '#mantle',
      mauve = '#mauve',
      peach = '#peach',
      red = '#red',
      teal = '#teal',
      yellow = '#yellow',
    },
    tabs = { background = '#background' },
  }
end

local marker_ok = true
package.loaded['wez.dmux_bridge.context'] = {
  from_pane = function()
    if not marker_ok then
      return nil, { code = 'missing_marker' }
    end
    return { gui_pane_id = 91 }
  end,
  tab_summary = function()
    return { mixed = true, valid = 1, invalid = 1 }
  end,
}
package.loaded['wez.dmux_bridge.controller'] = {
  background_refresh = function()
    error 'fresh cache must not schedule a refresh'
  end,
  cached_context = function()
    return {
      display = {
        logical_ref = 'b2',
        space_name = 'project',
        owner_label = 'archie',
        owner_alias = 'b',
        backend = 'wez',
        route = 'archie-ts',
      },
    }
  end,
}

local window = {
  active_tab = function()
    return {}
  end,
  leader_is_active = function()
    return false
  end,
  set_right_status = function(_, value)
    rendered = value
  end,
}

local function text_content(elements)
  local parts = {}
  for _, element in ipairs(elements) do
    if element.Text then
      table.insert(parts, element.Text)
    end
  end
  return table.concat(parts, '|')
end

require('wez.appearance.status').setup()
assert(type(events['update-status']) == 'function')
events['update-status'](window, {})
local text = text_content(rendered)
for _, expected in ipairs { 'b2', 'project', 'archie (b)', 'wez', 'archie-ts', 'MIXED', 'UNSTAMPED' } do
  assert(text:find(expected, 1, true), 'missing managed status field: ' .. expected)
end
assert(not text:find('gui-host-must-not-render', 1, true))

marker_ok = false
events['update-status'](window, {})
text = text_content(rendered)
assert(text:find('DMUX INVALID', 1, true))
assert(not text:find('project', 1, true))

io.stdout:write 'dmux status test: active logical context, MIXED, and invalid-marker fail-closed passed\n'

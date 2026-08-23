local mode = assert(os.getenv 'HAMMER_APP_STATE')
assert(mode == 'absent' or mode == 'zero')
-- Mirrors macos/hammerspoon/init.lua: only the explicit opt-out 0 is legacy.
local managed = os.getenv 'DMUX_WEZ_FIRST' ~= '0'
local frontmost = os.getenv 'HAMMER_FRONTMOST' == '1'

local tasks = {}
local launches = {}
local hotkeys = {}
local app_activations = 0
local app_hides = 0

local app = {
  activate = function()
    app_activations = app_activations + 1
  end,
  allWindows = function()
    return {}
  end,
  bundleID = function()
    return 'com.github.wez.wezterm'
  end,
  focusedWindow = function()
    return nil
  end,
  hide = function()
    app_hides = app_hides + 1
  end,
  isFrontmost = function()
    return frontmost
  end,
  mainWindow = function()
    return nil
  end,
}

local function no_entries()
  return function()
    return nil
  end
end

hs = {
  alert = { show = function() end },
  application = {
    applicationsForBundleID = function()
      return mode == 'zero' and { app } or {}
    end,
    frontmostApplication = function()
      return {
        bundleID = function()
          return 'test.frontmost'
        end,
      }
    end,
    launchOrFocusByBundleID = function(bundle)
      table.insert(launches, bundle)
    end,
  },
  caffeinate = {
    get = function()
      return false
    end,
    set = function() end,
  },
  fs = {
    dir = no_entries,
    mkdir = function()
      return true
    end,
  },
  hotkey = {
    bind = function(mods, key, callback)
      hotkeys[tostring(key)] = callback
      return { delete = function() end }
    end,
  },
  json = {
    decode = function()
      return {}
    end,
  },
  notify = {
    defaultNotificationSound = 'default',
    new = function()
      return {
        send = function(self)
          return self
        end,
        withdraw = function() end,
      }
    end,
  },
  pathwatcher = {
    new = function()
      return { start = function() end }
    end,
  },
  reload = function() end,
  task = {
    new = function(bin, callback, args)
      local task = { bin = bin, args = args, callback = callback }
      table.insert(tasks, task)
      return {
        start = function()
          if callback then
            callback(0, '', '')
          end
        end,
      }
    end,
  },
  timer = {
    doEvery = function()
      return {}
    end,
    secondsSinceEpoch = function()
      return 1800000000
    end,
  },
  window = {
    animationDuration = 0,
    orderedWindows = function()
      return {}
    end,
  },
}

dofile 'macos/hammerspoon/init.lua'
assert(type(hotkeys['§']) == 'function', 'summon hotkey was not registered')
hotkeys['§']()

local wezterm_bin = '/Applications/WezTerm.app/Contents/MacOS/wezterm'
if managed then
  assert(#launches == 0, 'managed summon must not raw-launch the application')
  assert(#tasks == 1, 'managed summon must issue exactly one broker task')
  assert(tasks[1].bin:match '/%.local/bin/dmux$')
  assert(tasks[1].args[1] == '_gui' and tasks[1].args[2] == 'summon' and tasks[1].args[3] == nil)
else
  if mode == 'absent' then
    assert(#tasks == 0, 'legacy absent-app path must not require dmux')
    assert(#launches == 1 and launches[1] == 'com.github.wez.wezterm')
  elseif frontmost then
    assert(#launches == 0)
    assert(#tasks == 0, 'legacy frontmost toggle must hide without spawning')
  else
    assert(#launches == 0)
    assert(#tasks == 1 and tasks[1].bin == wezterm_bin)
    assert(tasks[1].args[1] == 'cli' and tasks[1].args[2] == 'spawn' and tasks[1].args[3] == '--new-window')
  end
end

if mode == 'zero' then
  if not managed and frontmost then
    -- Preserve the legacy frontmost toggle: flag-off behavior hides the app
    -- before considering whether it currently has a window.
    assert(app_activations == 0)
    assert(app_hides == 1)
  else
    assert(app_activations == 1)
  end
end
if managed and mode == 'zero' and frontmost then
  assert(app_hides == 0, 'managed zero-window summon must run before frontmost hide-toggle')
end

io.stdout:write(
  string.format(
    'hammerspoon summon test: managed=%s state=%s frontmost=%s\n',
    tostring(managed),
    mode,
    tostring(frontmost)
  )
)

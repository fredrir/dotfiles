-- Hammerspoon: macOS glue for the WezTerm-centric workflow.
-- Owns every global hotkey (skhd retired); WezTerm keeps everything
-- scoped to the terminal itself.

hs.window.animationDuration = 0 -- no easing lag on focus/hide

local WEZTERM_BUNDLE = 'com.github.wez.wezterm'
local WEZTERM_BIN = '/Applications/WezTerm.app/Contents/MacOS/wezterm'
local HOME = os.getenv 'HOME'

-- Run a program without a shell; callback gets (exitCode, stdout, stderr).
local function run(bin, args, callback)
  local task = hs.task.new(bin, callback, args)
  task:start()
  return task
end

---------------------------------------------------------------------------
-- Quake-style summon: CMD+§ toggles WezTerm
---------------------------------------------------------------------------
-- Focus WezTerm from anywhere; press again to drop back to whatever was
-- focused before. Handles: not running (launch), running with zero
-- windows (spawn via the CLI), hidden/minimized windows, other Spaces,
-- and fullscreen (macOS ignores hide() there, so focus the previous app).

local previousApp = nil

local function focusPreviousApp()
  if previousApp and previousApp:isRunning() and previousApp:bundleID() ~= WEZTERM_BUNDLE then
    previousApp:activate()
    return
  end
  for _, win in ipairs(hs.window.orderedWindows()) do
    local app = win:application()
    if app and app:bundleID() ~= WEZTERM_BUNDLE then
      win:focus()
      return
    end
  end
end

local function summonWezTerm()
  local app = hs.application.applicationsForBundleID(WEZTERM_BUNDLE)[1]

  if not app then
    hs.application.launchOrFocusByBundleID(WEZTERM_BUNDLE)
    return
  end

  if app:isFrontmost() then
    local win = app:focusedWindow()
    if win and win:isFullScreen() then
      focusPreviousApp()
    else
      app:hide()
    end
    return
  end

  previousApp = hs.application.frontmostApplication()

  local windows = app:allWindows()
  if #windows == 0 then
    -- quit_when_all_windows_are_closed = false keeps the app resident;
    -- give it a window again through the warm GUI process.
    run(WEZTERM_BIN, { 'cli', 'spawn', '--new-window' })
    app:activate()
    return
  end

  app:activate() -- unhides; switches Space if the window lives elsewhere
  local win = app:focusedWindow() or app:mainWindow()
  if not win then
    for _, w in ipairs(windows) do
      if w:isMinimized() then
        w:unminimize()
        win = w
        break
      end
    end
  end
  if win then
    win:focus()
  end
end

-- '§' sits left of 1 on the Norwegian Apple layout: unshifted, no AltGr,
-- unclaimed by macOS. Fall back to raw keycode 10 (kVK_ISO_Section, the
-- same physical key on any ISO board) if the layout map has no '§'.
QuakeHotkey = nil
local okBind, hotkey = pcall(hs.hotkey.bind, { 'cmd' }, '§', summonWezTerm)
if okBind then
  QuakeHotkey = hotkey
else
  QuakeHotkey = hs.hotkey.bind({ 'cmd' }, 10, summonWezTerm)
end

---------------------------------------------------------------------------
-- clean-copy: CMD+ALT+C from any app (migrated from skhd)
---------------------------------------------------------------------------
-- Cleans whatever is on the clipboard, then captures it to the transcript.
-- The && chain needs a shell; both binaries are addressed absolutely.

CleanCopyHotkey = hs.hotkey.bind({ 'cmd', 'alt' }, 'c', function()
  run('/bin/sh', {
    '-c',
    HOME .. '/.local/bin/clean-copy && ' .. HOME .. '/.local/bin/transcript capture --quiet',
  }, function(exitCode, _, stderr)
    if exitCode ~= 0 then
      hs.alert.show 'clean-copy failed'
      print('clean-copy: ' .. tostring(stderr))
    end
  end)
end)

---------------------------------------------------------------------------
-- Attention bridge: marker files -> Notification Center (+ caffeinate)
---------------------------------------------------------------------------
-- The wezterm-attention plugin (and the bell handler in
-- wez/appearance/tabs.lua) drops one JSON file per pane in ATTENTION_DIR.
-- The tab bar renders glyphs; here the same markers become native
-- notifications when WezTerm is in the background, with click-to-jump to
-- the exact pane. While any pane is 'thinking', idle sleep is held off
-- (AC power only).

local ATTENTION_DIR = HOME .. '/.local/state/wezterm-attention'
local NOTIFY_TYPES = { stop = 'Done', notify = 'Needs attention' }
local THINKING_STALE_S = 30 * 60 -- match the plugin's stale_after_ms

hs.fs.mkdir(ATTENTION_DIR)

local lastSeen = {} -- pane_id -> { type = ..., at = ... } for dedup
SentNotifications = {} -- pane_id -> hs.notify, retained so clicks work

local function readMarker(path)
  local file = io.open(path, 'r')
  if not file then
    return nil
  end
  local raw = file:read '*a'
  file:close()
  local ok, data = pcall(hs.json.decode, raw)
  if ok and type(data) == 'table' and data.type then
    return data.type, data
  end
  local text = raw:gsub('%s+', '')
  if #text > 0 then
    return text, {}
  end
  return nil
end

local function anyPaneThinking()
  for name in hs.fs.dir(ATTENTION_DIR) do
    if name ~= '.' and name ~= '..' then
      local kind, data = readMarker(ATTENTION_DIR .. '/' .. name)
      if kind == 'thinking' then
        -- updated_at may be seconds or milliseconds; normalize to seconds.
        local at = tonumber(data.updated_at)
        if at and at > 1e11 then
          at = at / 1000
        end
        local age = at and (hs.timer.secondsSinceEpoch() - at) or 0
        if age < THINKING_STALE_S then
          return true
        end
      end
    end
  end
  return false
end

local function updateCaffeinate()
  local wantAwake = anyPaneThinking()
  if hs.caffeinate.get 'systemIdle' ~= wantAwake then
    hs.caffeinate.set('systemIdle', wantAwake, false) -- false: AC only
  end
end

local function notifyForPane(paneId, kind)
  -- One 'wezterm cli list' to label the notification with the pane title;
  -- degrade to a plain notification if the CLI is unavailable.
  run(WEZTERM_BIN, { 'cli', 'list', '--format', 'json' }, function(exitCode, stdout)
    local title = 'WezTerm'
    local body = NOTIFY_TYPES[kind] .. ' in pane ' .. paneId
    if exitCode == 0 then
      local ok, panes = pcall(hs.json.decode, stdout)
      if ok and type(panes) == 'table' then
        for _, pane in ipairs(panes) do
          if tostring(pane.pane_id) == paneId then
            if pane.title and #pane.title > 0 then
              title = pane.title
            end
            local tab = pane.tab_title
            body = NOTIFY_TYPES[kind] .. (tab and #tab > 0 and (' - ' .. tab) or '')
            break
          end
        end
      end
    end

    if SentNotifications[paneId] then
      SentNotifications[paneId]:withdraw()
    end
    SentNotifications[paneId] = hs.notify
      .new(function()
        -- Click: jump straight to the pane that asked for attention.
        run(WEZTERM_BIN, { 'cli', 'activate-pane', '--pane-id', paneId }, function()
          hs.application.launchOrFocusByBundleID(WEZTERM_BUNDLE)
        end)
      end, {
        title = title,
        informativeText = body,
        soundName = kind == 'notify' and hs.notify.defaultNotificationSound or nil,
        withdrawAfter = 0,
      })
      :send()
  end)
end

AttentionWatcher = hs.pathwatcher.new(ATTENTION_DIR, function(paths)
  for _, path in ipairs(paths) do
    local paneId = path:match '([^/]+)$'
    local kind = readMarker(path) -- nil when the marker was just removed
    if paneId and kind and NOTIFY_TYPES[kind] then
      local frontmost = hs.application.frontmostApplication()
      local wezFocused = frontmost and frontmost:bundleID() == WEZTERM_BUNDLE
      local seen = lastSeen[paneId]
      local dup = seen and seen.type == kind and (hs.timer.secondsSinceEpoch() - seen.at) < 5
      if not wezFocused and not dup then
        lastSeen[paneId] = { type = kind, at = hs.timer.secondsSinceEpoch() }
        notifyForPane(paneId, kind)
      end
    end
  end
  updateCaffeinate()
end)
AttentionWatcher:start()

-- Safety net: re-evaluate every 5 minutes so a crashed agent cannot pin
-- the machine awake through a stale thinking marker.
CaffeinateTimer = hs.timer.doEvery(300, updateCaffeinate)
updateCaffeinate()

---------------------------------------------------------------------------
-- Auto-reload on config change (watch the repo, not the symlink)
---------------------------------------------------------------------------

ConfigWatcher = hs.pathwatcher.new(HOME .. '/dotfiles/macos/hammerspoon/', function(paths)
  for _, path in ipairs(paths) do
    if path:sub(-4) == '.lua' then
      hs.reload()
      return
    end
  end
end)
ConfigWatcher:start()

hs.alert.show 'Hammerspoon ready'

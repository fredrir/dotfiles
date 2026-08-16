package.path = table.concat({
  'shared/wezterm/?.lua',
  'shared/wezterm/?/init.lua',
  package.path,
}, ';')

local host = '22222222-2222-4222-8222-222222222222'
local space = '33333333-3333-4333-8333-333333333333'
local backend_instance = '44444444-4444-4444-8444-444444444444'
local epoch = '55555555-5555-4555-8555-555555555555'
local group = 'g' .. epoch .. '.wz-7'
local split = 'p' .. epoch .. '.wz-9'
local workspace = 'dmux:' .. host .. ':' .. space

local vars = {
  dmux_context_version = '1',
  dmux_host_uid = host,
  dmux_space_uid = space,
  dmux_space_no = '2',
  dmux_backend = 'wez',
  dmux_domain = '',
  dmux_server_epoch = epoch,
  dmux_group_ref = group,
  dmux_split_ref = split,
}

local pane_activated = false
local pane = {
  activate = function()
    pane_activated = true
  end,
  get_domain_name = function()
    return 'dmux-b-usb'
  end,
  get_user_vars = function()
    return vars
  end,
  pane_id = function()
    return 91
  end,
}
local tab_activated = false
local tab = {
  activate = function()
    tab_activated = true
  end,
  panes = function()
    return { pane }
  end,
  panes_with_info = function()
    return { { pane = pane, is_active = true } }
  end,
  tab_id = function()
    return 7
  end,
}
local function mux_window(id, name, tabs)
  return {
    get_workspace = function()
      return name
    end,
    tabs = function()
      return tabs
    end,
    window_id = function()
      return id
    end,
  }
end
local target_window = mux_window(5, workspace, { tab })
local function sentinel_window_for(domain_name)
  local sentinel_pane = {
    get_domain_name = function()
      return domain_name
    end,
  }
  local sentinel_tab = {
    panes = function()
      return { sentinel_pane }
    end,
  }
  return mux_window(6, 'dmux:system:' .. epoch, { sentinel_tab })
end
local sentinel_window = sentinel_window_for 'dmux-b-usb'

local domains = {}
local function domain(name, state)
  local value = { current = state, panes = state == 'Attached', attaches = 0, detaches = 0 }
  function value:attach()
    if self.fail_attach then
      error 'route unavailable'
    end
    self.attaches = self.attaches + 1
    self.current = 'Attached'
    self.panes = true
  end
  function value:detach()
    self.detaches = self.detaches + 1
    self.current = 'Detached'
    self.panes = false
  end
  function value:state()
    return self.current
  end
  function value:has_any_panes()
    return self.panes
  end
  function value:name()
    return name
  end
  domains[name] = value
  return value
end
local selected = domain('dmux-b-usb', 'Detached')
local alternate = domain('dmux-b-ts', 'Attached')
local windows = {}
local active_workspace
local mux = {
  all_domains = function()
    local out = {}
    for _, value in pairs(domains) do
      table.insert(out, value)
    end
    return out
  end,
  all_windows = function()
    return windows
  end,
  get_domain = function(name)
    return domains[name]
  end,
  set_active_workspace = function(name)
    active_workspace = name
  end,
}

local scheduled = {}
local performed = {}
local managed_hides = 0
local gui_pane = {}
local gui_window = {
  active_pane = function()
    return gui_pane
  end,
  effective_config = function()
    return { dmux_managed_gui = true }
  end,
  perform_action = function(_, action, action_pane)
    table.insert(performed, { action = action, pane = action_pane })
  end,
  toast_notification = function() end,
}
local act = {
  HideApplication = { name = 'HideApplication' },
  QuitApplication = { name = 'QuitApplication' },
}
local visible_gui_windows = { gui_window }
local fake_wezterm = {
  action = act,
  gui = {
    gui_windows = function()
      return visible_gui_windows
    end,
  },
  log_error = function(message)
    error(message)
  end,
  mux = mux,
  target_triple = 'aarch64-apple-darwin',
  time = {
    call_after = function(_, callback)
      table.insert(scheduled, callback)
    end,
  },
}
package.preload.wezterm = function()
  return fake_wezterm
end

local presentation = require 'wez.dmux_bridge.presentation'
local target = {
  domain = 'dmux-b-usb',
  workspace = workspace,
  host_uid = host,
  space_uid = space,
  backend_instance_uid = backend_instance,
  server_epoch = epoch,
  group_ref = group,
  split_ref = split,
}
local function dispatch(action, action_target, state)
  local result, failure
  presentation.dispatch(
    { action = action, target = action_target, expiry = os.time() + 10 },
    state or { safe_quit = {}, persistent_domains = { 'dmux-b-ts', 'dmux-b-usb' } },
    function(ok, err)
      result, failure = ok, err
    end
  )
  return result, failure
end

windows = { target_window }
local result, failure = dispatch('activate', target)
assert(result and not failure and result.pane_id == 91 and result.split_ref == split)
assert(active_workspace == workspace and tab_activated and pane_activated)

local group_target = {}
for key, value in pairs(target) do
  if key ~= 'split_ref' then
    group_target[key] = value
  end
end
result, failure = dispatch('activate', group_target)
assert(result and not failure and result.group_ref == group and result.split_ref == split and result.pane_id == 91)

windows = {}
active_workspace = nil
result, failure = dispatch('activate', target)
assert(not result and failure.code == 'not_found' and active_workspace == nil)

-- A stale selector cannot attach USB while the same backend instance is
-- already visible through Tailscale. It fails without detaching either.
windows = { sentinel_window }
local attach_target = {
  domain = 'dmux-b-usb',
  backend_instance_uid = backend_instance,
  server_epoch = epoch,
  alternate_domains = { 'dmux-b-ts' },
}
result, failure = dispatch('attach_domain', attach_target)
assert(not result and failure.code == 'alternate_route_attached')
assert(alternate.detaches == 0 and alternate.current == 'Attached')
assert(selected.attaches == 0 and selected.current == 'Detached')

-- Correct route selection reuses the already-attached compatible domain.
local ts_target = {
  domain = 'dmux-b-ts',
  backend_instance_uid = backend_instance,
  server_epoch = epoch,
  alternate_domains = { 'dmux-b-usb' },
}
windows = { sentinel_window_for 'dmux-b-ts' }
result, failure = dispatch('attach_domain', ts_target)
assert(result and not failure and result.domain == 'dmux-b-ts')
assert(alternate.attaches == 0 and selected.attaches == 0)

-- With no attached route, the verified selected route attaches normally.
alternate.current = 'Detached'
alternate.panes = false
windows = { sentinel_window }
result, failure = dispatch('attach_domain', attach_target)
assert(result and not failure and result.domain_state == 'Attached')
assert(alternate.detaches == 0 and alternate.current == 'Detached')
assert(selected.attaches == 1 and selected.current == 'Attached')

-- A selected route in a transient state is never driven or reinterpreted
-- as detached merely because attach() happens to be callable.
selected.current = 'Attaching'
selected.panes = false
result, failure = dispatch('attach_domain', attach_target)
assert(not result and failure.code == 'domain_inventory_unstable')
assert(selected.attaches == 1 and alternate.detaches == 0)

-- A failed selected route never mutates an alternate route.
selected.current = 'Detached'
selected.panes = false
selected.fail_attach = true
alternate.current = 'Detached'
alternate.panes = false
local failed_route_result, failed_route_error = dispatch('attach_domain', attach_target)
assert(not failed_route_result and failed_route_error.code == 'attach_failed')
assert(alternate.current == 'Detached' and alternate.detaches == 0)
selected.fail_attach = false

-- A not-yet-visible sentinel keeps the action pending rather than creating
-- anything. Once it appears, the same bounded waiter completes.
selected.current = 'Detached'
windows = {}
scheduled = {}
local pending_result, pending_error
presentation.dispatch(
  { action = 'attach_domain', target = attach_target, expiry = os.time() + 10 },
  { safe_quit = {} },
  function(ok, err)
    pending_result, pending_error = ok, err
  end
)
assert(not pending_result and not pending_error and #scheduled == 1)
windows = { sentinel_window }
table.remove(scheduled, 1)()
assert(pending_result and pending_result.domain_state == 'Attached')

-- A detached state is not sufficient for safe lifecycle completion: the GUI
-- must also report that no imported panes remain for the domain.
selected.current = 'Detached'
selected.panes = true
scheduled = {}
pending_result, pending_error = nil, nil
presentation.dispatch({
  action = 'detach_domain',
  target = { domain = 'dmux-b-usb' },
  expiry = os.time() + 10,
}, { safe_quit = {} }, function(ok, err)
  pending_result, pending_error = ok, err
end)
assert(not pending_result and not pending_error and #scheduled == 1)
selected.panes = false
table.remove(scheduled, 1)()
assert(pending_result and pending_result.detached_domains[1] == 'dmux-b-usb')

local real_time = os.time
local fake_now = 100
os.time = function()
  return fake_now
end
selected.current = 'Detached'
windows = {}
scheduled = {}
pending_result, pending_error = nil, nil
presentation.dispatch(
  { action = 'attach_domain', target = attach_target, expiry = 110 },
  { safe_quit = {} },
  function(ok, err)
    pending_result, pending_error = ok, err
  end
)
assert(#scheduled == 1 and not pending_result and not pending_error)
fake_now = 105
table.remove(scheduled, 1)()
os.time = real_time
assert(not pending_result and pending_error.code == 'attach_timeout')

windows = { sentinel_window, target_window }
result, failure = dispatch('present', target)
assert(result and not failure and result.workspace == workspace and result.pane_id == 91)

local quit_state = { safe_quit = {}, persistent_domains = { 'dmux-b-ts', 'dmux-b-usb' } }
selected.current = 'Attached'
selected.panes = true
alternate.current = 'Detached'
alternate.panes = false

-- The detach set is an exact serialization-point snapshot, not a stale
-- controller hint. Empty is authorized only when nothing persistent is live.
local stale_result, stale_error = dispatch('safe_quit', { phase = 'detach', domains = {} }, quit_state)
assert(not stale_result and stale_error.code == 'quit_domain_snapshot_mismatch')
local rogue = domain('rogue-domain', 'Attached')
local unknown_result, unknown_error =
  dispatch('safe_quit', { phase = 'detach', domains = { 'dmux-b-usb' } }, quit_state)
assert(not unknown_result and unknown_error.code == 'unknown_persistent_domain')
domains['rogue-domain'] = nil

local detach_result
presentation.dispatch(
  {
    uid = '11111111-1111-4111-8111-111111111111',
    action = 'safe_quit',
    target = { phase = 'detach', domains = { 'dmux-b-usb' } },
    expiry = os.time() + 10,
  },
  quit_state,
  function(ok, err)
    assert(not err)
    detach_result = ok
  end
)
assert(detach_result and quit_state.safe_quit['11111111-1111-4111-8111-111111111111'])

-- A domain appearing between detach and finish invalidates the proof.
rogue = domain('rogue-domain', 'Attached')
local raced_result, raced_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = '11111111-1111-4111-8111-111111111111',
  platform_action = 'hide',
}, quit_state)
assert(not raced_result and raced_error.code == 'unknown_persistent_domain')
domains['rogue-domain'] = nil
local mismatch_result, mismatch_error
presentation.dispatch(
  {
    uid = '88888888-8888-4888-8888-888888888888',
    action = 'safe_quit',
    target = {
      phase = 'finish',
      proof_uid = '11111111-1111-4111-8111-111111111111',
      platform_action = 'quit',
    },
    expiry = os.time() + 10,
  },
  quit_state,
  function(ok, err)
    mismatch_result, mismatch_error = ok, err
  end
)
assert(not mismatch_result and mismatch_error.code == 'quit_platform_mismatch')
local finish_result, finish_error
presentation.dispatch(
  {
    uid = '99999999-9999-4999-8999-999999999999',
    action = 'safe_quit',
    target = {
      phase = 'finish',
      proof_uid = '11111111-1111-4111-8111-111111111111',
      platform_action = 'hide',
    },
    expiry = os.time() + 10,
  },
  quit_state,
  function(ok, err)
    finish_result, finish_error = ok, err
  end
)
assert(not finish_result and finish_error.code == 'managed_quit_unavailable')
gui_window.dmux_safe_hide_application = function()
  managed_hides = managed_hides + 1
end
gui_window.effective_config = function()
  return { dmux_managed_gui = false }
end
finish_result, finish_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = '11111111-1111-4111-8111-111111111111',
  platform_action = 'hide',
}, quit_state)
assert(not finish_result and finish_error.code == 'managed_quit_unavailable')
gui_window.effective_config = function()
  return { dmux_managed_gui = true }
end
finish_result, finish_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = '11111111-1111-4111-8111-111111111111',
  platform_action = 'hide',
}, quit_state)
assert(finish_result and finish_result.platform_action == 'hide' and type(finish_result.after_ack) == 'function')
assert(#performed == 0, 'platform lifecycle action must wait until ack publication')
finish_result.after_ack()
assert(managed_hides == 1 and #performed == 0, 'macOS safe hide must not perform native HideApplication')
assert(quit_state.safe_quit['11111111-1111-4111-8111-111111111111'] == nil)

-- Tmux-only/local GUI state may produce the one explicitly authorized empty
-- no-op detach proof; all configured persistent domains are already absent.
local empty_state = { safe_quit = {}, persistent_domains = { 'dmux-b-ts', 'dmux-b-usb' } }
local empty_result
presentation.dispatch(
  {
    uid = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
    action = 'safe_quit',
    target = { phase = 'detach', domains = {} },
    expiry = os.time() + 10,
  },
  empty_state,
  function(ok, err)
    assert(not err)
    empty_result = ok
  end
)
assert(empty_result and #empty_result.detached_domains == 0)
visible_gui_windows = {}
local hidden_result, hidden_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  platform_action = 'hide',
}, empty_state)
assert(hidden_result and not hidden_error and hidden_result.already_hidden == true)
hidden_result.after_ack()
assert(managed_hides == 1, 'zero-window macOS finish is already hidden and needs no stale window action')
visible_gui_windows = { gui_window }

-- Linux termination uses only the maintained fork's explicit post-proof
-- primitive. Native QuitApplication is not a managed lifecycle escape hatch.
fake_wezterm.target_triple = 'x86_64-unknown-linux-gnu'
local linux_state = { safe_quit = {}, persistent_domains = { 'dmux-b-ts', 'dmux-b-usb' } }
presentation.dispatch(
  {
    uid = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
    action = 'safe_quit',
    target = { phase = 'detach', domains = {} },
    expiry = os.time() + 10,
  },
  linux_state,
  function(_, err)
    assert(not err)
  end
)
local linux_result, linux_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  platform_action = 'quit',
}, linux_state)
assert(not linux_result and linux_error.code == 'managed_quit_unavailable')
local managed_quits = 0
gui_window.dmux_safe_quit_application = function()
  managed_quits = managed_quits + 1
end
gui_window.effective_config = function()
  return { dmux_managed_gui = false }
end
linux_result, linux_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  platform_action = 'quit',
}, linux_state)
assert(not linux_result and linux_error.code == 'managed_quit_unavailable')
gui_window.effective_config = function()
  return { dmux_managed_gui = true }
end
linux_result, linux_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  platform_action = 'quit',
}, linux_state)
assert(linux_result and not linux_error and type(linux_result.after_ack) == 'function')
linux_result.after_ack()
assert(managed_quits == 1 and #performed == 0, 'Linux safe quit must not perform native QuitApplication')
fake_wezterm.target_triple = 'aarch64-apple-darwin'

io.stdout:write 'dmux presentation test: no-create attach/focus/safe-quit passed\n'

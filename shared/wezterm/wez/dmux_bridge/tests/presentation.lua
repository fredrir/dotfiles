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
local pane_vars = vars

local pane_activated = false
local pane = {
  activate = function()
    pane_activated = true
  end,
  get_domain_name = function()
    return 'dmux-b-usb'
  end,
  get_user_vars = function()
    return pane_vars
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
local function sentinel_window_for(domain_name, sentinel_epoch)
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
  return mux_window(6, 'dmux:system:' .. (sentinel_epoch or epoch), { sentinel_tab })
end
local sentinel_window = sentinel_window_for 'dmux-b-usb'

-- The real mux keys domains by domain_id and hands Lua an ordered array, so
-- two rows may legitimately share one name. A name-keyed table cannot express
-- that, which is why a suite that already covered rogue domains never caught
-- the leaked connection-UI placeholder. pairs() also iterates in an
-- unspecified order, so this array pins the sequence the guards actually see.
local rows = {}
local by_name = {}
local function domain(name, state, spawnable)
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
    if self.refuse_detach then
      error 'cannot detach a TermWizTerminalDomain'
    end
    self.detaches = self.detaches + 1
    self.current = 'Detached'
    if not self.delay_detach then
      self.panes = false
    end
    -- Mux::domain_was_detached kills every pane in the domain and prunes the
    -- windows that leaves empty, so a detached route stops contributing an
    -- imported workspace. A fake that kept the window would hide exactly the
    -- sentinel ambiguity two attached routes to one server produce.
    if self.on_detach then
      self.on_detach(self)
    end
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
  -- A domain created without a capability has no is_spawnable method at all,
  -- which is the shape of any domain object that predates or omits it.
  if spawnable ~= nil then
    value.spawnable = spawnable
    function value:is_spawnable()
      if self.spawnable == 'error' then
        error 'capability unavailable'
      end
      return self.spawnable
    end
  end
  table.insert(rows, value)
  by_name[name] = value
  return value
end

local function remove_domain(value)
  for index, row in ipairs(rows) do
    if row == value then
      table.remove(rows, index)
      break
    end
  end
  for name, row in pairs(by_name) do
    if row == value then
      by_name[name] = nil
    end
  end
end

-- WezTerm registers one of these per ClientDomain::attach() and never frees it.
local function placeholder(has_panes)
  local value = domain('TermWizTerminalDomain', 'Attached', false)
  value.panes = has_panes or false
  value.refuse_detach = true
  return value
end

local selected = domain('dmux-b-usb', 'Detached', true)
local alternate = domain('dmux-b-ts', 'Attached', true)
local windows = {}
local active_workspace
local mux = {
  all_domains = function()
    return rows
  end,
  all_windows = function()
    return windows
  end,
  get_domain = function(name)
    return by_name[name]
  end,
  set_active_workspace = function(name)
    active_workspace = name
  end,
}

local scheduled = {}
local performed = {}
local managed_hides = 0
local managed_quits = 0
local completion_error
local completion_calls = {}
local logged_errors = {}
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
    table.insert(logged_errors, message)
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
local secure_bridge = {
  identity = function()
    return {
      gui_instance = 'gui-42-cafe',
      pid = 42,
      process_start_token = 'start-token',
    }
  end,
  resident_brokered = function()
    return true
  end,
  complete_safe_lifecycle = function(_, uid, action)
    if completion_error then
      error(completion_error)
    end
    table.insert(completion_calls, { uid = uid, action = action })
    if action == 'hide' then
      managed_hides = managed_hides + 1
    else
      managed_quits = managed_quits + 1
    end
    return true
  end,
}
local raw_dispatch = presentation.dispatch
local generated_uid = 0
presentation.dispatch = function(request, state, done)
  generated_uid = generated_uid + 1
  request.uid = request.uid or string.format('00000000-0000-4000-8000-%012d', generated_uid)
  request.origin = request.origin
    or {
      kind = 'resident_gui',
      gui_instance = 'gui-42-cafe',
      pid = 42,
      process_start_token = 'start-token',
    }
  state = state or {}
  state.id = state.id or 'gui-42-cafe'
  state.pid = state.pid or 42
  state.process_start_token = state.process_start_token or 'start-token'
  state.bridge = state.bridge or secure_bridge
  state.safe_quit = state.safe_quit or {}
  state.persistent_domains = state.persistent_domains or { 'dmux-b-ts', 'dmux-b-usb' }
  state.persistent_domain_instances = state.persistent_domain_instances
    or {
      ['dmux-b-ts'] = backend_instance,
      ['dmux-b-usb'] = backend_instance,
    }
  return raw_dispatch(request, state, done)
end
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

-- §12.3 keeps at most one route to a backend instance attached, so selecting
-- USB while the same backend instance is still imported through Tailscale
-- detaches the stale alternate first. This is acceptance case 20's path: the
-- host, backend instance, epoch and Space are unchanged and only the transport
-- differs, so refusing here would strand the Space on a dead route. Both routes
-- import the one server's sentinel workspace, so the alternate's window must be
-- gone before the selected route's sentinel can be unambiguous.
windows = { sentinel_window, sentinel_window_for 'dmux-b-ts' }
alternate.on_detach = function()
  windows = { sentinel_window }
end
local attach_target = {
  domain = 'dmux-b-usb',
  backend_instance_uid = backend_instance,
  server_epoch = epoch,
  alternate_domains = { 'dmux-b-ts' },
}
result, failure = dispatch('attach_domain', attach_target)
assert(result and not failure and result.domain == 'dmux-b-usb' and result.domain_state == 'Attached')
assert(alternate.detaches == 1 and alternate.current == 'Detached' and alternate.panes == false)
assert(selected.attaches == 1 and selected.current == 'Attached')

-- §8.4: a route change never moves the server epoch. An alternate whose system
-- sentinel reports a new epoch invalidates the whole plan instead of being
-- detached as though it were still the signed backend.
selected.current = 'Detached'
selected.panes = false
selected.attaches = 0
alternate.current = 'Attached'
alternate.panes = true
alternate.detaches = 0
windows = { sentinel_window, sentinel_window_for('dmux-b-ts', '66666666-6666-4666-8666-666666666666') }
result, failure = dispatch('attach_domain', attach_target)
assert(not result and failure.code == 'backend_epoch_changed')
assert(alternate.detaches == 0 and alternate.current == 'Attached')
assert(selected.attaches == 0 and selected.current == 'Detached')
alternate.on_detach = nil

-- Correct route selection reuses the already-attached compatible domain and
-- never touches an alternate that is already detached.
local ts_target = {
  domain = 'dmux-b-ts',
  backend_instance_uid = backend_instance,
  server_epoch = epoch,
  alternate_domains = { 'dmux-b-usb' },
}
selected.current = 'Detached'
selected.panes = false
windows = { sentinel_window_for 'dmux-b-ts' }
result, failure = dispatch('attach_domain', ts_target)
assert(result and not failure and result.domain == 'dmux-b-ts')
assert(alternate.attaches == 0 and selected.attaches == 0 and selected.detaches == 0)

-- With no attached route, the verified selected route attaches normally and
-- the already-detached alternate is left alone.
alternate.current = 'Detached'
alternate.panes = false
alternate.detaches = 0
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

-- Detaching the alternate again after the selected route is up could race an
-- external re-attach without a bound, so an alternate that came back during the
-- attach fails closed instead of being detached a second time.
selected.current = 'Detached'
selected.panes = false
alternate.current = 'Detached'
alternate.panes = false
alternate.detaches = 0
windows = {}
scheduled = {}
pending_result, pending_error = nil, nil
presentation.dispatch(
  { action = 'attach_domain', target = attach_target, expiry = os.time() + 10 },
  { safe_quit = {} },
  function(ok, err)
    pending_result, pending_error = ok, err
  end
)
assert(not pending_result and not pending_error and #scheduled == 1)
alternate.current = 'Attached'
alternate.panes = true
windows = { sentinel_window }
table.remove(scheduled, 1)()
assert(not pending_result and pending_error.code == 'alternate_route_attached')
assert(alternate.detaches == 0 and alternate.current == 'Attached')
alternate.current = 'Detached'
alternate.panes = false

-- A detached state is not sufficient for safe lifecycle completion: the GUI
-- must also report that no imported panes remain for the domain.
selected.current = 'Attached'
selected.panes = true
selected.delay_detach = true
scheduled = {}
pending_result, pending_error = nil, nil
presentation.dispatch({
  action = 'detach_domain',
  target = { domain = 'dmux-b-usb', backend_instance_uid = backend_instance, server_epoch = epoch },
  expiry = os.time() + 10,
}, { safe_quit = {} }, function(ok, err)
  pending_result, pending_error = ok, err
end)
assert(not pending_result and not pending_error and #scheduled == 1)
selected.panes = false
selected.delay_detach = false
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

-- Import can expose the pane before its SetUserVar snapshot is complete.
-- The bridge waits for that exact transient shape, revalidates, then focuses;
-- it never acknowledges the incomplete marker itself.
pane_vars = {}
selected.current = 'Attached'
selected.panes = true
scheduled = {}
pending_result, pending_error = nil, nil
presentation.dispatch(
  { action = 'present', target = target, expiry = os.time() + 10 },
  { safe_quit = {} },
  function(ok, err)
    pending_result, pending_error = ok, err
  end
)
assert(not pending_result and not pending_error and #scheduled == 1)
pane_vars = vars
table.remove(scheduled, 1)()
assert(pending_result and not pending_error and pending_result.pane_id == 91)

local quit_state = { safe_quit = {}, persistent_domains = { 'dmux-b-ts', 'dmux-b-usb' } }
local usb_incarnation = {
  name = 'dmux-b-usb',
  backend_instance_uid = backend_instance,
  server_epoch = epoch,
}
selected.current = 'Attached'
selected.panes = true
alternate.current = 'Detached'
alternate.panes = false

-- The detach set is an exact serialization-point snapshot, not a stale
-- controller hint. Empty is authorized only when nothing persistent is live.
local stale_result, stale_error = dispatch('safe_quit', { phase = 'detach', domains = {} }, quit_state)
assert(not stale_result and stale_error.code == 'quit_domain_snapshot_mismatch')
local rogue = domain('rogue-domain', 'Attached', true)
local unknown_result, unknown_error =
  dispatch('safe_quit', { phase = 'detach', domains = { usb_incarnation } }, quit_state)
assert(not unknown_result and unknown_error.code == 'unknown_persistent_domain')
remove_domain(rogue)

local detach_result
presentation.dispatch(
  {
    uid = '11111111-1111-4111-8111-111111111111',
    action = 'safe_quit',
    target = { phase = 'detach', domains = { usb_incarnation } },
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
rogue = domain('rogue-domain', 'Attached', true)
local raced_result, raced_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = '11111111-1111-4111-8111-111111111111',
  platform_action = 'hide',
}, quit_state)
assert(not raced_result and raced_error.code == 'unknown_persistent_domain')
remove_domain(rogue)
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
local completion_method = secure_bridge.complete_safe_lifecycle
secure_bridge.complete_safe_lifecycle = nil
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
secure_bridge.complete_safe_lifecycle = completion_method
finish_result, finish_error = dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = '11111111-1111-4111-8111-111111111111',
  platform_action = 'hide',
}, quit_state)
assert(finish_result and finish_result.platform_action == 'hide' and type(finish_result.after_ack) == 'function')
assert(#performed == 0, 'platform lifecycle action must wait until ack publication')
finish_result.after_ack()
assert(managed_hides == 1 and #performed == 0, 'macOS safe hide must not perform native HideApplication')
assert(completion_calls[#completion_calls].action == 'hide')
assert(fake_wezterm.gui.dmux_safe_hide_application == nil, 'global lifecycle completion must be absent')
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
assert(linux_result and not linux_error and type(linux_result.after_ack) == 'function')
linux_result.after_ack()
assert(managed_quits == 1 and #performed == 0, 'Linux safe quit must not perform native QuitApplication')
assert(completion_calls[#completion_calls].action == 'quit')
assert(fake_wezterm.gui.dmux_safe_quit_application == nil, 'global lifecycle completion must be absent')
fake_wezterm.target_triple = 'aarch64-apple-darwin'

local function create_attached_quit_proof(uid, state)
  selected.current = 'Attached'
  selected.panes = true
  selected.fail_attach = false
  alternate.current = 'Detached'
  alternate.panes = false
  windows = { sentinel_window, target_window }
  local detached, detach_error
  presentation.dispatch(
    {
      uid = uid,
      action = 'safe_quit',
      target = { phase = 'detach', domains = { usb_incarnation } },
      expiry = os.time() + 10,
    },
    state,
    function(ok, err)
      detached, detach_error = ok, err
    end
  )
  assert(detached and not detach_error and state.safe_quit[uid])
end

-- A synchronous held-capability completion failure keeps the proof and
-- retries exact rollback. A transient attach failure is bounded and backed
-- off; the next attempt restores the same incarnation.
local completion_failure_state = {}
local completion_proof_uid = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc'
create_attached_quit_proof(completion_proof_uid, completion_failure_state)
local failed_finish = assert(dispatch('safe_quit', {
  phase = 'finish',
  proof_uid = completion_proof_uid,
  platform_action = 'hide',
}, completion_failure_state))
completion_error = 'injected capability failure'
failed_finish.after_ack()
completion_error = nil
assert(completion_failure_state.safe_quit[completion_proof_uid], 'completion failure discarded rollback proof')
selected.fail_attach = true
table.remove(scheduled)()
assert(completion_failure_state.safe_quit[completion_proof_uid] and selected.current == 'Detached')
selected.fail_attach = false
table.remove(scheduled)()
assert(completion_failure_state.safe_quit[completion_proof_uid] == nil and selected.current == 'Attached')
assert(#logged_errors > 0 and logged_errors[#logged_errors]:match 'retrying')

-- Caller death is represented by running the proof-owned expiry timer. It
-- likewise survives one transient rollback failure and eventually restores.
local expiry_state = {}
local expiry_uid = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd'
create_attached_quit_proof(expiry_uid, expiry_state)
selected.fail_attach = true
table.remove(scheduled)()
assert(expiry_state.safe_quit[expiry_uid] and selected.current == 'Detached')
selected.fail_attach = false
table.remove(scheduled)()
assert(expiry_state.safe_quit[expiry_uid] == nil and selected.current == 'Attached')

-- If a detached route name rebinds to a different epoch after attach, the
-- rollback immediately contains it by detaching again before reporting the
-- failure; the proof-owned retry can restore only after the exact epoch is
-- visible again.
local rebound_state = {}
local rebound_uid = 'eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee'
create_attached_quit_proof(rebound_uid, rebound_state)
local rebound_result, rebound_error
local rebound_now = 100
os.time = function()
  return rebound_now
end
windows = { sentinel_window_for('dmux-b-usb', '66666666-6666-4666-8666-666666666666') }
presentation.dispatch(
  {
    action = 'safe_quit',
    target = { phase = 'rollback', proof_uid = rebound_uid },
    expiry = 110,
  },
  rebound_state,
  function(ok, err)
    rebound_result, rebound_error = ok, err
  end
)
assert(not rebound_result and not rebound_error)
rebound_now = 105
table.remove(scheduled)()
assert(not rebound_result and rebound_error and selected.current == 'Detached')
assert(rebound_state.safe_quit[rebound_uid], 'rebound containment discarded the recovery proof')
os.time = real_time
windows = { sentinel_window }
table.remove(scheduled)()
assert(rebound_state.safe_quit[rebound_uid] == nil and selected.current == 'Attached')

-- Every ClientDomain::attach() leaks a TermWizTerminalDomain that reports
-- Attached forever and refuses detach(). Safe-quit must survive any number of
-- them, in any position, with or without their own panes, while still refusing
-- a domain that is genuinely spawnable and outside the configuration.
local function detach_attempt()
  selected.current = 'Attached'
  selected.panes = true
  selected.fail_attach = false
  alternate.current = 'Detached'
  alternate.panes = false
  windows = { sentinel_window }
  scheduled = {}
  return dispatch(
    'safe_quit',
    { phase = 'detach', domains = { usb_incarnation } },
    { safe_quit = {}, persistent_domains = { 'dmux-b-ts', 'dmux-b-usb' } }
  )
end

local leak_result, leak_error = detach_attempt()
assert(leak_result and not leak_error, 'baseline detach must succeed')

local first_leak = placeholder(false)
leak_result, leak_error = detach_attempt()
assert(
  leak_result and not leak_error,
  'one connection UI refused safe-quit: ' .. tostring(leak_error and leak_error.code)
)
assert(selected.detaches > 0 and first_leak.current == 'Attached', 'the placeholder must be left untouched')

-- A detach then re-attach cycle produces a second row under the same name.
local second_leak = placeholder(false)
leak_result, leak_error = detach_attempt()
assert(leak_result and not leak_error, 'a duplicate placeholder name refused safe-quit')

-- An open connection UI owns a real pane of its own.
second_leak.panes = true
leak_result, leak_error = detach_attempt()
assert(leak_result and not leak_error, 'a placeholder holding panes refused safe-quit')

-- Position must not matter: the guard returns on first refusal, so a leading
-- placeholder exercises a different path from a trailing one.
table.insert(rows, 1, table.remove(rows))
leak_result, leak_error = detach_attempt()
assert(leak_result and not leak_error, 'a leading placeholder refused safe-quit')

-- A rogue domain that really is spawnable is still policed, and so is one whose
-- capability cannot be proved: an exemption is never granted by default.
local rogue_spawnable = domain('rogue-spawnable', 'Attached', true)
leak_result, leak_error = detach_attempt()
assert(not leak_result and leak_error.code == 'unknown_persistent_domain', 'a spawnable rogue domain was exempted')
remove_domain(rogue_spawnable)

local rogue_silent = domain('rogue-no-capability', 'Attached')
leak_result, leak_error = detach_attempt()
assert(
  not leak_result and leak_error.code == 'unknown_persistent_domain',
  'a domain without the capability was exempted'
)
remove_domain(rogue_silent)

local rogue_throwing = domain('rogue-throwing', 'Attached', 'error')
leak_result, leak_error = detach_attempt()
assert(
  not leak_result and leak_error.code == 'unknown_persistent_domain',
  'a throwing capability was read as an exemption'
)
remove_domain(rogue_throwing)

-- A configured domain is answerable to the bridge whatever it reports, or the
-- capability becomes a way for a real route to opt out of being proven.
selected.spawnable = false
leak_result, leak_error = detach_attempt()
assert(leak_result and not leak_error, 'a configured domain was exempted by its own capability')
assert(selected.detaches > 0 and selected.current == 'Detached')
selected.spawnable = true
remove_domain(first_leak)
remove_domain(second_leak)

io.stdout:write 'dmux presentation test: no-create attach/focus/safe-quit passed\n'

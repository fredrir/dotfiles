-- Shared mux domain inventory predicate for the GUI-side guards.
--
-- Every `ClientDomain::attach()` spawns a WezTerm connection UI, which
-- registers a `TermWizTerminalDomain` in the mux. That placeholder reports
-- `Attached` forever, refuses `detach()`, and the mux has no removal API
-- (upstream `termwiztermtab.rs` still carries a `// TODO: make a singleton`),
-- so one leaks per attach for the life of the process. Any guard that walks
-- `mux.all_domains()` and assumes every row is a routable persistent domain
-- therefore refuses permanently after the first attach, and a detach then
-- re-attach cycle leaves two rows sharing one name.
--
-- `spawnable()` is an exact discriminator rather than a heuristic:
-- `TermWizTerminalDomain` is the only implementation in the whole tree that
-- returns false, the trait default is true, and upstream's own launcher
-- filters on exactly this capability.
local M = {}

---Build the sanitized configured-domain lookup shared by both inventories.
---Returns nil when the authority is unavailable so callers fail closed rather
---than treating an absent configuration as "nothing is configured".
---@param persistent_domains string[]|nil sanitized dmux persistent domain names
---@return table<string, boolean>|nil
function M.configured_set(persistent_domains)
  if type(persistent_domains) ~= 'table' then
    return nil
  end
  local configured = {}
  for _, name in ipairs(persistent_domains) do
    configured[name] = true
  end
  return configured
end

---Answer "is this domain one the bridge is answerable for?".
---@param domain any mux domain handle
---@param name string domain name, already validated by the caller
---@param configured table<string, boolean> from `M.configured_set`
---@return boolean
function M.routable(domain, name, configured)
  if name == 'local' or configured[name] then
    -- Never exempt local or a configured domain. Those are exactly the routes
    -- the bridge must prove the state of, whatever capability they report.
    return true
  end
  local ok, spawnable = pcall(function()
    return domain:is_spawnable()
  end)
  -- Fail closed: a capability that cannot be proved is never an exemption, so
  -- an unknown spawnable domain is still policed as a rogue route.
  return not (ok and spawnable == false)
end

return M

local wezterm = require 'wezterm'

local M = {}

function M.join(...)
  local parts = { ... }
  local path = table.remove(parts, 1) or ''
  for _, part in ipairs(parts) do
    path = path:gsub('/+$', '') .. '/' .. tostring(part):gsub('^/+', '')
  end
  return path
end

function M.read(path, maximum)
  local file = io.open(path, 'rb')
  if not file then
    return nil, 'not_found'
  end
  local body, read_err = file:read(maximum and maximum + 1 or '*a')
  file:close()
  if body == nil then
    if read_err then
      return nil, 'read_failed: ' .. tostring(read_err)
    end
    -- Lua reports EOF-before-any-byte as nil for a bounded read. Preserve an
    -- empty document so callers can classify it as an empty/malformed key or
    -- JSON message instead of crashing the watchdog on `#body` below.
    body = ''
  end
  if maximum and #body > maximum then
    return nil, 'too_large'
  end
  return body
end

local function run(argv)
  local ok, success, _, stderr = pcall(wezterm.run_child_process, argv)
  if not ok then
    return nil, tostring(success)
  end
  if not success then
    return nil, tostring(stderr)
  end
  return true
end

function M.ensure_private_dirs(paths)
  for _, path in ipairs(paths) do
    local ok, err = run { '/bin/mkdir', '-p', '-m', '0700', path }
    if not ok then
      return nil, 'mkdir ' .. path .. ': ' .. err
    end
    ok, err = run { '/bin/chmod', '0700', path }
    if not ok then
      return nil, 'chmod ' .. path .. ': ' .. err
    end
  end
  return true
end

function M.write_private_atomic(path, body)
  local tmp = path .. '.tmp'
  local file, err = io.open(tmp, 'wb')
  if not file then
    return nil, err
  end
  local ok, write_err = file:write(body)
  if not ok then
    file:close()
    os.remove(tmp)
    return nil, write_err
  end
  file:flush()
  file:close()
  local mode_ok, mode_err = run { '/bin/chmod', '0600', tmp }
  if not mode_ok then
    os.remove(tmp)
    return nil, mode_err
  end
  local renamed, rename_err = os.rename(tmp, path)
  if not renamed then
    os.remove(tmp)
    return nil, rename_err
  end
  return true
end

return M

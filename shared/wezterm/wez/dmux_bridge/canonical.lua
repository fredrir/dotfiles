-- Canonical JSON used as the HMAC input for bridge protocol v1.
--
-- Objects are recursively byte-lexicographically key sorted, arrays retain
-- order, separators are compact, and only integer JSON numbers are accepted.
-- Optional fields are omitted rather than encoded as null. This deliberately
-- small RFC8259 subset is identical in Lua and Rust and avoids relying on a
-- JSON encoder's unspecified object iteration order.
local M = {}

-- Lua normally cannot distinguish an empty JSON array from an empty object.
-- The bridge decoder marks arrays with this private metatable so an empty
-- `domains: []` remains byte-for-byte canonical instead of becoming `{}`.
local ARRAY_METATABLE = {}

function M.array(value)
  if type(value) ~= 'table' then
    error('canonical.array requires a table', 2)
  end
  return setmetatable(value, ARRAY_METATABLE)
end

function M.is_array(value)
  return type(value) == 'table' and getmetatable(value) == ARRAY_METATABLE
end

local ESCAPES = {
  ['\b'] = '\\b',
  ['\t'] = '\\t',
  ['\n'] = '\\n',
  ['\f'] = '\\f',
  ['\r'] = '\\r',
  ['"'] = '\\"',
  ['\\'] = '\\\\',
}

local function quote(value)
  return '"'
    .. value:gsub('[%z\1-\31\\"]', function(char)
      return ESCAPES[char] or string.format('\\u%04x', char:byte())
    end)
    .. '"'
end

local function table_shape(value)
  local count = 0
  local max_index = 0
  local has_string = false
  local has_number = false
  for key in pairs(value) do
    count = count + 1
    if type(key) == 'string' then
      has_string = true
    elseif type(key) == 'number' and key >= 1 and key % 1 == 0 then
      has_number = true
      max_index = math.max(max_index, key)
    else
      return nil, 'object keys must be strings; array keys must be positive integers'
    end
  end
  if M.is_array(value) then
    if has_string or (has_number and max_index ~= count) then
      return nil, 'marked array must have dense positive integer keys'
    end
    return 'array'
  end
  if count == 0 then
    return 'object'
  end
  if has_string and has_number then
    return nil, 'mixed object/array table'
  end
  if has_number and max_index ~= count then
    return nil, 'sparse array'
  end
  return has_number and 'array' or 'object'
end

local encode

local function encode_table(value)
  local shape, err = table_shape(value)
  if not shape then
    return nil, err
  end
  if shape == 'array' then
    local out = {}
    for index = 1, #value do
      local item, item_err = encode(value[index])
      if not item then
        return nil, string.format('array[%d]: %s', index, item_err)
      end
      out[index] = item
    end
    return '[' .. table.concat(out, ',') .. ']'
  end

  local keys = {}
  for key in pairs(value) do
    table.insert(keys, key)
  end
  table.sort(keys)
  local out = {}
  for index, key in ipairs(keys) do
    local item, item_err = encode(value[key])
    if not item then
      return nil, string.format('object.%s: %s', key, item_err)
    end
    out[index] = quote(key) .. ':' .. item
  end
  return '{' .. table.concat(out, ',') .. '}'
end

encode = function(value)
  local kind = type(value)
  if kind == 'string' then
    return quote(value)
  end
  if kind == 'boolean' then
    return value and 'true' or 'false'
  end
  if kind == 'number' then
    if value ~= value or value == math.huge or value == -math.huge or value % 1 ~= 0 then
      return nil, 'only finite integer numbers are allowed'
    end
    if math.abs(value) > 9007199254740991 then
      return nil, 'integer exceeds the exactly representable JSON range'
    end
    return string.format('%.0f', value)
  end
  if kind == 'table' then
    return encode_table(value)
  end
  return nil, 'unsupported JSON type: ' .. kind
end

function M.encode(value)
  return encode(value)
end

function M.signing_document(request)
  if type(request) ~= 'table' then
    return nil, 'request must be an object'
  end
  local unsigned = {}
  for key, value in pairs(request) do
    if key ~= 'hmac_sha256' then
      unsigned[key] = value
    end
  end
  return M.encode(unsigned)
end

return M

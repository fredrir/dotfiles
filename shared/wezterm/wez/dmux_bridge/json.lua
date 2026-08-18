-- Strict JSON codec for signed bridge documents.
--
-- WezTerm's general JSON decoder represents both [] and {} as an empty Lua
-- table.  That ambiguity is unsafe for a signed schema, because canonical
-- JSON must preserve the original shape.  This decoder marks every array
-- with canonical.array(), rejects duplicate object keys/null/fractional
-- numbers, and accepts only exactly representable integer numbers.
local canonical = require 'wez.dmux_bridge.canonical'

local M = {}

local function utf8_char(codepoint)
  if codepoint <= 0x7f then
    return string.char(codepoint)
  end
  if codepoint <= 0x7ff then
    return string.char(0xc0 + math.floor(codepoint / 0x40), 0x80 + codepoint % 0x40)
  end
  if codepoint <= 0xffff then
    return string.char(
      0xe0 + math.floor(codepoint / 0x1000),
      0x80 + math.floor(codepoint / 0x40) % 0x40,
      0x80 + codepoint % 0x40
    )
  end
  return string.char(
    0xf0 + math.floor(codepoint / 0x40000),
    0x80 + math.floor(codepoint / 0x1000) % 0x40,
    0x80 + math.floor(codepoint / 0x40) % 0x40,
    0x80 + codepoint % 0x40
  )
end

local function decoder(raw)
  if type(raw) ~= 'string' then
    return nil, 'JSON document must be a string'
  end
  local position = 1
  local length = #raw
  local depth = 0

  local function fail(message)
    return nil, string.format('%s at byte %d', message, position)
  end

  local function skip_space()
    while position <= length do
      local byte = raw:byte(position)
      if byte ~= 0x20 and byte ~= 0x09 and byte ~= 0x0a and byte ~= 0x0d then
        break
      end
      position = position + 1
    end
  end

  local function read_hex(offset)
    local value = raw:sub(offset, offset + 3)
    if #value ~= 4 or not value:match '^[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]$' then
      return nil
    end
    return tonumber(value, 16)
  end

  local function parse_string()
    if raw:byte(position) ~= 0x22 then
      return fail 'expected string'
    end
    position = position + 1
    local out = {}
    local chunk_start = position
    while position <= length do
      local byte = raw:byte(position)
      if byte == 0x22 then
        if position > chunk_start then
          table.insert(out, raw:sub(chunk_start, position - 1))
        end
        position = position + 1
        local value = table.concat(out)
        if utf8 and utf8.len and not utf8.len(value) then
          return fail 'string is not valid UTF-8'
        end
        return value
      end
      if byte < 0x20 then
        return fail 'unescaped control byte in string'
      end
      if byte == 0x5c then
        if position > chunk_start then
          table.insert(out, raw:sub(chunk_start, position - 1))
        end
        position = position + 1
        local escape = raw:sub(position, position)
        local simple = {
          ['"'] = '"',
          ['\\'] = '\\',
          ['/'] = '/',
          b = '\b',
          f = '\f',
          n = '\n',
          r = '\r',
          t = '\t',
        }
        if simple[escape] then
          table.insert(out, simple[escape])
          position = position + 1
        elseif escape == 'u' then
          local codepoint = read_hex(position + 1)
          if not codepoint then
            return fail 'invalid unicode escape'
          end
          position = position + 5
          if codepoint >= 0xd800 and codepoint <= 0xdbff then
            if raw:sub(position, position + 1) ~= '\\u' then
              return fail 'high surrogate is not followed by a low surrogate'
            end
            local low = read_hex(position + 2)
            if not low or low < 0xdc00 or low > 0xdfff then
              return fail 'invalid low surrogate'
            end
            codepoint = 0x10000 + (codepoint - 0xd800) * 0x400 + (low - 0xdc00)
            position = position + 6
          elseif codepoint >= 0xdc00 and codepoint <= 0xdfff then
            return fail 'unpaired low surrogate'
          end
          table.insert(out, utf8_char(codepoint))
        else
          return fail 'invalid string escape'
        end
        chunk_start = position
      else
        position = position + 1
      end
    end
    return fail 'unterminated string'
  end

  local parse_value

  local function parse_array()
    position = position + 1
    skip_space()
    local out = canonical.array {}
    if raw:byte(position) == 0x5d then
      position = position + 1
      return out
    end
    local index = 1
    while true do
      local value, err = parse_value()
      if err then
        return nil, err
      end
      out[index] = value
      index = index + 1
      skip_space()
      local byte = raw:byte(position)
      if byte == 0x5d then
        position = position + 1
        return out
      end
      if byte ~= 0x2c then
        return fail 'expected comma or closing bracket'
      end
      position = position + 1
      skip_space()
    end
  end

  local function parse_object()
    position = position + 1
    skip_space()
    local out = {}
    local seen = {}
    if raw:byte(position) == 0x7d then
      position = position + 1
      return out
    end
    while true do
      local key, key_err = parse_string()
      if key_err then
        return nil, key_err
      end
      if seen[key] then
        return fail 'duplicate object key'
      end
      seen[key] = true
      skip_space()
      if raw:byte(position) ~= 0x3a then
        return fail 'expected colon'
      end
      position = position + 1
      skip_space()
      local value, value_err = parse_value()
      if value_err then
        return nil, value_err
      end
      out[key] = value
      skip_space()
      local byte = raw:byte(position)
      if byte == 0x7d then
        position = position + 1
        return out
      end
      if byte ~= 0x2c then
        return fail 'expected comma or closing brace'
      end
      position = position + 1
      skip_space()
    end
  end

  local function parse_integer()
    local start = position
    if raw:byte(position) == 0x2d then
      position = position + 1
    end
    local first = raw:byte(position)
    if first == 0x30 then
      position = position + 1
      local following = raw:byte(position)
      if following and following >= 0x30 and following <= 0x39 then
        return fail 'integer has a leading zero'
      end
    elseif first and first >= 0x31 and first <= 0x39 then
      repeat
        position = position + 1
        first = raw:byte(position)
      until not first or first < 0x30 or first > 0x39
    else
      return fail 'invalid JSON number'
    end
    local suffix = raw:sub(position, position)
    if suffix == '.' or suffix == 'e' or suffix == 'E' then
      return fail 'only integer JSON numbers are allowed'
    end
    local value = tonumber(raw:sub(start, position - 1))
    -- Two-sided, not math.abs: integer arithmetic wraps, so abs(math.mininteger)
    -- is math.mininteger, and an absolute-value bound lets that literal through.
    if not value or value % 1 ~= 0 or value < -9007199254740991 or value > 9007199254740991 then
      return fail 'integer exceeds the exactly representable JSON range'
    end
    return value
  end

  parse_value = function()
    depth = depth + 1
    if depth > 128 then
      return fail 'JSON nesting exceeds bridge limit'
    end
    skip_space()
    local byte = raw:byte(position)
    local value, err
    if byte == 0x7b then
      value, err = parse_object()
    elseif byte == 0x5b then
      value, err = parse_array()
    elseif byte == 0x22 then
      value, err = parse_string()
    elseif raw:sub(position, position + 3) == 'true' then
      position = position + 4
      value = true
    elseif raw:sub(position, position + 4) == 'false' then
      position = position + 5
      value = false
    elseif raw:sub(position, position + 3) == 'null' then
      value, err = fail 'null is not part of bridge JSON v1'
    elseif byte == 0x2d or (byte and byte >= 0x30 and byte <= 0x39) then
      value, err = parse_integer()
    else
      value, err = fail 'invalid JSON value'
    end
    depth = depth - 1
    return value, err
  end

  skip_space()
  local value, err = parse_value()
  if err then
    return nil, err
  end
  skip_space()
  if position <= length then
    return fail 'trailing content'
  end
  return value
end

function M.decode(raw)
  return decoder(raw)
end

function M.encode(value)
  return canonical.encode(value)
end

function M.array(value)
  return canonical.array(value or {})
end

function M.is_array(value)
  return canonical.is_array(value)
end

return M

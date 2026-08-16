-- Dependency-free SHA-256/HMAC-SHA256 for the local dmux bridge.
--
-- WezTerm embeds Lua 5.4, so the native integer bit operators are available.
-- Keeping this here avoids invoking a shell or accepting an unsigned request
-- while a helper process starts.
local M = {}

local MASK = 0xffffffff
local K = {
  0x428a2f98,
  0x71374491,
  0xb5c0fbcf,
  0xe9b5dba5,
  0x3956c25b,
  0x59f111f1,
  0x923f82a4,
  0xab1c5ed5,
  0xd807aa98,
  0x12835b01,
  0x243185be,
  0x550c7dc3,
  0x72be5d74,
  0x80deb1fe,
  0x9bdc06a7,
  0xc19bf174,
  0xe49b69c1,
  0xefbe4786,
  0x0fc19dc6,
  0x240ca1cc,
  0x2de92c6f,
  0x4a7484aa,
  0x5cb0a9dc,
  0x76f988da,
  0x983e5152,
  0xa831c66d,
  0xb00327c8,
  0xbf597fc7,
  0xc6e00bf3,
  0xd5a79147,
  0x06ca6351,
  0x14292967,
  0x27b70a85,
  0x2e1b2138,
  0x4d2c6dfc,
  0x53380d13,
  0x650a7354,
  0x766a0abb,
  0x81c2c92e,
  0x92722c85,
  0xa2bfe8a1,
  0xa81a664b,
  0xc24b8b70,
  0xc76c51a3,
  0xd192e819,
  0xd6990624,
  0xf40e3585,
  0x106aa070,
  0x19a4c116,
  0x1e376c08,
  0x2748774c,
  0x34b0bcb5,
  0x391c0cb3,
  0x4ed8aa4a,
  0x5b9cca4f,
  0x682e6ff3,
  0x748f82ee,
  0x78a5636f,
  0x84c87814,
  0x8cc70208,
  0x90befffa,
  0xa4506ceb,
  0xbef9a3f7,
  0xc67178f2,
}

local function rotr(value, count)
  return ((value >> count) | (value << (32 - count))) & MASK
end

local function u32be(value)
  return string.char((value >> 24) & 0xff, (value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff)
end

local function padded(message)
  local bit_len = #message * 8
  local high = math.floor(bit_len / 0x100000000)
  local low = bit_len & MASK
  local zeroes = (56 - ((#message + 1) % 64)) % 64
  return message .. '\x80' .. string.rep('\0', zeroes) .. u32be(high) .. u32be(low)
end

function M.sha256_bytes(message)
  assert(type(message) == 'string', 'sha256 input must be a string')
  local h0 = 0x6a09e667
  local h1 = 0xbb67ae85
  local h2 = 0x3c6ef372
  local h3 = 0xa54ff53a
  local h4 = 0x510e527f
  local h5 = 0x9b05688c
  local h6 = 0x1f83d9ab
  local h7 = 0x5be0cd19
  local data = padded(message)

  for offset = 1, #data, 64 do
    local w = {}
    for index = 0, 15 do
      local pos = offset + index * 4
      w[index] = ((data:byte(pos) << 24) | (data:byte(pos + 1) << 16) | (data:byte(pos + 2) << 8) | data:byte(pos + 3))
        & MASK
    end
    for index = 16, 63 do
      local x = w[index - 15]
      local y = w[index - 2]
      local s0 = rotr(x, 7) ~ rotr(x, 18) ~ (x >> 3)
      local s1 = rotr(y, 17) ~ rotr(y, 19) ~ (y >> 10)
      w[index] = (w[index - 16] + s0 + w[index - 7] + s1) & MASK
    end

    local a, b, c, d = h0, h1, h2, h3
    local e, f, g, h = h4, h5, h6, h7
    for index = 0, 63 do
      local s1 = rotr(e, 6) ~ rotr(e, 11) ~ rotr(e, 25)
      local choose = (e & f) ~ (~e & g)
      local temp1 = (h + s1 + choose + K[index + 1] + w[index]) & MASK
      local s0 = rotr(a, 2) ~ rotr(a, 13) ~ rotr(a, 22)
      local majority = (a & b) ~ (a & c) ~ (b & c)
      local temp2 = (s0 + majority) & MASK
      h, g, f, e = g, f, e, (d + temp1) & MASK
      d, c, b, a = c, b, a, (temp1 + temp2) & MASK
    end

    h0 = (h0 + a) & MASK
    h1 = (h1 + b) & MASK
    h2 = (h2 + c) & MASK
    h3 = (h3 + d) & MASK
    h4 = (h4 + e) & MASK
    h5 = (h5 + f) & MASK
    h6 = (h6 + g) & MASK
    h7 = (h7 + h) & MASK
  end

  return u32be(h0) .. u32be(h1) .. u32be(h2) .. u32be(h3) .. u32be(h4) .. u32be(h5) .. u32be(h6) .. u32be(h7)
end

function M.hex(bytes)
  return (bytes:gsub('.', function(byte)
    return string.format('%02x', byte:byte())
  end))
end

function M.sha256(message)
  return M.hex(M.sha256_bytes(message))
end

function M.hmac_sha256(key, message)
  assert(type(key) == 'string', 'hmac key must be a string')
  assert(type(message) == 'string', 'hmac input must be a string')
  if #key > 64 then
    key = M.sha256_bytes(key)
  end
  key = key .. string.rep('\0', 64 - #key)
  local outer = {}
  local inner = {}
  for index = 1, 64 do
    local byte = key:byte(index)
    outer[index] = string.char(byte ~ 0x5c)
    inner[index] = string.char(byte ~ 0x36)
  end
  local digest = M.sha256_bytes(table.concat(inner) .. message)
  return M.hex(M.sha256_bytes(table.concat(outer) .. digest))
end

-- Compare all bytes before returning. Length is folded into the accumulator
-- so a malformed short signature takes the same path as a wrong full one.
function M.constant_time_equal(left, right)
  if type(left) ~= 'string' or type(right) ~= 'string' then
    return false
  end
  local length = math.max(#left, #right)
  local different = #left ~ #right
  for index = 1, length do
    different = different | ((left:byte(index) or 0) ~ (right:byte(index) or 0))
  end
  return different == 0
end

return M

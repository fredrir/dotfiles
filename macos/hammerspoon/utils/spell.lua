local M = {}

local dspell = os.getenv "HOME" .. "/dotfiles/.bin/dspell"
local menu = hs.menubar.new(false)
local busy = false

---@class Typo
---@field word string
---@field length integer
---@field offset integer
---@field guesses string[]

---@param predicate fun(): boolean
---@param timeout number
---@return boolean
local function waitFor(predicate, timeout)
  local deadline = hs.timer.secondsSinceEpoch() + timeout
  while not predicate() do
    if hs.timer.secondsSinceEpoch() > deadline then
      return false
    end
    hs.coroutineApplicationYield(0.01)
  end
  return true
end

---@param key string
---@param times? integer
---@param mods? string[]
local function press(key, times, mods)
  for _ = 1, times or 1 do
    hs.eventtap.keyStroke(mods or {}, key, 0)
  end
end

---@param graphemes integer
local function move(graphemes)
  press(graphemes > 0 and "right" or "left", math.abs(graphemes))
end

---@return string|nil
local function copy()
  local count = hs.pasteboard.changeCount()
  press("c", 1, { "cmd" })
  local copied = waitFor(function()
    return hs.pasteboard.changeCount() ~= count
  end, 0.5)
  return copied and (hs.pasteboard.getContents() or "") or nil
end

---@param edge string
---@param back string
---@return string
local function copyTo(edge, back)
  press(edge, 1, { "cmd", "shift" })
  local text = copy()
  if text then
    press(back)
  end
  return text or ""
end

---@param before string
---@param after string
---@return Typo|nil
local function check(before, after)
  local code, stdout
  local task = hs.task.new(dspell, function(exitCode, output)
    code, stdout = exitCode, output
  end, { "--", before, after })
  if not (task and task:start()) then
    return nil
  end
  if not waitFor(function()
    return code ~= nil
  end, 2) then
    task:terminate()
    return nil
  end
  return code == 0 and hs.json.decode(stdout) or nil
end

---@param element hs.axuielement
---@return table|nil
local function caretBounds(element)
  local range = element:attributeValue "AXSelectedTextRange"
  local bounds = range and element:parameterizedAttributeValue("AXBoundsForRange", range)
  if bounds and bounds.h > 0 then
    return bounds
  end
  local markers = element:attributeValue "AXSelectedTextMarkerRange"
  bounds = markers and element:parameterizedAttributeValue("AXBoundsForTextMarkerRange", markers)
  if bounds and bounds.h > 0 then
    return bounds
  end
end

---@return table
local function caret()
  local app = hs.axuielement.applicationElement(hs.application.frontmostApplication())
  -- Electron apps only expose their text to accessibility clients that ask for it.
  app:setAttributeValue("AXManualAccessibility", true)
  local element = app:attributeValue "AXFocusedUIElement"
  local ok, bounds = pcall(caretBounds, element)
  if ok and bounds then
    return { x = bounds.x, y = bounds.y + bounds.h }
  end
  return hs.mouse.absolutePosition()
end

---@param guesses string[]
---@return string|nil
local function choose(guesses)
  local point = caret()
  local guess
  local closed = false
  menu:setMenu(hs.fnutils.imap(guesses, function(title)
    return {
      title = title,
      fn = function()
        guess = title
      end,
    }
  end))
  M.popup = hs.timer.doAfter(0, function()
    menu:popupMenu(point)
    closed = true
  end)
  waitFor(function()
    return closed
  end, math.huge)
  M.popup = nil
  waitFor(function()
    return guess ~= nil
  end, 0.1)
  return guess
end

---@param typo Typo
---@param guess string
local function replace(typo, guess)
  move(typo.offset)
  press("left", typo.length, { "shift" })
  if copy() ~= typo.word then
    press "right"
    move(-typo.offset)
    return
  end
  hs.eventtap.keyStrokes(guess)
  move(math.max(0, -typo.offset))
end

local function fix()
  local before = copyTo("left", "right")
  local after = copyTo("right", "left")
  local typo = check(before, after)
  if not typo or #typo.guesses == 0 then
    return
  end
  local guess = #typo.guesses == 1 and typo.guesses[1] or choose(typo.guesses)
  if guess then
    replace(typo, guess)
  end
end

function M.fix()
  if busy then
    return
  end
  busy = true
  coroutine.wrap(function()
    local clipboard = hs.pasteboard.readAllData()
    local count = hs.pasteboard.changeCount()
    local ok, err = pcall(fix)
    if hs.pasteboard.changeCount() ~= count then
      hs.pasteboard.writeAllData(clipboard)
    end
    busy = false
    if not ok then
      hs.showError(err)
    end
  end)()
end

return M
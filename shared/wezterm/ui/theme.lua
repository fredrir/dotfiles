local wezterm = require "wezterm"

local MIN_TEXT_CONTRAST = 4.5

---@param candidate string
---@param fill Color
---@return number
local function contrast(candidate, fill)
  return wezterm.color.parse(candidate):contrast_ratio(fill)
end

---@param colors DotfileThemeColors
---@return string
local function selection_foreground(colors)
  local fill = wezterm.color.parse(colors.primary)
  for _, candidate in ipairs { colors.foreground, colors.background } do
    if contrast(candidate, fill) >= MIN_TEXT_CONTRAST then
      return candidate
    end
  end

  local black = contrast("#000000", fill)
  local white = contrast("#ffffff", fill)
  return black >= white and "#000000" or "#ffffff"
end

---@param colors DotfileThemeColors
---@return Palette
local function palette(colors)
  return {
    foreground = colors.foreground,
    background = colors.background,
    cursor_bg = colors.foreground,
    cursor_fg = colors.background,
    cursor_border = colors.foreground,
    selection_fg = selection_foreground(colors),
    selection_bg = colors.primary,
    ansi = colors.ansi,
    brights = colors.brights,
  }
end

---@param themes table<string, DotfileThemeColors>
---@return table<string, Palette>
local function schemes(themes)
  local resolved = {}
  for name, colors in pairs(themes) do
    resolved[name] = palette(colors)
  end
  return resolved
end

return {
  palette = palette,
  schemes = schemes,
}

local platform = require 'wez.platform'

-- "Mocha, Properly": canonical Catppuccin Mocha with real bright variants
-- and a per-host accent identity. Hand-crafted, deliberately diverging from
-- the theme generator (wez/theme.lua) until the design settles; fold the
-- final values back into theme/profiles/ afterwards.
local M = {}

M.host = platform.pick {
    mac = 'macie',
    default = 'archie'
}

-- Warm vs cool reads in peripheral vision: rosewater macie, blue archie.
-- Carried by the cursor and the always-on host chip in the status bar.
M.accent = platform.pick {
    mac = '#f5e0dc',
    default = '#89b4fa'
}

M.palette = {
    crust = '#11111b',
    mantle = '#181825',
    base = '#1e1e2e',
    surface0 = '#313244',
    surface1 = '#45475a',
    surface2 = '#585b70',
    overlay0 = '#6c7086',
    overlay1 = '#7f849c',
    overlay2 = '#9399b2',
    subtext0 = '#a6adc8',
    subtext1 = '#bac2de',
    text = '#cdd6f4',
    lavender = '#b4befe',
    blue = '#89b4fa',
    sapphire = '#74c7ec',
    sky = '#89dceb',
    teal = '#94e2d5',
    green = '#a6e3a1',
    yellow = '#f9e2af',
    peach = '#fab387',
    maroon = '#eba0ac',
    red = '#f38ba8',
    mauve = '#cba6f7',
    pink = '#f5c2e7',
    flamingo = '#f2cdcd',
    rosewater = '#f5e0dc'
}

M.colors = {
    foreground = M.palette.text,
    background = M.palette.base,
    cursor_bg = M.accent,
    cursor_fg = M.palette.crust,
    cursor_border = M.accent,
    -- Selection is a lit surface, not an inversion; the cursor stays the
    -- sharpest thing on screen.
    selection_bg = M.palette.surface1,
    selection_fg = M.palette.text,
    ansi = {'#45475a', '#f38ba8', '#a6e3a1', '#f9e2af', '#89b4fa', '#f5c2e7', '#94e2d5', '#bac2de'},
    brights = {'#585b70', '#f37799', '#89d88b', '#ebd391', '#74a8fc', '#f2aede', '#6bd7ca', '#a6adc8'},
    indexed = {
        [16] = '#fab387',
        [17] = '#f5e0dc'
    }
}

-- Bar background equals the window background so tabs float instead of
-- sitting on a seam; the active tab is calm surface hierarchy, not a beacon.
M.tabs = {
    background = M.palette.base,
    active_bg = M.palette.surface0,
    active_fg = M.palette.text,
    inactive_bg = M.palette.mantle,
    inactive_fg = M.palette.overlay1,
    hover_bg = '#25253a',
    hover_fg = M.palette.subtext1,
    index_active = M.palette.lavender,
    index_inactive = M.palette.surface2,
    badge = M.palette.overlay2
}

-- JetBrains Mono everywhere: clean build plus symbols on macie, the nerd
-- variant on archie, Hack Nerd as the glyph backstop either way.
M.fonts = {'JetBrains Mono', 'JetBrainsMono Nerd Font Mono', 'Symbols Nerd Font Mono', 'Hack Nerd Font Mono'}

M.sizes = {
    terminal = platform.pick {
        mac = 13.0,
        default = 11.5
    },
    palette = platform.pick {
        mac = 14.0,
        default = 13.0
    }
}

return M

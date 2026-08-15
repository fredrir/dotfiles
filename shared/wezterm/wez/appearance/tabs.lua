local wezterm = require 'wezterm'
local theme = require 'wez.theme'

local M = {}

local LEFT_CAP = '\u{e0b6}'
local RIGHT_CAP = '\u{e0b4}'

function M.apply(config)
    -- TODO Redesign and Reimplement
    config.use_fancy_tab_bar = false
    config.tab_bar_at_bottom = false
    config.hide_tab_bar_if_only_one_tab = false
    config.show_new_tab_button_in_tab_bar = false
    config.tab_max_width = 32

    config.colors.tab_bar = {
        background = theme.tabs.bar_background,
        active_tab = {
            bg_color = theme.tabs.active_background,
            fg_color = theme.tabs.active_foreground
        },
        inactive_tab = {
            bg_color = theme.tabs.inactive_background,
            fg_color = theme.tabs.inactive_foreground
        }
    }
end

local function tab_title(tab)
    local title = tab.tab_title
    if title and #title > 0 then
        return title
    end
    return tab.active_pane.title
end

function M.setup()
    wezterm.on('format-tab-title', function(tab, _tabs, _panes, _config, _hover, max_width)
        local bg = tab.is_active and theme.tabs.active_background or theme.tabs.inactive_background
        local fg = tab.is_active and theme.tabs.active_foreground or theme.tabs.inactive_foreground
        local label = string.format(' %d: %s ', tab.tab_index + 1, tab_title(tab))

        return {{
            Background = {
                Color = theme.tabs.bar_background
            }
        }, {
            Foreground = {
                Color = bg
            }
        }, {
            Text = LEFT_CAP
        }, {
            Background = {
                Color = bg
            }
        }, {
            Foreground = {
                Color = fg
            }
        }, {
            Text = wezterm.truncate_right(label, math.max(max_width - 2, 1))
        }, {
            Background = {
                Color = theme.tabs.bar_background
            }
        }, {
            Foreground = {
                Color = bg
            }
        }, {
            Text = RIGHT_CAP
        }}
    end)
end

return M

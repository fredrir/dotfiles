local wezterm = require 'wezterm'
local theme = require 'wez.theme'

local M = {}

function M.setup()
    wezterm.on('update-status', function(window, pane)
        local segments = {}

        local domain = pane and pane:get_domain_name() or nil
        if domain and domain ~= 'local' then
            table.insert(segments, {
                fg = theme.palette.peach,
                text = domain
            })
        end

        local workspace = window:active_workspace()
        if workspace and workspace ~= 'default' then
            table.insert(segments, {
                fg = theme.palette.mauve,
                text = workspace
            })
        end

        if window:leader_is_active() then
            table.insert(segments, {
                fg = theme.palette.red,
                text = 'LEADER'
            })
        end

        local elements = {}
        for index, segment in ipairs(segments) do
            if index > 1 then
                table.insert(elements, {
                    Foreground = {
                        Color = theme.palette.overlay
                    }
                })
                table.insert(elements, {
                    Text = ' · '
                })
            end
            table.insert(elements, {
                Foreground = {
                    Color = segment.fg
                }
            })
            table.insert(elements, {
                Text = segment.text
            })
        end
        table.insert(elements, {
            Text = '  '
        })

        window:set_right_status(wezterm.format(elements))
    end)
end

return M

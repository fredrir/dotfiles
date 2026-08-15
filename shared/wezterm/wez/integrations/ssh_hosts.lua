local wezterm = require 'wezterm'

local M = {}

local SSH_DIR = wezterm.home_dir .. '/.ssh'

local function read_hosts(path, out, seen)
    local file = io.open(path, 'r')
    if not file then
        return
    end
    for line in file:lines() do
        local names = line:match '^%s*[Hh]ost%s+(.+)$'
        if names then
            for name in names:gmatch '%S+' do
                if not name:match '[*?!]' and not seen[name] then
                    seen[name] = true
                    table.insert(out, name)
                end
            end
        end
    end
    file:close()
end

function M.hosts()
    local out, seen = {}, {}
    read_hosts(SSH_DIR .. '/config', out, seen)
    for _, path in ipairs(wezterm.glob(SSH_DIR .. '/config.d/*')) do
        read_hosts(path, out, seen)
    end
    table.sort(out)
    return out
end

function M.apply(config)
    local menu = config.launch_menu or {}
    for _, host in ipairs(M.hosts()) do
        table.insert(menu, {
            label = 'ssh ' .. host,
            args = {'ssh', host}
        })
    end
    config.launch_menu = menu
end

return M

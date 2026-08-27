local M = {}

function M.apply_to_config(config)
    config.unix_domains = {{
        name = "localmux"
    }}

    config.default_gui_startup_args = {"connect", "localmux"}
end

return M

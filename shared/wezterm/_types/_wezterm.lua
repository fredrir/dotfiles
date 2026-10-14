---@meta

---@class Window
---@field get_input_selection fun(self: Window, pane: Pane): InputSelection?

---@class InputSelection
---@field get_range fun(self: InputSelection, buffer: string, cursor: integer): {start: integer, ['end']: integer}?

---@class Wezterm.Mux
---@field supports_local_pane_layout? boolean
---@field local_pane_layout_domains? string[]

---@class PaneMetadata
---@field remote_pane_id? integer
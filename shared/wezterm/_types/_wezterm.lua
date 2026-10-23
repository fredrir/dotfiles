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

---@class Pane
---@field get_text_from_region fun(self: Pane, start_x: integer, start_y: integer, end_x: integer, end_y: integer): string
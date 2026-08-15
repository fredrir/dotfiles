local platform = require 'wez.platform'

-- Performance knobs that earned their place through source reading or
-- measurement. Everything else deliberately stays at default: front_end
-- (OpenGL is the most-exercised path on both GPUs), max_fps on Wayland
-- (provably unused there — frame pacing comes from the compositor), and the
-- mux parser buffers (128 KiB / 3 ms are the right latency tradeoff).
local M = {}

function M.apply(config)
  -- The default 50/s prefetch limit is tuned for slow WANs; paging remote
  -- scrollback over the ~1.5 ms USB link hits the limiter and shows stale
  -- lines. Client-side, safe on Tailscale too.
  config.ratelimit_mux_line_prefetches_per_second = 1000

  if platform.is_mac then
    -- ProMotion panel; max_fps is only consulted on macOS/X11/Windows.
    config.max_fps = 120
  end

  -- Advertises synchronized output (coherent TUI frames), styled underlines
  -- and truecolor. Needs the wezterm terminfo entry wherever you ssh:
  -- macie ~/.terminfo, archie /usr/share/terminfo via the package; push to
  -- other servers with
  --   tic -xe wezterm -o /tmp/ti termwiz/data/wezterm.terminfo && rsync ...
  config.term = 'wezterm'
end

return M

ConfigWatcher = hs.pathwatcher.new(HOME .. "/dotfiles/macos/hammerspoon/", function(paths)
  for _, path in ipairs(paths) do
    if path:sub(-4) == ".lua" then
      hs.reload()
      return
    end
  end
end)
ConfigWatcher:start()

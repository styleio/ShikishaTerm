-- Runs when the session ends (including a dropped connection or a crash)
-- Example: reconnect on its own

local n = (shikisha.get_var("retry") or 0) + 1
if n > 5 then
  shikisha.notify("slack", tab.name .. " keeps exiting")
  return
end
shikisha.set_var("retry", n)
shikisha.sleep(2000)
shikisha.restart(tab)      -- on_start runs again after the restart

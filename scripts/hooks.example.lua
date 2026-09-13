-- SHIKISHA-TERM example hook script
--
-- A script can be attached at any of three levels, and the more specific one wins:
--   "lua" in a tab's settings     ... that tab only (highest priority)
--   "lua" in a desk               ... shared by that desk
--   "lua" in config.json          ... shared by everything (the fallback)
-- If the tab's script does not define a hook, it falls back to the desk's,
-- then to the shared one (never both).
-- Shared variables (get_var/set_var) are shared by every script in the desk.
-- Hooks: on_start / on_question / on_busy / on_done / on_exit / on_tick
-- API: shikisha.send_to_tab(n, text)  send a prompt (with Enter, chain depth +1)
--      shikisha.send(tab, keys)       send raw keys (as they are)
--      shikisha.wait(tab, pattern, ms) wait until a pattern appears on screen (true/false)
--      shikisha.sleep(ms) / shikisha.notify(dest, text) / shikisha.log(text)
--      shikisha.get_var(k) / shikisha.set_var(k, v)   variables shared between hooks

-- Example 1: start-up automation -- starting the app is enough to pick the work back up
function on_start(tab)
  if tab.index ~= 1 then return end
  -- Wait for the shell prompt, then go to the working folder
  if not shikisha.wait(tab, "\\$ $", 15000) then return end
  shikisha.send(tab, "cd /srv/myproj\r")
  shikisha.wait(tab, "\\$ $", 5000)
  shikisha.send(tab, "claude --resume\r")
  -- If the session picker appears, take the top one
  if shikisha.wait(tab, "[Ss]elect", 8000) then
    shikisha.send(tab, "\r")
  end
end

-- Example 2: auto-approve -- hand only the dangerous-looking questions to a person
function on_question(tab, screen)
  -- A CLI may ask in the language it is set to, so both spellings are matched
  if screen:match("[Dd]elete") or screen:match("削除") or screen:match("rm %-rf") then
    return nil          -- nil = do not answer; leave it to a person (blue WAIT)
  end
  return "1\r"          -- pick option 1
end

-- Example 2b: bring a session back after it ends (an SSH drop, a CLI updating itself, ...)
-- "auto_restart": true in the settings does the same thing.
-- In Lua you also decide how many times to reconnect, and whether to tell someone
function on_exit(tab)
  local key = "restarts_" .. tab.index
  local n = (shikisha.get_var(key) or 0) + 1
  if n > 5 then
    shikisha.notify("slack", tab.name .. " keeps exiting. Please take a look")
    return
  end
  shikisha.set_var(key, n)
  shikisha.log(tab.name .. " exited -> restarting (attempt " .. n .. ")")
  shikisha.sleep(2000)
  shikisha.restart(tab)     -- on_start runs again after the restart
end

-- Example 3: A (build) <-> B (review) loop, with a notice when it is done
function on_done(tab)
  -- tab.chain_depth == 0 is a conversation a person started directly.
  -- Leave here if the pipeline should not react to a person's own instructions
  -- (pairing the middle tabs with "locked": true in the settings is safer still)
  if tab.chain_depth == 0 and tab.index ~= 1 then return end

  local out = tab.output
  if tab.index == 1 then
    local rounds = shikisha.get_var("rounds") or 0
    if out:match("LGTM") or rounds >= 5 then
      shikisha.notify("slack", "Loop finished (" .. rounds .. " rounds)")
      return            -- doing nothing = the loop stops
    end
    shikisha.set_var("rounds", rounds + 1)
    shikisha.send_to_tab(2, "Review this code. If there is nothing to fix, reply with only LGTM:\n" .. out)
  elseif tab.index == 2 then
    if out:match("LGTM") then
      shikisha.notify("slack", "Review passed!")
    else
      shikisha.send_to_tab(1, "Fix what the review found:\n" .. out)
    end
  end
end

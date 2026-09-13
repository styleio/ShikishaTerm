-- The rally starts: tell the AI the goal and the "hand moves over in files" protocol
-- (the AI tab's on_start).
-- This is the directory style, so the contents of this file are the body of on_start(tab, screen).

-- Reset the judge's state
shikisha.set_var("rally_done", false)                    -- true once decided; on_done does nothing after that
shikisha.set_var("rally_round", 0)                       -- moves run
shikisha.set_var("rally_tok", 0)                         -- rough cost (characters exchanged)
shikisha.set_var("rally_t0", shikisha.epoch_ms())        -- start time (milliseconds)

-- Make the folder for this run (outside any synced drive, unique, and old ones are
-- swept at start-up so it does not grow)
local run = shikisha.exchange_new()
shikisha.set_var("rally_run", run)
shikisha.set_var("rally_record", run .. "/record.lua")   -- a record that replays the run when pasted

local br = RALLY.browser
local infile = run .. "/in.lua"
local humanfile = run .. "/human.txt"

-- The AI's first instructions. Its moves are written to a *file*, not the screen
-- (so nothing depends on how the TUI draws)
local prompt = table.concat({
  "You will reach a goal by driving the browser \"" .. br .. "\".",
  "Each turn, hand over your next move by **writing it to a file** (do not paste it on screen).",
  "",
  "[Moves] Always save your next move, as Lua, over this file:",
  "  " .. infile,
  "  Functions you can use in that file (the tab is \"" .. br .. "\"):",
  "    browser_go(\"" .. br .. "\", \"to\"|\"reload\"|\"back\"|\"forward\", url?)",
  "    browser_click(\"" .. br .. "\", sel)      browser_fill(\"" .. br .. "\", sel, value)",
  "    browser_fill_secret(\"" .. br .. "\", sel, secret_name)  -- passwords and the like. Write the name, never the value",
  "    browser_auth(\"" .. br .. "\", secret_name)              -- basic auth (a user:pass secret)",
  "    browser_text(\"" .. br .. "\", sel)       browser_find(\"" .. br .. "\", sel)",
  "    sel is \"#id\" or {xpath=\"...\"}. One file = one move.",
  "  When you have written it, end your turn. This side runs it and sends back the next screen text.",
  "",
  "[People] When a person is needed (sign-in, CAPTCHA, two-step verification), write the request to this file:",
  "  " .. humanfile,
  "  Keep the request short (one or two sentences). No detailed steps; the person can see the browser.",
  "",
  "This side (the judge) decides automatically when it is over. You do not need to declare it done.",
  "If a move does not work, write a different approach to " .. infile .. " on your next turn.",
  "",
  "Goal: " .. RALLY.goal,
  "",
  "Now write your first move to " .. infile .. ".",
}, "\n")

shikisha.send_to_tab(tab.index, prompt)

-- The AI's turn is over. Read the hand-off file, run it, and let the judge decide
-- whether it is finished (the AI tab's on_done).
-- This is the directory style, so the contents of this file are the body of on_done(tab, screen).

-- Do not react to a conversation a person started (no self-loops, no taking over).
-- During a rally the turns go back and forth through send_to_tab, so the chain is 1 or more.
if tab.chain_depth == 0 then return end

-- Already decided: do nothing more (and send no further turns).
-- Without this the AI keeps going after set_result, prompts pile up, and it looks like an endless loop
if shikisha.get_var("rally_done") then return end

local br = RALLY.browser
local ai = tab.index
local run = shikisha.get_var("rally_run")
if not run then return end
local infile = run .. "/in.lua"
local humanfile = run .. "/human.txt"

-- Add up the rough cost (characters exchanged). What the tokens stop condition measures
shikisha.set_var("rally_tok", (shikisha.get_var("rally_tok") or 0) + #(tab.output or ""))

-- 1) A request for a person? -> show the browser, put up the banner, wait until it is pressed
local human = shikisha.exchange_take(humanfile)
if human and #human > 0 then
  shikisha.show(br)
  shikisha.browser_wait(br, { ask = human, label = "Press when done" })
  shikisha.show(ai)
  shikisha.send_to_tab(ai, "The person has finished. Carry on. Write your next move to " .. infile .. ".")
  return
end

-- 2) A move? -> LINT (syntax) -> run in the sandbox -> record
local code = shikisha.exchange_take(infile)
if code and #code > 0 then
  local lint_err = shikisha.lint(code)
  if lint_err then
    shikisha.send_to_tab(ai, table.concat({
      "The Lua you wrote has a syntax error:",
      lint_err,
      "Fix it and write it to " .. infile .. " again.",
    }, "\n"))
    return
  end
  -- Switch to the browser tab at once, so the move is seen happening (no waiting = snappy).
  -- To keep the switch from feeling slow, nothing sleeps before the run
  shikisha.show(br)
  local err = shikisha.run_scoped(br, code)
  if err then
    shikisha.show(ai)
    shikisha.send_to_tab(ai, table.concat({
      "Running it failed:",
      err,
      "Write a different move to " .. infile .. ".",
    }, "\n"))
    return
  end
  -- Record only the moves that worked (kept with secret names, so pasting it replays the run)
  shikisha.exchange_append(shikisha.get_var("rally_record"), code)
  shikisha.set_var("rally_round", (shikisha.get_var("rally_round") or 0) + 1)
  -- Keep the browser in view and poll briefly until the page body appears. Move on
  -- as soon as it does (snappy); wait if it is slow. The short wait first is so the
  -- old page from before the navigation is not picked up
  for _ = 1, 12 do
    shikisha.sleep(150)
    local t = shikisha.browser_text(br, "body")
    if t and #(t:gsub("%s", "")) > 0 then break end
  end
end

-- 3) The judge: check the stop conditions. If one is met, record the exit code and finish
local verdict = RALLY_judge(tab.output)
if verdict then
  shikisha.set_var("rally_done", true)                 -- stops later on_done calls (no more turns)
  shikisha.show(verdict.outcome == "success" and ai or br)
  shikisha.set_result(verdict.code or 0, verdict.reason or "")
  return
end

-- 4) Not over yet -> hand the turn back to the AI with the current screen text, and ask for the next move
shikisha.show(ai)
local text = shikisha.browser_text(br, "body") or ""
if #text > RALLY.screen_chars then
  text = text:sub(1, RALLY.screen_chars) .. "... (truncated)"
end
shikisha.send_to_tab(ai, table.concat({
  "Done. The screen text now:",
  "----",
  text,
  "----",
  "Write your next move to " .. infile .. ". Goal: " .. RALLY.goal,
}, "\n"))

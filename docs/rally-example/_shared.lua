-- The rally's settings and its judge. Rewrite these for your desk.
-- _shared.lua is read before the hooks in the same directory, and shares their namespace.
--
-- Where it goes: point the AI session tab's automation at this directory.
--   e.g. the AI tab in config.json:  { "name": "ai", "command": "claude --model opus",
--         "id": "ai", "automation": "scripts/rally" }
--
-- How it works (the screen is not read; moves are handed over in files)
--   Each turn, the AI *writes* its next move as Lua into in.lua in the exchange
--   folder, then ends its turn. This side reads in.lua (byte for byte), then
--   LINT -> run in the sandbox -> record. Nothing depends on how the TUI draws,
--   so problems like a vanishing code fence or an echoed instruction being
--   mistaken for an answer cannot happen in the first place.
--
-- The judge (this side decides when it is over; the AI is never asked to declare it done)
--   RALLY.stops is checked from the top, and the first condition met wins (first-match-wins).
--   Build on the deterministic conditions (css/xpath/screen/console/rounds/time/tokens),
--   and mix in console (what the AI says) only for a fuzzy goal. Always keep the
--   safety net (rounds/time/tokens).
--
-- Note: each round trip counts as one automatic chain. Raise the global
-- max_chain to at least the rounds limit (at the default of 10 it stops after 10 rounds).

RALLY = {
  -- The id of the browser tab to drive (the "id" of the browser tab in config.json)
  browser = "br",

  -- The goal. Passed to the AI as it is
  goal = "(Write the goal here. e.g. sign in to the diary service and post an entry)",

  -- The most characters of screen text sent back to the AI after each move
  screen_chars = 3000,

  -- Stop conditions (the judge). Checked from the top; the first one met wins.
  --   when="css"/"xpath" ... an element is visible          sel=selector
  --   when="screen"       ... text in the browser page body  pattern=... (one line each for several)
  --   when="console"      ... text in what the AI said       pattern=...
  --   when="rounds"       ... moves run                      max=N
  --   when="time"         ... seconds elapsed                sec=N
  --   when="tokens"       ... rough cost (characters exchanged) max=N
  -- outcome="success"/"fail", code=exit code, reason=why
  -- The same measure can appear more than once with different thresholds
  -- (e.g. success at rounds=10, a safety-net failure at rounds=50).
  stops = {
    -- Examples of success (write them for your goal):
    -- { when="css",    sel="#editor",              outcome="success", code=0, reason="editor is showing" },
    -- { when="screen", pattern="Posted",           outcome="success", code=0, reason="post published" },
    -- Examples of failure:
    -- { when="screen", pattern="Error",            outcome="fail",    code=1, reason="an error is showing" },
    -- { when="css",    sel=".g-recaptcha",         outcome="fail",    code=3, reason="CAPTCHA" },

    -- The safety net (against a runaway. Always keep it; remove it knowingly and at your own risk):
    { when="rounds", max=20,     outcome="fail", code=124, reason="reached the round limit" },
    { when="time",   sec=600,    outcome="fail", code=124, reason="reached the time limit" },
    { when="tokens", max=300000, outcome="fail", code=125, reason="reached the (estimated) cost limit" },
  },
}

-- The judge itself. Checks the stop conditions and returns the one met (a table), or nil.
-- screen_out is what the AI said this turn (tab.output). Used by console conditions.
function RALLY_judge(screen_out)
  local br = RALLY.browser
  for _, s in ipairs(RALLY.stops or {}) do
    local hit = false
    if s.when == "css" or s.when == "xpath" then
      local sel = (s.when == "xpath") and { xpath = s.sel } or s.sel
      -- A missing element or a page not yet loaded does not stop it. True only when visible
      local ok, state = pcall(shikisha.browser_find, br, sel)
      hit = ok and state == "visible"
    elseif s.when == "screen" then
      local ok, body = pcall(shikisha.browser_text, br, "body")
      hit = ok and body and body:find(s.pattern, 1, true) ~= nil
    elseif s.when == "console" then
      hit = (screen_out or ""):find(s.pattern, 1, true) ~= nil
    elseif s.when == "rounds" then
      hit = (shikisha.get_var("rally_round") or 0) >= (s.max or 0)
    elseif s.when == "time" then
      local t0 = shikisha.get_var("rally_t0") or shikisha.epoch_ms()
      hit = (shikisha.epoch_ms() - t0) >= (s.sec or 0) * 1000
    elseif s.when == "tokens" then
      hit = (shikisha.get_var("rally_tok") or 0) >= (s.max or 0)
    end
    if hit then return s end
  end
  return nil
end

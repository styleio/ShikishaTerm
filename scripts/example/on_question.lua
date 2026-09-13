-- Runs when the tab is asked to confirm or choose
-- Available: tab, screen (the whole screen as text)
-- Return a string to send it automatically; return nothing to leave it to a person

-- A CLI may ask in the language it is set to, so both spellings are matched
if screen:match("[Dd]elete") or screen:match("削除") or screen:match("rm %-rf") then
  return nil          -- leave dangerous-looking questions to a person
end
return "1\r"          -- pick option 1

-- Runs right after the tab starts
-- Example: pick up where the last session left off
-- How to write these: docs/AUTOMATION.md

if not shikisha.wait(tab, "%$ $", 15000) then return end
shikisha.send(tab, "cd /srv/myproj\r")
shikisha.wait(tab, "%$ $", 5000)

-- --continue picks the last conversation straight back up (nothing to choose)
shikisha.send(tab, "claude --continue\r")

-- To choose an older conversation instead, use --resume and pick from the list:
-- shikisha.send(tab, "claude --resume\r")
-- if shikisha.wait(tab, "[Ss]elect", 8000) then shikisha.send(tab, "\r") end

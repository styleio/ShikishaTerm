-- Runs when a reply has finished
-- Available: tab (tab.output / tab.name / tab.index / tab.chain_depth)

-- Do nothing when a person gave the instruction directly (only react to hand-offs)
if tab.chain_depth == 0 then return end

local rounds = shikisha.get_var("rounds") or 0
if tab.output:match("LGTM") or rounds >= 5 then
  shikisha.notify("slack", "Review finished (" .. rounds .. " rounds)")
  return
end
shikisha.set_var("rounds", rounds + 1)
-- A tab is named by its automation name (shown on the tab settings screen; it
-- survives reordering the tabs and renaming them on screen)
shikisha.send_to_tab("coder", "Fix what the review found:\n" .. tab.output)

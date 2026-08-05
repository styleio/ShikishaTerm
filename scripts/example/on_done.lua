-- 応答が完了したときに実行されます
-- 使える変数: tab (tab.output / tab.name / tab.index / tab.chain_depth)

-- 人間が直接指示したときは反応しない (自動転送のときだけ動く)
if tab.chain_depth == 0 then return end

local rounds = shikisha.get_var("rounds") or 0
if tab.output:match("LGTM") or rounds >= 5 then
  shikisha.notify("slack", "レビュー完了 (" .. rounds .. "往復)")
  return
end
shikisha.set_var("rounds", rounds + 1)
-- タブは名前で指定できる (並べ替えても壊れない)
shikisha.send_to_tab("実装", "指摘を修正して:\n" .. tab.output)

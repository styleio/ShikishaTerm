-- セッションが終了したときに実行されます (切断・クラッシュを含む)
-- 例: 自動で再接続する

local n = (shikisha.get_var("retry") or 0) + 1
if n > 5 then
  shikisha.notify("slack", tab.name .. " が繰り返し終了しています")
  return
end
shikisha.set_var("retry", n)
shikisha.sleep(2000)
shikisha.restart(tab)      -- 再起動後は on_start がもう一度動く

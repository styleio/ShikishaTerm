-- タブが起動した直後に実行されます
-- 例: 前回の作業を自動で再開する
-- 書き方: docs/AUTOMATION.md

if not shikisha.wait(tab, "%$ $", 15000) then return end
shikisha.send(tab, "cd /srv/myproj\r")
shikisha.wait(tab, "%$ $", 5000)
shikisha.send(tab, "claude --resume\r")
if shikisha.wait(tab, "[Ss]elect", 8000) then
  shikisha.send(tab, "\r")     -- 一番上のセッションを選ぶ
end

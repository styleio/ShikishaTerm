-- タブが起動した直後に実行されます
-- 例: 前回の作業を自動で再開する
-- 書き方: docs/AUTOMATION.md

if not shikisha.wait(tab, "%$ $", 15000) then return end
shikisha.send(tab, "cd /srv/myproj\r")
shikisha.wait(tab, "%$ $", 5000)

-- --continue は直前の会話をそのまま再開する (選択操作が要らない)
shikisha.send(tab, "claude --continue\r")

-- 過去の会話を選んで再開したいときは --resume を使い、一覧から選ぶ:
-- shikisha.send(tab, "claude --resume\r")
-- if shikisha.wait(tab, "[Ss]elect", 8000) then shikisha.send(tab, "\r") end

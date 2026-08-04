-- ShikishaTerm-AI フックスクリプト例
-- config.json に "lua": "scripts/hooks.lua" を書くと読み込まれる。
-- 使えるフック: on_start / on_question / on_busy / on_done / on_exit / on_tick
-- API: shikisha.send_to_tab(n, text)  プロンプト送信 (Enter付き、チェーン深度+1)
--      shikisha.send(tab, keys)       生のキー送信 (そのまま)
--      shikisha.wait(tab, pattern, ms) 画面に正規表現が出るまで待つ (true/false)
--      shikisha.sleep(ms) / shikisha.notify(dest, text) / shikisha.log(text)
--      shikisha.get_var(k) / shikisha.set_var(k, v)   フック間共有変数

-- 例1: スタートアップ自動化 — 起動するだけで作業を復旧する
function on_start(tab)
  if tab.index ~= 1 then return end
  -- シェルプロンプトを待って作業フォルダへ
  if not shikisha.wait(tab, "\\$ $", 15000) then return end
  shikisha.send(tab, "cd /srv/myproj\r")
  shikisha.wait(tab, "\\$ $", 5000)
  shikisha.send(tab, "claude --resume\r")
  -- セッション選択画面が出たら一番上を選ぶ
  if shikisha.wait(tab, "[Ss]elect", 8000) then
    shikisha.send(tab, "\r")
  end
end

-- 例2: 自動承認 — 危険そうな確認だけ人間に回す
function on_question(tab, screen)
  if screen:match("削除") or screen:match("rm %-rf") then
    return nil          -- nil = 自動応答せず人間へ (青WAIT)
  end
  return "1\r"          -- 選択肢1を選ぶ
end

-- 例3: A(実装) ⇔ B(レビュー) 自動ループ + 完了通知
function on_done(tab)
  local out = tab.output
  if tab.index == 1 then
    local rounds = shikisha.get_var("rounds") or 0
    if out:match("LGTM") or rounds >= 5 then
      shikisha.notify("slack", "自動ループ完了 (" .. rounds .. "往復)")
      return            -- 何もしない = ループ停止
    end
    shikisha.set_var("rounds", rounds + 1)
    shikisha.send_to_tab(2, "このコードをレビューして。問題なければLGTMとだけ返して:\n" .. out)
  elseif tab.index == 2 then
    if out:match("LGTM") then
      shikisha.notify("slack", "レビュー通過!")
    else
      shikisha.send_to_tab(1, "レビュー指摘を修正して:\n" .. out)
    end
  end
end

-- 確認・選択肢を聞かれたときに実行されます
-- 使える変数: tab, screen (画面テキスト全体)
-- 文字列を返すと自動送信、返さなければ人間の判断待ちになります

if screen:match("削除") or screen:match("rm %-rf") then
  return nil          -- 危険そうな確認は人間に任せる
end
return "1\r"          -- 選択肢1を選ぶ

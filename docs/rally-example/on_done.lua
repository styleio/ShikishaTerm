-- AIの手番が終わった。出力を解析して1手進める (AIセッションタブの on_done)。
-- ディレクトリ方式なので、このファイルの中身がそのまま on_done(tab, screen) の本体になる。

-- 人間が始めた会話には反応しない(自己ループ・乗っ取り防止)。
-- ラリー中は send_to_tab で往復するのでチェーンは1以上になる。
if tab.chain_depth == 0 then return end

local out = tab.output or ""
local br = RALLY.browser
local ai = tab.index

-- 1) 完了マーカー？ → 終了コードを記録して終わる
local done = out:match("```shikisha%-done(.-)```")
if done then
  local code = tonumber(done:match("code%s*=%s*(%-?%d+)")) or 0
  local reason = done:match("reason%s*=%s*(.-)%s*$")
    or done:match("reason%s*=%s*(.-)\n") or ""
  shikisha.show(ai)
  shikisha.set_result(code, reason)
  return
end

-- 2) 人間依頼？ → ブラウザを見せて帯を出し、押されるまで待つ
local human = out:match("«HUMAN»%s*([^\n]+)")
if human then
  shikisha.show(br)
  shikisha.browser_wait(br, { ask = human, label = "できたら押す" })
  shikisha.show(ai)
  shikisha.send_to_tab(ai, "人間が対応を終えました。続けてください。")
  return
end

-- 3) Luaブロック？ → サンドボックスで実行
local code = out:match("```shikisha%-lua%s*(.-)```")
if not code then
  shikisha.send_to_tab(ai,
    "書式が読めません。```shikisha-lua のLuaブロックか ```shikisha-done の完了マーカーを、1つだけ出してください。")
  return
end

-- 手数の上限(暴走ガード)。max_chain とは別に、ここでも数える
local step = (shikisha.get_var("rally_step") or 0) + 1
shikisha.set_var("rally_step", step)
if step > RALLY.max_steps then
  shikisha.set_result(2, "手数の上限(" .. RALLY.max_steps .. ")に達しました")
  return
end

-- 実行を観戦できるように: ブラウザへ切替 → 少し待つ → サンドボックス実行 → 記録
shikisha.show(br)
shikisha.sleep(400)                 -- 画面が切り替わってから動かす(人が目で追える)
local err = shikisha.run_scoped(br, code)
shikisha.record(code)               -- 再生用に、実行したLuaを鍵名のまま積む
shikisha.sleep(600)                 -- 結果を目視できるよう少し待つ

-- 状態を集めてAIへ返す(次の手番へ)
shikisha.show(ai)
if err then
  shikisha.send_to_tab(ai, "実行でエラーになりました:\n" .. err .. "\n別の手を出してください。")
  return
end
local text = shikisha.browser_text(br, "body") or ""
if #text > RALLY.screen_chars then
  text = text:sub(1, RALLY.screen_chars) .. "…(以下略)"
end
shikisha.send_to_tab(ai, table.concat({
  "実行しました。今の画面テキスト:",
  "----",
  text,
  "----",
  "次の1手を出してください。達成/失敗が判断できたら ```shikisha-done を。",
}, "\n"))

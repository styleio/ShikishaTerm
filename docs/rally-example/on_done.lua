-- AIの手番が終わった。受け渡しファイルを読み、実行し、審判で終了を判定する (AIタブの on_done)。
-- ディレクトリ方式なので、このファイルの中身がそのまま on_done(tab, screen) の本体になる。

-- 人間が始めた会話には反応しない(自己ループ・乗っ取り防止)。
-- ラリー中は send_to_tab で往復するのでチェーンは1以上になる。
if tab.chain_depth == 0 then return end

-- 決着済みなら、もう何もしない(手番も送らない)。
-- これが無いと set_result 後もAIが動き続け、プロンプトが溜まって「無限ループ」に見える
if shikisha.get_var("rally_done") then return end

local br = RALLY.browser
local ai = tab.index
local run = shikisha.get_var("rally_run")
if not run then return end
local infile = run .. "/in.lua"
local humanfile = run .. "/human.txt"

-- 概算コスト(やり取り文字数)を積む。tokens 停止条件の材料
shikisha.set_var("rally_tok", (shikisha.get_var("rally_tok") or 0) + #(tab.output or ""))

-- 1) 人間依頼ファイル？ → ブラウザを見せて帯を出し、押されるまで待つ
local human = shikisha.exchange_take(humanfile)
if human and #human > 0 then
  shikisha.show(br)
  shikisha.browser_wait(br, { ask = human, label = "できたら押す" })
  shikisha.show(ai)
  shikisha.send_to_tab(ai, "人間が対応を終えました。続けてください。次の1手を " .. infile .. " に書いてください。")
  return
end

-- 2) 操作ファイル？ → LINT(構文) → サンドボックス実行 → 記録
local code = shikisha.exchange_take(infile)
if code and #code > 0 then
  local lint_err = shikisha.lint(code)
  if lint_err then
    shikisha.send_to_tab(ai, table.concat({
      "書いてくれたLuaが構文エラーでした:",
      lint_err,
      "直して " .. infile .. " に書き直してください。",
    }, "\n"))
    return
  end
  -- 実行 (ブラウザは隠れたまま処理する。人へ逐一見せる必要はない。
  -- 見えているAIタブの「処理中」表示だけで、進んでいることは伝わる)。
  local err = shikisha.run_scoped(br, code)
  if err then
    shikisha.send_to_tab(ai, table.concat({
      "実行でエラーになりました:",
      err,
      "別の手を " .. infile .. " に書いてください。",
    }, "\n"))
    return
  end
  -- 成功した手だけ記録する(貼れば再現できるよう、鍵名のまま積む)
  shikisha.exchange_append(shikisha.get_var("rally_record"), code)
  shikisha.set_var("rally_round", (shikisha.get_var("rally_round") or 0) + 1)
  -- 遷移が終わって本文が出るまで短く待つ。出たら即進む(きびきび)、遅ければ待つ。
  -- 先に少し待つのは、遷移前の古い画面を掴まないため
  for _ = 1, 12 do
    shikisha.sleep(150)
    local t = shikisha.browser_text(br, "body")
    if t and #(t:gsub("%s", "")) > 0 then break end
  end
end

-- 3) 審判: 停止条件を評価。成立したら終了コードを記録して終わる
local verdict = RALLY_judge(tab.output)
if verdict then
  shikisha.set_var("rally_done", true)                 -- 以後の on_done を止める(手番を送らない)
  shikisha.show(verdict.outcome == "success" and ai or br)
  shikisha.set_result(verdict.code or 0, verdict.reason or "")
  return
end

-- 4) まだ終わらない → 今の画面テキストを返して次の手番へ (AIタブは出したまま)
local text = shikisha.browser_text(br, "body") or ""
if #text > RALLY.screen_chars then
  text = text:sub(1, RALLY.screen_chars) .. "…(以下略)"
end
shikisha.send_to_tab(ai, table.concat({
  "実行しました。今の画面テキスト:",
  "----",
  text,
  "----",
  "次の1手を " .. infile .. " に書いてください。目的: " .. RALLY.goal,
}, "\n"))

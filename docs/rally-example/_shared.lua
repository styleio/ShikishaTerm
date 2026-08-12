-- ラリーの設定と審判。このワークスペース用に書き換える。
-- _shared.lua は同じディレクトリのフックより先に読まれ、名前空間を共有する。
--
-- 置き場所: このディレクトリを、AIセッションタブの automation に指す。
--   例) config.json のAIタブ:  { "name": "ai", "command": "claude --model opus",
--         "id": "ai", "automation": "scripts/rally" }
--
-- ■ 仕組み (画面を読まない・ファイルで受け渡す)
--   AIは毎手番、次の1手のLuaを exchange の in.lua に *書き* 、手番を終える。
--   こちらは in.lua を読んで(バイト正確)、LINT→サンドボックス実行→記録 する。
--   TUIの描画に依存しないので、コードフェンスが消える/指示エコーを誤検知する
--   といった不具合が原理的に起きない。
--
-- ■ 審判 (終了はこちらが決める。AIに完了宣言はさせない)
--   RALLY.stops を上から評価し、最初に成立した条件が勝つ(first-match-wins)。
--   決定的条件(css/xpath/screen/console/rounds/time/tokens)を土台にし、
--   曖昧なゴールのときだけ console(AIの発言) を混ぜる。安全網(rounds/time/tokens)は必ず入れる。
--
-- 注意: 往復は「自動チェーン」を1回ずつ数える。全体設定 max_chain を
-- rounds上限以上に上げておくこと(既定10だと10往復で止まる)。

RALLY = {
  -- 操作するブラウザタブの id (config.json のブラウザタブの "id")
  browser = "br",

  -- 目的。AIにそのまま渡る
  goal = "（ここに目的を書く。例: 日記SaaSにログインして本文を投稿する）",

  -- 1手ごとにAIへ返す画面テキストの最大文字数
  screen_chars = 3000,

  -- 停止条件(審判)。上から順に評価し、最初に成立したものが勝つ。
  --   when="css"/"xpath" … 要素が見える     sel=セレクタ
  --   when="screen"       … ブラウザ本文に文字列  pattern=... (複数なら別行で並べる)
  --   when="console"      … AIの発言に文字列     pattern=...
  --   when="rounds"       … 実行回数            max=N
  --   when="time"         … 経過秒              sec=N
  --   when="tokens"       … 概算コスト(やり取り文字数) max=N
  -- outcome="success"/"fail", code=終了コード, reason=理由
  -- 同じ指標に、しきい値違いで複数置ける(例: rounds=10で成功, rounds=50で保険失敗)。
  stops = {
    -- 達成の例(目的に合わせて書く):
    -- { when="css",    sel="#editor",              outcome="success", code=0, reason="エディタ表示" },
    -- { when="screen", pattern="投稿しました",       outcome="success", code=0, reason="投稿完了" },
    -- 失敗の例:
    -- { when="screen", pattern="エラー",            outcome="fail",    code=1, reason="エラー表示" },
    -- { when="css",    sel=".g-recaptcha",         outcome="fail",    code=3, reason="CAPTCHA" },

    -- 安全網(暴走保険。必ず入れる。危険承知で外すなら自己責任):
    { when="rounds", max=20,     outcome="fail", code=124, reason="往復上限に到達" },
    { when="time",   sec=600,    outcome="fail", code=124, reason="時間上限に到達" },
    { when="tokens", max=300000, outcome="fail", code=125, reason="コスト上限(概算)に到達" },
  },
}

-- 審判本体。停止条件を評価し、成立した条件(テーブル)を返す。無ければ nil。
-- screen_out はこの手番のAIの発言(tab.output)。console 条件で使う。
function RALLY_judge(screen_out)
  local br = RALLY.browser
  for _, s in ipairs(RALLY.stops or {}) do
    local hit = false
    if s.when == "css" or s.when == "xpath" then
      local sel = (s.when == "xpath") and { xpath = s.sel } or s.sel
      -- 要素が無い/ページ未読込でも止めない。見えたときだけ true
      local ok, state = pcall(shikisha.browser_find, br, sel)
      hit = ok and state == "visible"
    elseif s.when == "screen" then
      local ok, body = pcall(shikisha.browser_text, br, "body")
      hit = ok and body and body:find(s.pattern, 1, true) ~= nil
    elseif s.when == "console" then
      hit = (screen_out or ""):find(s.pattern, 1, true) ~= nil
    elseif s.when == "rounds" then
      hit = (shikisha.get_var("rally_round") or 0) >= (s.max or 0)
    elseif s.when == "time" then
      local t0 = shikisha.get_var("rally_t0") or shikisha.epoch_ms()
      hit = (shikisha.epoch_ms() - t0) >= (s.sec or 0) * 1000
    elseif s.when == "tokens" then
      hit = (shikisha.get_var("rally_tok") or 0) >= (s.max or 0)
    end
    if hit then return s end
  end
  return nil
end

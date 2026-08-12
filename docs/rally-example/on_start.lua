-- ラリー開始: AIに目的と「ファイルで受け渡す」プロトコルを伝える (AIタブの on_start)。
-- ディレクトリ方式なので、このファイルの中身がそのまま on_start(tab, screen) の本体になる。

-- 審判用の状態を初期化
shikisha.set_var("rally_round", 0)                       -- 実行した手数
shikisha.set_var("rally_tok", 0)                         -- 概算コスト(やり取り文字数)
shikisha.set_var("rally_t0", shikisha.epoch_ms())        -- 開始時刻(ミリ秒)

-- run 用フォルダを作る (Drive外・ユニーク・肥大化しないよう起動時に古いものは掃除される)
local run = shikisha.exchange_new()
shikisha.set_var("rally_run", run)
shikisha.set_var("rally_record", run .. "/record.lua")   -- 貼れば再現できる記録

local br = RALLY.browser
local infile = run .. "/in.lua"
local humanfile = run .. "/human.txt"

-- AIへの最初の指示。出力は画面ではなく *ファイル* に書かせる (TUI描画に依存しない)
local prompt = table.concat({
  "あなたはブラウザ \"" .. br .. "\" を操作して目的を達成します。",
  "毎手番、次の1手を **ファイルに書いて** 渡してください(画面に貼らない)。",
  "",
  "【操作】次の1手のLuaを、必ずこのファイルに上書き保存する:",
  "  " .. infile,
  "  ファイルの中で使える関数(対象タブ名は \"" .. br .. "\"):",
  "    browser_go(\"" .. br .. "\", \"to\"|\"reload\"|\"back\"|\"forward\", url?)",
  "    browser_click(\"" .. br .. "\", sel)      browser_fill(\"" .. br .. "\", sel, value)",
  "    browser_fill_secret(\"" .. br .. "\", sel, 秘密名)  -- パスワード等。値は書かず鍵名で",
  "    browser_auth(\"" .. br .. "\", 秘密名)              -- ベーシック認証(user:pass の秘密)",
  "    browser_text(\"" .. br .. "\", sel)       browser_find(\"" .. br .. "\", sel)",
  "    sel は \"#id\" か {xpath=\"...\"}。1ファイル=1手。",
  "  書いたら手番を終える。こちらが実行し、次の画面テキストを返します。",
  "",
  "【人間】ログイン/CAPTCHA/2段階認証など人手が要るときは、依頼文をこのファイルに書く:",
  "  " .. humanfile,
  "",
  "終了の判定はこちら(審判)が自動で行います。あなたは完了宣言をしなくてよい。",
  "うまくいかない手は、次の手番で別の方法を " .. infile .. " に書いてください。",
  "",
  "目的: " .. RALLY.goal,
  "",
  "では最初の1手を " .. infile .. " に書いてください。",
}, "\n")

shikisha.send_to_tab(tab.index, prompt)

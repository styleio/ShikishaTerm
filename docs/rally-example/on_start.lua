-- ラリー開始: AIに目的とプロトコルを伝える (AIセッションタブの on_start)。
-- ディレクトリ方式なので、このファイルの中身がそのまま on_start(tab, screen) の本体になる。

shikisha.record_reset()
shikisha.set_var("rally_step", 0)

local br = RALLY.browser

-- AIへの最初の指示。手番ごとの出力の書式(プロトコル)を厳密に伝える
local prompt = table.concat({
  "あなたはブラウザを操作して目的を達成します。手番ごとに、次のどれか1つだけを出力してください。",
  "",
  "【操作】ブラウザ操作の Lua を  ```shikisha-lua  と  ```  で囲んで出す(複数行可)。",
  "  対象タブ名は \"" .. br .. "\" です。使える関数:",
  "    browser_go(\"" .. br .. "\", \"to\"|\"reload\"|\"back\"|\"forward\", url?)  -- その場移動",
  "    browser_click(\"" .. br .. "\", sel)     browser_fill(\"" .. br .. "\", sel, value)",
  "    browser_fill_secret(\"" .. br .. "\", sel, 秘密名)  -- パスワード等。値は書かず鍵名で",
  "    browser_auth(\"" .. br .. "\", 秘密名)             -- ベーシック認証(user:pass の秘密)",
  "    browser_text(\"" .. br .. "\", sel)  browser_html(\"" .. br .. "\")",
  "    browser_find(\"" .. br .. "\", sel) -> \"visible\"/\"not_found\"",
  "    browser_fetch(\"" .. br .. "\", url, {method=,headers=,body=}) -> {status,ok,body,...}",
  "    sel は \"#id\" か {xpath=\"...\"}。",
  "",
  "【人間】ログイン/CAPTCHA等で人手が要るとき:  «HUMAN» 人間への依頼文",
  "",
  "【完了】達成/失敗を判定したら  ```shikisha-done  と  ```  で囲んで:",
  "    code = 0",
  "    reason = 判定の理由",
  "",
  "実行後は毎回、今の画面テキストを返します。それを見て次の1手を決めてください。",
  "",
  "目的: " .. RALLY.goal,
  "",
  "まず最初の手を1つだけ出してください。",
}, "\n")

shikisha.send_to_tab(tab.index, prompt)

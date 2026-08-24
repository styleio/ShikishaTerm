# 自動化リファレンス

タブの状態が変わったときに、自動でなにかを実行する仕組みです。
書き方はLuaという小さなスクリプト言語ですが、必要なのは数行だけです。

このドキュメントは**人間向けの説明**であると同時に、
設定画面の「AIに書いてもらう」機能がAIへ渡す**仕様書**でもあります。

翻訳は `docs/AUTOMATION.<コード>.md` として同じ場所に置きます。言語設定に応じて自動で選ばれます。

---

## 1. いつ実行されるか（イベント）

自動化フォルダの中に、イベント名のファイルを置くと、その瞬間に実行されます。
必要なものだけ置けば構いません。

| ファイル名 | いつ動くか |
|---|---|
| `on_start.lua` | タブが起動して落ち着いたとき (下記) |
| `on_done.lua` | 聞いたことへのAIの応答が完了したとき |
| `on_question.lua` | AIが確認・選択肢を出してきたとき |
| `on_exit.lua` | セッションが終了したとき（切断・クラッシュを含む） |
| `on_busy.lua` | 応答が始まったとき（上級者向け） |
| `_shared.lua` | 上記より先に読まれる。共通の下請け関数を置く場所 |

`on_done.lua` と `on_busy.lua` は、**そのタブに何か送ったあとだけ**動きます。
どんなプログラムも起動時に何か出力するので画面は「動いて→止まる」となり、
これは応答と同じ形をしています。この条件が無いと、**起動時のバナーが応答として
他のタブへ転送されてしまいます**。

`on_start.lua` はタブが出た瞬間には動きません。AI CLIは自分の入力欄を描き終わるまで
入力を受け取らないため、**出力が出て画面が落ち着いてから**実行されます (たいてい1〜2秒)。
自分で待つ必要はありません。

ファイルの中身は**処理の本体だけ**を書きます。`function ... end` は不要です。

```lua
-- on_done.lua の例
shikisha.send_to_tab(2, "このコードをレビューして:\n" .. tab.output)
```

---

## 2. 使える変数

どのイベントでも `tab` が使えます。

| 変数 | 内容 |
|---|---|
| `tab.index` | タブ番号（1から） |
| `tab.name` | タブ名 |
| `tab.id` | 設定で付けた「自動化での呼び名」。付いていなければ `nil`。名前を変えても壊れない唯一の手がかりなので、条件分岐はこれで書きます |
| `tab.output` | **直前の応答テキスト**（過去の履歴は含まれない） |
| `tab.state` | `"BUSY"` / `"DONE"` / `"QUESTION"` / `"WAIT"` / `"EXIT"` |
| `tab.profile` | 適用中のプロファイル名 |
| `tab.chain_depth` | 自動転送が何回連鎖したか。**0なら人間が始めた会話** |
| `tab.locked` | 入力ロック中かどうか |
| `tab.is_model` | CLIではなく、APIでモデルと話すタブかどうか |
| `tab.reply` | モデルタブの返答そのもの（そのタブでのみ）。`tab.output` は同じ内容が画面に描かれたもの（折り返し済み） |

`on_question.lua` だけ、2つめの変数 `screen` に画面テキスト全体が入ります。

---

## 3. 使える命令

### タブの指し方

番号は**並べ替えると変わる**ので、名前で指すのが基本です。

```lua
shikisha.send_to_tab("検査", "レビューして")   -- 推奨
shikisha.send_to_tab(2, "レビューして")        -- 番号でも可（並べ替えで変わる）
```

タブ名を変える予定がある場合や、**同じ名前のタブが複数ある**場合は、
設定の「自動化での呼び名」（`id`）を付けてください。IDを付けると、
タブ名を自由に変えても自動化は壊れません。

```jsonc
{ "name": "検査", "id": "reviewer", "command": "codex" }
```

```lua
shikisha.send_to_tab("reviewer", "レビューして")   -- 名前を変えても有効
```

| 命令 | 説明 |
|---|---|
| `shikisha.send_to_tab(タブ, "文字列")` | **タブに指示を渡して実行させる。** 自分自身にも使えます（自動チェーン+1） |
| `shikisha.send(tab, "文字列")` | 生のキー入力を送る（改行は `\r`）。指示ではなく、確認への返答用 |
| `shikisha.wait(tab, "正規表現", ミリ秒)` | 画面にその文字が出るまで待つ。出たら `true` |
| `shikisha.sleep(ミリ秒)` | 待つ（待っている間も他のタブは動きます） |
| `shikisha.state(tab)` | **今の**状態を読む（ループの終了条件に使う） |
| `shikisha.wait_state(tab, "DONE", ミリ秒)` | その状態になるまで待つ |
| `shikisha.notify("宛先", "文字列")` | Slack / Telegram へ通知（設定済みの宛先のみ） |
| `shikisha.restart(tab)` | そのタブを再起動する |
| `shikisha.log("文字列")` | `logs/hooks.log` に記録 |
| `shikisha.get_var("キー")` / `shikisha.set_var("キー", 値)` | 記憶しておける変数。ワークスペース内で共有 |

`on_question.lua` で**文字列を返す**と、それが自動的に送信されます。
`nil` を返す（または何も返さない）と、人間の判断待ちになります。

### AIに指示を出すときは `send_to_tab`

AI CLIは「貼り付けられた指示」と「それを実行する改行」を別の出来事として扱い、
**貼り付けを取り込む前に届いた改行は捨てます**。`send_to_tab` はそこを引き受けます。

```lua
-- 正しい。1回の呼び出しで、入力されて実行される
shikisha.send_to_tab(tab, "あなたはビアンカ派閥として相手を論破してください")

-- 誤り。入力欄に文章が入ったまま止まる
shikisha.send(tab, "あなたはビアンカ派閥として相手を論破してください")
shikisha.send(tab, "\r")
```

**`sleep` で誤魔化さないでください。** 固定の待ち時間は「相手が何秒で準備できるか」の
推測でしかなく、機種・モデル・プロンプトの長さで変わります。**いつか必ず破綻します。**
`send_to_tab` は時計ではなく実際の出来事を待ちます。

`send` は、AIが既に待っているキー入力を送るときに使ってください
（確認に `"1\r"` で答える、シェルを操作する、など）。

---

## 4. よくある例

### 起動時に最初の指示を渡す（on_start.lua）

```lua
shikisha.send_to_tab(tab, "このプロジェクトの昨日の変更点をまとめて")
```

これだけです。フックは**プログラムが入力を受け取れる状態になるまで待ってから**動きます。

### 起動しただけで前日の作業を再開する（on_start.lua）

```lua
if not shikisha.wait(tab, "%$ $", 15000) then return end
shikisha.send(tab, "cd /srv/myproj\r")
shikisha.wait(tab, "%$ $", 5000)
shikisha.send(tab, "claude --continue\r")   -- 直前の会話をそのまま再開する
```

過去の会話を選んで再開したい場合は `claude --resume` を使います。
一覧が出るので、そこから選ぶ操作も自動化できます:

```lua
shikisha.send(tab, "claude --resume\r")
if shikisha.wait(tab, "[Ss]elect", 8000) then
  shikisha.send(tab, "\r")     -- 一番上のセッションを選ぶ
end
```

### 危険な確認だけ人間に回して、あとは自動で承認（on_question.lua）

```lua
if screen:match("削除") or screen:match("rm %-rf") then
  return nil          -- 人間に任せる
end
return "1\r"          -- 選択肢1を選ぶ
```

### AとBでレビューを往復させ、5回で打ち切る（on_done.lua）

```lua
-- 人間が直接指示したときは反応しない
if tab.chain_depth == 0 then return end

local rounds = shikisha.get_var("rounds") or 0
if tab.output:match("LGTM") or rounds >= 5 then
  shikisha.notify("slack", "レビュー完了（" .. rounds .. "往復）")
  return                                   -- 何もしない = ループ終了
end
shikisha.set_var("rounds", rounds + 1)
shikisha.send_to_tab(1, "指摘を修正して:\n" .. tab.output)
```

### 切断されたら自動で再接続する（on_exit.lua）

```lua
local n = (shikisha.get_var("retry") or 0) + 1
if n > 5 then
  shikisha.notify("slack", tab.name .. " が繰り返し落ちています")
  return
end
shikisha.set_var("retry", n)
shikisha.sleep(2000)
shikisha.restart(tab)      -- 再起動後は on_start がもう一度動く
```

### 定期的に様子を見る（on_busy.lua）

`sleep` で待っている間も画面や他のタブは止まりません。間隔は自分で決められます。

```lua
-- 処理中の間だけ、30秒おきに記録する
while shikisha.state(tab) == "BUSY" do
  shikisha.sleep(30000)
  shikisha.log(tab.name .. " はまだ処理中")
end
```

`tab.state` は**呼ばれた瞬間の**状態なので、ループの条件には
`shikisha.state(tab)`（今の状態）を使ってください。
タブが終了・再起動すると、待機中のループは自動で破棄されます。

### 完了したらSlackに通知するだけ（on_done.lua）

```lua
shikisha.notify("slack", tab.name .. " が完了しました:\n" .. tab.output)
```

---

### 何が渡されるか

渡るのは応答だけで、まわりの飾りは落とします。起動バナー、入力欄の枠、
CLI が下端に出し続けるヒント行やステータス行 (`? for shortcuts`、
モデル名と作業フォルダの表示) は含まれません。

見分けるのは**位置と変化で、文字列の一致では判定しません**。カーソルより下は
何が書かれていようと入力欄です。それ以外は、実行した瞬間に撮った画面と見比べ、
**応答が存在する前から画面にあったもの**を落とします。CLI が文言を変えても翻訳しても
効きますし、応答にどんな文章が入っていても消される心配がありません。

指示そのものも返しません。応答が始まるのは**実行した行の次の行**からです。
指示が長くて折り返していると、以前はその後半だけが答えの先頭に付いてきて、
相手が言ったことのように見えていました。

ひとつだけ手が届かないことがあります。**応答の最中に画面の幅を狭めると、応答が欠けます。**
端末が保存している各行を新しい幅で切り捨てるためで、切られた文字は戻せません
(広げる・高さを変えるのは無害です)。狭めたことは `logs/hooks.log` に記録されるので、
応答が短いときの原因は追えます。

### 動いている様子を見る

**タブに仕事を渡しても画面は動きません。**見せたいときは、そう書きます。

```lua
shikisha.show("検査")             -- このタブを画面に出す
shikisha.send_to_tab("検査", msg) -- そのうえで仕事を渡す
```

この2行の順で書けば、勝手に切り替わることはありません。`shikisha.show(0)` で盤面へ戻ります。

**人間のほうが常に優先されます。**基本設定の「自動切り替え」を切っているとき、
直前に自分で画面を動かしたとき、設定画面を開いているときは `show` は何もしません。
読んでいる途中で引き剥がされることはありません。

なお、画面が追わなくても**ボール自体は盤面を飛びます**。ボールは「今どのタブが仕事を持っているか」
を示すもので、「自分がどこを見ているか」とは別の話だからです。

---

### 人が書き足す下書きを置く

`send_to_tab` は打ち込んで送信します。**送らずに入力欄へ置いておきたい**ときは、
ペーストとして送り、改行を送らないようにします。

```lua
shikisha.draft_to_tab("ai", "lp.html を読んでください。

")
```

入力欄に入ったまま止まります。改行は**キーではなく文字**として扱われるので送信されず、
人が続きを書いてから自分で Enter を押せます。このソフト自身も送信と見なさないため、
そのタブで `on_done` が撃たれることもありません。

**下書きは連鎖の終わりではなく、輪の中に人を入れることです。** ボールは深さを保ったまま
そのタブへ移って待ち、画面も追いかけるので、**呼ばれた場所に自分が着きます**。
そのタブで打鍵しても、他のタブと違って連鎖は切れません。**乗っ取りではなく、自分の番**
だからです。送れば数え上げはそのまま続き、連鎖の上限も効きます。
**人が入っていても、回り続ける輪は輪なので。**

**シェルへは置きません。断って `logs/hooks.log` に理由を残します。**
端末のプログラムは「貼り付けを理解するか」を自分で申告します。シェルは申告しないので、
同じものを送ると中身がコマンドとして実行されてしまいます。実測では
`cmd.exe` も `powershell.exe` も申告せず、Claude Code は申告しました。
**コマンド名から推測するのではなく、その申告を読んでいます。**

下書きは短くしてください。長いと `[Pasted text #1 +N lines]` に畳まれ、
**これから送る中身が人に読めなくなります。**

---

### ブラウザを動かす

ブラウザも楽団に加われます。エンジンは Windows に最初から入っているので、
**何もダウンロードせず、何もインストールしません。**

ワークスペースのタブと並べて宣言します。宣言したブラウザは、**セッションの続きの
番号でタブになります。** `Ctrl+B` に続けてその番号を押せば、他のタブと同じように
切り替わります。

```json
{
  "name": "LP検討",
  "browsers": [{ "id": "br", "url": "https://example.com/login" }],
  "tabs": [{ "name": "Claude", "id": "ai", "command": "claude" }]
}
```

### ブラウザのフック

ブラウザには**ブラウザの言葉**があります。セッションの状態（実行中・完了・質問）は
ページには当てはまらないので、別の名前を使います。

| ファイル | いつ呼ばれるか |
|---|---|
| `on_load.lua` | ページの読み込みが終わった（**移動のたび**） |
| `on_press.lua` | 人が帯のボタンを押した |

**帯は放っておいても出ません。** `shikisha.browser_ask` を呼んだときだけ、
ページの最下部に出ます（左に文言、右にボタン）。押されると `on_press` が呼ばれます。
出しっぱなしにしておけば、人がいつ押しても受け取れます。

```lua
-- scripts/lp/on_load.lua
shikisha.browser_ask(page.id, "ログインが終わったら押してください", "できました")
```
```lua
-- scripts/lp/on_press.lua — 押されたら続きをやる
shikisha.browser_unask(page.id)
shikisha.draft_to_tab("ai", shikisha.browser_html(page.id))
```

帯はページ側のCSSの影響を受けません（Shadow DOM に入れてあります）。

### ページの上に、戻る・進む・更新・URL欄を出す

人に自分でページを選んでもらってから解析させたいときは、`shikisha.browser_nav`
でページの上に操作を出せます。**帯と違ってページの中には描きません。**
ページを一段下げて、空いた場所にアプリが描くので、遷移しても消えず、
サイト自身の固定ヘッダーを覆うこともありません。

```lua
shikisha.browser_nav(page.id)                        -- 全部出す
shikisha.browser_nav(page.id, { reload = true, url = true })  -- 選んで出す
shikisha.browser_unnav(page.id)                      -- 引っ込める
```

| 名前 | 出るもの |
|---|---|
| `back` | ← 戻る（戻れないときは押せません） |
| `forward` | → 進む |
| `reload` | ⟳ 更新 |
| `url` | URL欄。人が打った先へ移ります（http/https のみ） |

**設定画面のブラウザタブでも同じことを選べます。** そちらで選んでおけば、
`on_load` に何も書かなくても最初から出ます。Luaから呼べば設定より優先されます。

**帯も同じです。** 設定画面の「帯（ボタン）」に文言とボタンの字を書けば、
開いた時点から出ます。そうすると書くのは `on_press.lua` の1枚だけになります。

```lua
-- scripts/lp/on_press.lua — これだけで「人が選んで、押したら渡る」
shikisha.draft_to_tab("ai", shikisha.browser_html(page.id))
```

**URLを打っても `on_load` が焚かれます。** 移動のたびに解析させたくなければ、
`on_load.lua` は空のままにして `on_press.lua` にだけ書いてください。
それで「人が選んで、押したときだけ渡る」になります。

なお**連鎖の深さは動きません。** 深さが増えるのは他のタブへ渡ったときだけです。

受け取るのは `tab` ではなく `page` です。

| | 中身 |
|---|---|
| `page.index` | 画面の番号（人が押す番号と同じ） |
| `page.id` | 自動化から指す呼び名 |
| `page.name` | 人が読む名前 |
| `page.url` | いま開いているURL |
| `page.complete` | 参照しているものまで揃ったか |

```lua
-- scripts/lp/on_load.lua
if shikisha.get_var("saved") == page.url then return end   -- 移動のたびに来る
shikisha.set_var("saved", page.url)

local name = shikisha.now() .. ".html"
shikisha.write_file("tmp", name, shikisha.browser_html(page.id))
shikisha.draft_to_tab("ai", "tmp/" .. name .. " を読んでください。\n\n")
```

**`page.complete` が false のとき**は、`load` が来ないので読み込み途中で呼ばれています。
広告のページでは外部の計測タグが終わらないことがあり、そのまま待つと永久に来ません。
中身が要るなら `browser_wait` でセレクタを待ってください。

**渡す相手が起動しきっていなくても構いません。** `draft_to_tab` と `send_to_tab` は、
相手が入力を受け取れるようになるまで待ってから渡します。捨てられて消える、
ということは起きません（30秒待って駄目なら、その旨を知らせます）。

あとは自動化から動かします。

```lua
-- ログインは人にやってもらう。ページ下に帯が出る
local why = shikisha.browser_wait("br", {
  selector = "#dashboard",            -- ここへ着いたら抜ける
  ask      = "ログインしてください",    -- ボタンでも抜ける
  timeout_ms = 300000,
})
shikisha.log("抜けた理由: " .. why)   -- selector / button / timeout

shikisha.browser_fill("br", "#title", answer)
shikisha.browser_click("br", { xpath = '//button[text()="保存"]' })
local html = shikisha.browser_html("br")
```

セレクタは `"#id"`（CSS）か `{ xpath = "..." }` か `{ ref = N }` です。XPath は入力フォームや
管理画面で効きます。**「『名前』というラベルの隣のセル」は CSS では書けません。**

`{ ref = N }` の番号は `browser_digest` が発行します:

```lua
local list = shikisha.browser_digest("br")
-- [1] textbox "検索" placeholder="検索"
-- [2] button "検索"
-- [3] link "ヘルプ" https://example.com/help
shikisha.browser_fill("br", { ref = 1 }, "俳句")
shikisha.browser_click("br", { ref = 2 })
```

digest はページを**操作できる要素だけ**に蒸留した一覧です。role と名前はブラウザ自身の
アクセシビリティツリー（スクリーンリーダーが見るものと同じ計算結果）から取り、標準の
role を持たない JS クリッカブル（`cursor:pointer` な `<div>` など）は `div*` のように
`*` 印で補完します。生 HTML を読むより桁違いに短く、セレクタを推測で書く必要がなくなります。

そして `{ ref = N }` への操作は **本物の入力**（CDP 経由の信頼済みマウス/キーイベント）に
なります。合成イベントを無視するサイトでも、人間のクリック・タイプと区別が付きません。
日本語などのマルチバイト文字も IME を経ずに1文字ずつ確定入力されます。

番号はその時点のページに紐づきます。ページが変わる（遷移・再描画）と失効し、古い番号への
操作は「digest を取り直して」という明確なエラーで止まります — 別の要素を黙って
クリックすることはありません。

さらに `{ ref = N }` への click / fill は **2値目に「実際に操作した要素」のエコー**を
返します（例: `visible, link 「ヘルプ」`。fill のエコーは欄の属性だけで、値は含みません）。
番号を取り違えても、返ってきたエコーがその場で告発します。

**再現・持ち運びについて**: `{ ref = N }` はどの実行モード（automation スクリプト、
composer の ▶ Lua 実行、operate のラリー）でも同じ意味を持つ通常のセレクタです。ただし
番号は「直前の `browser_digest` の一覧」への参照なので、ref のまま持ち運ぶものではありません。

そこで **実行と記録は独立しています**。operate のラリーでは、実行された各操作が
**耐久形に書き直されて** run フォルダの `replay.lua` に積まれます — `{ ref = N }` は
「実際に触った要素」から導出したアンカー（人が付けた `#id`、無ければ一意なテキスト/属性の
XPath。📼 レコーダと同じ流儀で、機械生成の id は拒否）に置き換わり、`browser_digest` は
一行も現れません。つまり:

- **実行の通貨 = ref**（能力最大: shadow DOM も届く・本物入力・計量モデルに優しい）
- **持ち運びの通貨 = replay.lua**（素の css / xpath だけ。▶ 実行モードに貼っても、
  automation に組み込んでも、別 PC の SHIKISHA でも、そのまま動く）

replay.lua は、🎯 ターゲットパネルのプルダウン横の「⬇ 再現Lua」ボタン、または操作終了時に
開く結果ビュー右上の同名ボタンからダウンロードできます。耐久アンカーを導出できなかった操作は
黙って欠落させず、`-- click (…): 何を押したか` というコメントで残ります。

要素を探すと3つの状態が返ります（`visible` / `off_screen` / `not_found`）。
**セレクタを疑うのか、待ちを疑うのか**が、これで決まるからです。

**見つからないときに止めるかどうかは、呼び出しごとに選べます。** 既定では止まり、
`{ on_missing = "continue" }` なら状態を返して進みます。出たり出なかったりする
Cookieバナーは失敗ではありませんが、**それを知っているのは呼んだ側だけ**です。

**セレクタを指定していても、ボタンは待っている間ずっと出します。** サイトの改修で
条件が合わなくなったとき、**止まるのではなくクリック1回で済む**ようにするためです。
そして抜けた理由が返るので、**毎回 button で抜けているなら、そのセレクタは
一度も効いていない**と分かります。

**click / fill は自動で待ちます(auto-wait)。** 要素が **現れる → 見える → 動きが
止まる(連続フレームで矩形が同一) → 無効でない** まで待ってから操作します。リトライは 0/20/100/100/500ms のバックオフ、
リトライ毎にスクロール位置を変えて sticky なオーバーレイを外し、ページ遷移で JS 世界が
消えても外側のリトライが新しいドキュメントに入り直します。だから **`browser_go` の直後に
次ページの要素を操作する連打スクリプト(replay.lua)がそのまま通ります**。待ち時間の
上限は操作あたり 10 秒で、要素が存在するのに最後まで安定しなかった場合は従来どおり
その場で操作します(新しい失敗モードは増やしません)。

**値がコードになることはありません。** `fill` に渡したものは全てデータとしてページへ
届くので、引用符や山括弧だらけの回答でも、そのまま入って何も起こしません。
**生のJavaScriptをページへ渡す口は、意図的に用意していません。**

開けるのは `http` と `https` だけです。1行の `<input>` は改行を保持できません
（このソフトではなく HTML の仕様です）ので、複数行を入れるなら `textarea` が要ります。

---

## 5. 安全のしくみ

自動化が暴走しないよう、いくつもの歯止めがあります。

- **自動チェーン上限** … AI同士の自動転送が続いた回数を数え、上限（既定10回）で止まります。
  人間が手で入力すると0に戻ります
- **手動操作の優先** … 人間が触った直後5秒は自動送信されません
- **緊急停止** … `Ctrl+B x` で全自動化を即停止、`Ctrl+B a` でON/OFF。
  ステータス行にも同じボタンがあり、どの画面でも同じ位置に出ます
- **入力ロック** … 中間タブを🔒にしておけば、人間が誤って指示を出せません
- **サンドボックス** … 自動化からはファイル操作もインターネット接続も**既定ではできません**。
  通知先も、設定に登録済みのSlack / Telegramにしか送れません

---

## 6. ファイル・通信を使う（上級者向け・既定は無効）

必要な場合だけ、`config.json` に「窓口」を登録すると使えるようになります。
設定画面からは編集できません（影響が大きいため、ファイルを直接編集する人だけの機能です）。

```jsonc
"capabilities": {
  "files": {
    "reports": { "dir": "reports", "read": true, "write": true }
  },
  "http": {
    "github-issue": {
      "url": "https://api.github.com/repos/me/proj/issues",
      "method": "POST",
      "auth_from_secrets": "github_token"
    }
  }
}
```

```lua
shikisha.write_file("reports", "review.md", tab.output)
local prev = shikisha.read_file("reports", "review.md")
shikisha.http("github-issue", '{"title":"指摘","body":"..."}')
```

| 命令 | 説明 |
|---|---|
| `shikisha.now([書き方])` | いまの日時を文字列で返す |
| `shikisha.write_file(窓口, ファイル名, 文字列)` | 登録済みフォルダへ書き込む |
| `shikisha.read_file(窓口, ファイル名)` | 登録済みフォルダから読む |
| `shikisha.http(窓口, 本文)` | 登録済みURLへ送信（認証はアプリが付与） |

### 日時

保存するものに時刻の名前を付けたいことは、よくあります。

```lua
shikisha.now()                  -- 20260807012604 （既定）
shikisha.now("%Y-%m-%d")        -- 2026-08-07
shikisha.now("%Y%m%d") .. ".html"
```

綴りは `os.date` と同じです。使えるのは `%Y` `%y` `%m` `%d` `%H` `%M` `%S` と、
`%` そのものを出す `%%` です。知らない綴りはそのまま残ります。

**`os` は渡していません。** 日時のほかにプロセスを起こす道具もファイルを消す道具も
入っているので、日時のために丸ごと渡すわけにはいきません。

**この方式の安全性**: スクリプトはパスもURLも組み立てられず、登録済みの名前しか
呼べません。認証トークンはスクリプトから見えず、アプリが付与します。
`config.json` / `secrets.json` / `.env` / `.lua` ファイルは、許可フォルダ内にあっても
常に読み書きできません。

さらに自由度が必要なら、生パス・生URLも使えます（**既定は空＝全拒否**）:

```jsonc
"capabilities": {
  "allow_dirs": ["reports"],
  "allow_hosts": ["api.example.com"]
}
```

```lua
shikisha.write_path("reports/a.md", "text")
shikisha.http_raw("https://api.example.com/hook", '{"x":1}')
```

接続先はホスト名の**完全一致**で照合し、`https` のみ許可されます
（`api.example.com.evil.com` のようなすり抜けは弾かれます）。
ファイル・通信は必ず `logs/hooks.log` に記録されます。

---

## 7. 外から操る（外部API）

アプリの外にいるプログラムから、Luaで書くのと同じ命令を呼べます。名前も引数も同じで、
覚え直す語彙はありません。

入口は **名前付きパイプ** `\\.\pipe\shikisha-<pid>` です。1行1JSON、返事も1行:

```text
→ {"token":"…"}                                                  最初の1回だけ、合言葉
← {"ok":true,"result":"hello"}

→ {"id":"1","method":"send_to_tab","params":["reviewer","状況は?"]}
← {"id":"1","ok":true,"result":null}

→ {"id":"2","method":"list"}
← {"id":"2","ok":true,"result":["browser_click","browser_close", … ]}
```

`method` は9章の命令から `shikisha.` を取った名前、`params` はその引数を順に並べたものです。
`list` は存在する命令を全部返します。アプリ自身の表を読んで答えるので、**実際にできることと
食い違うことがありません**。

ループや分岐は、まとまったコードを1回で渡せます:

```text
→ {"id":"3","method":"lua","params":["for i=1,3 do shikisha.send_to_tab(i,'ping') end"]}
← {"id":"3","ok":true,"result":[null,null]}
```

`lua` の答えは必ず2つ組で、**1つめがエラー（実行できたときは `null`）**、続けてコードが
返した値が並びます。

### 誰が入れるか

```jsonc
"external_api": { "access": "children" }   // 既定
```

| 値 | 呼べる相手 |
|---|---|
| `children` | アプリが起動したものだけ（タブのCLIと、それがさらに起動したもの） |
| `user` | あなたの権限で動くものすべて。合言葉は `datapi-token` にも書かれます |
| `off` | 誰も。パイプ自体を作りません |

タブのプロセスは、起動の時点で次の3つを知らされています。タブの中のAIは、何の準備もなしに
そのまま呼べます。

| 変数 | 中身 |
|---|---|
| `SHIKISHA_PIPE` | つなぐ先のパイプ |
| `SHIKISHA_TOKEN` | そのタブ専用の合言葉（起動時に発行） |
| `SHIKISHA_TAB` | 自分がどのタブにいるか |

合言葉がタブごとなので、呼び出しは**誰からのものか分かった状態で届きます**。そしてそのタブが
送ったものは、画面越しに渡したときと**同じ連鎖の上限**（5章）に数えられます。
外部APIはブレーキの抜け道ではありません。

**合言葉が守るもの・守らないもの。** パイプはあなたのアカウントだけを許可するアクセス制御を
付けて作られるので、別アカウントからは届きません。ただし**あなたの権限で動く別のプログラムは
あなたのプロセスの環境変数を読めます**し、タブの中のAIが自分の合言葉をログに書くこともあります。
これで防げるのは「事故」と「別アカウント」までで、すでにあなたである相手は防げません。

そのセッションで最初につないできた相手は `logs/hooks.log` に記録されます。合言葉が違う接続も
同じく残ります。

---

## 8. 書き方のこつ

- **文字の連結は `..`** です（`+` ではありません）
- **`tab.output` は直前の応答だけ**が入ります。過去の会話は含まれません
- **正規表現はLua独自**です。`%d`（数字）、`%s`（空白）、`.-`（最短一致）など。
  `\d` ではなく `%d` と書きます
- **何もしたくないときは `return`** と書けば、その場で終わります
- 迷ったら `shikisha.log()` を仕込んで `logs/hooks.log` を見てください

---

## 9. 命令の一覧

自動化から呼べるものを、すべてここに置きます。上の章はよく使うものの説明で、
こちらが全部です。

### タブと手番

| 命令 | 説明 |
|---|---|
| `shikisha.send_to_tab(タブ, "文字列")` | **タブに指示を渡して実行させる。** 自分自身にも使えます（チェーン+1） |
| `shikisha.send(タブ, "文字列")` | 生のキー入力（改行は `\r`）。指示ではなく、確認への返答用 |
| `shikisha.draft_to_tab(タブ, "文字列")` | 入力欄に置くだけで**実行しない**。人が書き足して送ります |
| `shikisha.state(タブ)` | 今の状態: `WAIT` / `BUSY` / `DONE` / `ASK` / `EXIT` |
| `shikisha.wait_state(タブ, "DONE", ミリ秒)` | その状態になるまで待つ。なれば `true` |
| `shikisha.tab_output(タブ)` | 他のタブの最新の返答（まだ無ければ `""`） |
| `shikisha.restart(タブ)` | そのタブを再起動する |

### 画面

| 命令 | 説明 |
|---|---|
| `shikisha.show(タブ)` | そのタブを画面に出す。`0` は盤面。「自動切り替え」を切っているとき、直前に人が画面を動かしたとき、設定画面を開いているときは何もしません |
| `shikisha.open_result(run)` | その実行の記録を結果ページとして開き、そこへ移動する |

### 待つ・時刻

| 命令 | 説明 |
|---|---|
| `shikisha.wait(タブ, "正規表現", ミリ秒)` | そのタブの画面にその文字が出るまで待つ。出れば `true` |
| `shikisha.sleep(ミリ秒)` | 待つ（待っている間も他のタブは動きます） |
| `shikisha.now("%Y-%m-%d")` | 現地の日時を整形して返す。既定は時系列に並ぶ形なので、ファイル名向き |
| `shikisha.epoch_ms()` | エポックからのミリ秒（数値）。経過時間の計測用 |

### 覚える・記録する・知らせる

| 命令 | 説明 |
|---|---|
| `shikisha.get_var("キー")` / `shikisha.set_var("キー", 値)` | 記憶しておける変数。ワークスペース内で共有 |
| `shikisha.log("文字列")` | `logs/hooks.log` に1行書く |
| `shikisha.notify("文字列")` | Slack / Telegram へ通知（設定済みの宛先のみ） |
| `shikisha.remote_url()` | スマホからこのアプリに繋がるURL。リモートが切れているときは `nil`。通知に入れておくと「手伝いに来て」がワンタップになります |
| `shikisha.t("キー")` / `shikisha.tf("キー", {name="…"})` | 訳語を引く（`tf` は `{name}` も差し込む）。組み込みの進行役がアプリの言語で話すために使っています |

### ブラウザを動かす

ページは付けた id で指します。上の「ブラウザを動かす」も参照してください。

| 命令 | 説明 |
|---|---|
| `shikisha.browser_open(id, url, profile, private)` | ページを開く。`profile` はcookieの入れ物の名前、`private` は使い捨て |
| `shikisha.browser_close(id)` | 閉じる |
| `shikisha.browser_go(id, "back"/"forward"/"reload"/"to", url)` | 移動する |
| `shikisha.browser_nav(id, {…})` / `shikisha.browser_unnav(id)` | ページの上に戻る・進む・更新・URL欄を出す／消す |
| `shikisha.browser_find(id, セレクタ)` | あるか: `"visible"` / `"hidden"` / `"missing"` |
| `shikisha.browser_click(id, セレクタ)` | 押す |
| `shikisha.browser_fill(id, セレクタ, "文字列")` | 入力する。**送信はしません** — 続けて `browser_press` |
| `shikisha.browser_fill_secret(id, セレクタ, "キー")` | 登録済みの秘密情報から入力する。値はスクリプトに渡りません |
| `shikisha.browser_press(id, "enter")` | ページ上でキーを押す |
| `shikisha.browser_text(id, セレクタ)` | 見えている文字 |
| `shikisha.browser_html(id)` | 文書全体 |
| `shikisha.browser_digest(id)` | 操作できる要素の一覧（番号付き）。次の手を決める前に読むもの |
| `shikisha.browser_fetch(id, url, opts)` | ページの中から通信する（cookieを引き継ぐ）。`{status, ok, url, headers, body}` を返す |
| `shikisha.browser_auth(id, "キー")` | 登録済みの秘密情報でBasic認証に答える |
| `shikisha.browser_ask(id, "文字列", "ラベル")` | ページの下端にボタン付きの帯を出す |
| `shikisha.browser_pressed(id)` | 押されたか |
| `shikisha.browser_unask(id)` | 帯を消す |
| `shikisha.browser_wait(id, {ask=…, selector=…, timeout_ms=…})` | 早い者勝ちで待つ。`"selector"` / `"button"` / `"timeout"` を返す |

### 参加者のあいだで実行を受け渡す

ラリーの仕組みそのものです。ファイルの受け渡しと審判。同じ道具で自作もできます。

| 命令 | 説明 |
|---|---|
| `shikisha.exchange_new()` | この実行用のフォルダを作り、その場所を返す |
| `shikisha.exchange_write(パス, "文字列")` | ファイルに書く（上書き） |
| `shikisha.exchange_append(パス, "文字列")` | 追記する |
| `shikisha.exchange_take(パス)` | 読んで、消して、返す。無ければ `nil` — これが受け渡しの本体 |
| `shikisha.lint(コード)` | Luaを実行せずに構文検査する。壊れていればエラー文字列、健全なら `nil` |
| `shikisha.run_scoped(id, コード)` | AIが書いたLuaを1つのページに対してだけ実行する牢屋。ファイルも通信も他のタブも触れません。`err, out` を返す |
| `shikisha.lua(コード)` | まとまったコードを、何にでも手が届く場所で実行する（ループも分岐も、複数の命令も一度に）。`err`（実行できたら `nil`）に続けて、コードが返した値をそのまま返す。`run_scoped` の牢屋なし版なので、自分で書いていないコードを渡さないこと |
| `shikisha.list()` | 存在する命令の名前を全部返す。表そのものを読むので、古くなりようがない |
| `shikisha.record(文字列)` / `shikisha.record_reset()` | 貼り直せる形で実行の記録を残す |
| `shikisha.take_replay()` | 再生用の記録を取り出す（前回取り出して以降の全操作を、壊れにくい書き方で） |
| `shikisha.set_result(コード, "理由")` | この実行の判定。`data/last-result.json` に書かれ、画面にも出ます |

### ファイル・通信

「窓口」を登録しない限り使えません。6章を参照してください。

| 命令 | 説明 |
|---|---|
| `shikisha.read_file(名前, 相対パス)` / `shikisha.write_file(名前, 相対パス, データ)` | 登録済みのファイル窓口を通して |
| `shikisha.http(名前, 本文)` | 登録済みのHTTP窓口を通して |
| `shikisha.read_path(パス)` / `shikisha.write_path(パス, データ)` / `shikisha.http_raw(url, 本文)` | 生のパス・生のURL。`allow_dirs` / `allow_hosts` が空のあいだは必ず失敗します |

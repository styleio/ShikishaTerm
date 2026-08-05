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
| `on_done.lua` | AIの応答が完了したとき |
| `on_question.lua` | AIが確認・選択肢を出してきたとき |
| `on_exit.lua` | セッションが終了したとき（切断・クラッシュを含む） |
| `on_busy.lua` | 応答が始まったとき（上級者向け） |
| `_shared.lua` | 上記より先に読まれる。共通の下請け関数を置く場所 |

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
| `tab.output` | **直前の応答テキスト**（過去の履歴は含まれない） |
| `tab.state` | `"BUSY"` / `"DONE"` / `"QUESTION"` / `"WAIT"` / `"EXIT"` |
| `tab.profile` | 適用中のプロファイル名 |
| `tab.chain_depth` | 自動転送が何回連鎖したか。**0なら人間が始めた会話** |
| `tab.locked` | 入力ロック中かどうか |

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
| `shikisha.send_to_tab(タブ, "文字列")` | 他のタブへ送信して実行させる（自動チェーン+1） |
| `shikisha.send(tab, "文字列")` | そのタブへキー入力を送る（改行は `\r`） |
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

---

## 4. よくある例

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

## 5. 安全のしくみ

自動化が暴走しないよう、いくつもの歯止めがあります。

- **自動チェーン上限** … AI同士の自動転送が続いた回数を数え、上限（既定10回）で止まります。
  人間が手で入力すると0に戻ります
- **手動操作の優先** … 人間が触った直後5秒は自動送信されません
- **緊急停止** … `Ctrl+B x` で全自動化を即停止、`Ctrl+B a` でON/OFF
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
| `shikisha.write_file(窓口, ファイル名, 文字列)` | 登録済みフォルダへ書き込む |
| `shikisha.read_file(窓口, ファイル名)` | 登録済みフォルダから読む |
| `shikisha.http(窓口, 本文)` | 登録済みURLへ送信（認証はアプリが付与） |

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

## 7. 書き方のこつ

- **文字の連結は `..`** です（`+` ではありません）
- **`tab.output` は直前の応答だけ**が入ります。過去の会話は含まれません
- **正規表現はLua独自**です。`%d`（数字）、`%s`（空白）、`.-`（最短一致）など。
  `\d` ではなく `%d` と書きます
- **何もしたくないときは `return`** と書けば、その場で終わります
- 迷ったら `shikisha.log()` を仕込んで `logs/hooks.log` を見てください

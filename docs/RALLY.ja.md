# AI⇄ブラウザ ラリー機能 実装計画

AIタブとブラウザタブが往復（ラリー）して、目的の動作（例：日記SaaSへの投稿）を
自律的に完了させる機能。人間は横で全部を目視でき、必要な時だけ歯車として組み込まれる。

この文書は「何を・どの順で作るか」の計画書。実装はフェーズごとに検証しながら進める。

---

## 1. ビジョン（想定フロー）

日記SaaSへの投稿を例に：

1. AIが目的に向けて考える（思考はAIタブに表示される）
2. AIがLuaを出す → **自動でブラウザタブに切替** → Luaが実行されるのを人間が目視
3. 終わったら**自動でAIタブに戻る** → AIが考える
4. 「ログインページだから人間の助けが要る」と判断 → Luaを出す → ブラウザに切替
5. 「ログインしてボタンを押して」と帯が出る → 人間がログイン → ボタン押下
6. AIに戻る → 遷移後のページから投稿ページを特定 → Luaでクリック/遷移
7. 投稿画面へ → AIに戻る → **目的の投稿内容をファイルから読んで**フォームに入力 → 投稿
8. ブラウザで実行 → AIに戻る → 「ちゃんと投稿できた」と判断 → **終了（コード）**

---

## 2. 設計原則（これは曲げない）

- **専用エンジンを作らない／全部Luaで表現する。** ラリーの流れ・判定・再生はすべて
  素の `shikisha.*` プリミティブの組み合わせ。Rust側にラリー専用の分岐は作らず、
  汎用プリミティブと受け皿だけを足す。
- **記録＝実行されたLua。** AIはLuaをテキストで送るので、司令塔はそのソースを
  そのまま順に連結すれば再現用スクリプトになる（`data/last_rally.lua`）。
- **再生＝貼るだけ。** 記録したLuaをブラウザタブの `on_done.lua` に貼れば、AI無しで
  同じブラウザ操作を再現できる（人間待ちステップも含めて）。
- **観戦モード（自動タブ切替）が主役。** 手番に応じて表示タブを自動で動かし、
  人間が全工程を目視できる。実行はゆっくり刻める。
- **GitHub Secrets 契約。** 秘密は名前で参照し、復号値はAIの世界に一度も出さない
  （構造的に不可能にする＋遮蔽＋檻）。詳細は §5。
- **既定は全拒否。** capability も秘密も、明示的に許可したものだけ使える。
  上級者は「危険承知の全許可」を一手で選べる。

---

## 3. アーキテクチャ

### 3.1 司令塔＝Luaスクリプト

AIタブの `on_done`（応答完了フック、デバウンス済み）を司令塔にする。
既存のアプリ実績のある完了検知をそのまま使う。

- `on_start.lua`：AIへ「目的＋プロトコル＋使えるブラウザ関数」を送る（キックオフ）
- `on_done.lua`：AIの直近出力を解析し、
  - `` ```shikisha-lua … ``` `` → ①ブラウザで実行 → ②状態収集 → ③AIへ返信 → ループ
  - `` ```shikisha-done code=N reason=… ``` `` → `set_result(N, reason)` で終了

### 3.2 AI↔司令塔プロトコル（センチネル方式）

AIはCLI（claude/codex等）なので、決まった書式で読み書きする：

- 操作：`` ```shikisha-lua `` … `` ``` `` の囲みでLua文を出す
- 完了：`` ```shikisha-done `` に `code` と `reason` を書く
- 人間依頼：`«HUMAN» ログインしてください` を出す → 司令塔が帯を出して待つ

司令塔は `tab.output`（確定した応答）をこの書式で解析する。書式外なら
「Luaブロックか完了マーカーを1つ出して」と返して促す。

### 3.3 人間を歯車に組み込む

`browser_ask`/`browser_wait`（既存）を再利用。行き詰まり全般（ログイン、
reCAPTCHA、確認）を「人へ回す」1つの汎用機構に集約する。
- reCAPTCHA … DOMで検知（`browser_find`）→ 人が可視タブで解く（突破はしない）
- ベーシック認証 … CDPで検知（§6）→ `browser_auth` で自動、または人へ

---

## 4. 新規プリミティブ（すべて素のLua関数）

| プリミティブ | 役割 | 実装 |
|---|---|---|
| `shikisha.show(対象)` | 表示タブを切替（観戦モード） | 新Command→`active`を設定 |
| （刻み） | 実行をゆっくり見せる | 既存の `shikisha.sleep` で足りる |
| `shikisha.browser_fetch(name,url,opts)` | `{status,ok,body,headers,url}` | ページ内 `fetch` を eval（async結果） |
| `shikisha.set_result(code,reason)` | 終了コード＋理由 | 新Command→ファイル+ログ+UI |
| `shikisha.record(lua)` | 実行Luaを記録に追記 | 信頼側の書き込み（`data/last_rally.lua`） |
| `shikisha.run_scoped(code,opts)` | AI製Luaを制限環境で実行 | curbらた_ENV（browser＋宣言窓口のみ） |
| `shikisha.browser_auth(name,秘密名)` | ベーシック認証を秘密で応答 | CDP Fetch.continueWithAuth |
| `shikisha.browser_last_response(name)` | 遷移の本当のHTTP状態/BODY | CDP Network（後期フェーズ） |

`browser_open/find/click/fill/text/html/close`、`ask/unask/pressed/wait`、
`read`（宣言窓口）、`send_to_tab` は既存。

---

## 5. セキュリティ（秘密・許可・暴走ガード）

### 5.1 秘密のモデル（GitHub Secrets 相当）

- 保存：`secrets.json`（AES-GCM暗号化、マスターパスワードと接続）
- 参照：**名前だけ**（`browser_auth("br","diary_saas")`）。値を返す関数は作らない
- 遮蔽：AIへ向かう全テキスト（html/text/fetch本文）・ログ・画面・スクショ・
  `last_rally.lua` から**既知の秘密値をマスク**。`type=password` のDOM値は
  `browser_text/html` から常に伏字
- 檻：AI製Luaはサンドボックス（file書き込み・生HTTP・os 不可）＝持ち出し口が無い
- 密閉が完全なケース：ベーシック認証（CDP注入）は値がDOMにもレスポンスにも
  載らないので到達経路ゼロ
- 残余：フォームのパスワード入力は一瞬DOMに入る。password欄マスク＋値スクラブで
  塞ぐが、悪意ページ＋悪意AIが値を変形してエコーする経路は理論上残る
  → CDP注入を優先、最大保証が要る所は人間入力に回す

### 5.2 許可リスト（既定拒否）

```
全拒否(既定) ── ワークスペースで選択(通常) ── 全許可トグル(危険承知)
   安全 ←─────────────────────────────────→ 便利
```

- ワークスペース設定に「使える秘密」チェックリスト（キー＋説明が並ぶ）
- 「全許可（危険）」トグルを警告付きで用意
- ラリーはそのワークスペースが許可した秘密名しか参照できない
  （別用途の鍵の流用を封じる）

### 5.3 秘密登録GUI

- 一覧：`キー`＋`説明`＋`登録済み` のみ表示（値は絶対に出さない）
- 追加/更新：`ユニークキー`／`説明`／`値`（保存後は二度と表示不可、上書きで差替）
- 削除可。APIは値を返す経路を一切持たない。設定GUIは 127.0.0.1＋トークンのみ

### 5.4 暴走ガードレール

- `chain_depth`/`max_chain`（既定10）でラリー往復に上限（既存）
- 緊急停止（Ctrl+B x / auto off）で全コルーチン破棄（既存）
- サンドボックス＋秘密の許可リスト（上記）

---

## 6. 状態取得（ステータス/BODY）の段階導入

- **第1段：`browser_fetch`＋DOM**。ページ内fetchで `{status,body}`、URL/HTMLで判定。
  ログイン/API/投稿フローはこれで通せる。
- **第2段：CDP Network**。`Network.enable`＋`responseReceived`＋`getResponseBody` で
  **実際のページ遷移/フォーム送信の本当のHTTPステータス/本文**を観測。
  `Fetch.authRequired` はベーシック認証の検知＆応答点にもなる。

---

## 7. フェーズ計画（作る順）

各フェーズは独立に検証可能（単体テスト＋デバッグサーバ＋スクショ）。

- **P0 基盤プリミティブ**：`shikisha.show`（観戦の要）／`browser_fetch`。
  汎用的でラリー以外にも有用。
- **P1 記録**：実行Luaを `data/last_rally.lua` に追記（司令塔Lua＋信頼側書込）。
  成果物：ラリー後、貼れば再生できるLuaが残る。
- **P2 終了コード**：`set_result` → `data/last_result.json`＋ログ＋UIバッジ。
- **P3 サンドボックス**：`run_scoped` でAI製Luaを browser＋宣言窓口に限定。
- **P4 秘密**：ストア確定／登録GUI（write-only）／名前参照consumer／遮蔽（redaction）／
  ワークスペース許可リスト＋全許可トグル。
- **P5 ベーシック認証＋captcha**：CDP Fetch で検知/応答（`browser_auth`）。
  reCAPTCHAはDOM検知→人間（既存ask/wait）。
- **P6 CDP Network**：遷移の本当のHTTPステータス/本文（`browser_last_response`）。
- **P7 参照司令塔＋ドキュメント**：`on_start.lua`/`on_done.lua` の雛形と、
  記録/再生・秘密・許可リストの手順書。ワークスペースに載せれば動く形。
- **(将来) P8 ヘッドレス終了コードモード**：`--rally` で1回実行しプロセス終了コードで返す
  （CI/バッチ需要が出たら）。

ラリーはP3（値ベタ書き）またはP4（秘密込み）で端から端まで動くようになり、
以降で忠実度と安全性が上がる。

### 着手順

P0 から。まず `shikisha.show`（自己完結・観戦UXの要）を実装・検証し、次に
`browser_fetch`。並行して最小の司令塔でラリーの往復を通し、P1以降を積む。

---

## 9. 実装状況（2026-08-11）

| フェーズ | 状態 |
|---|---|
| P0 `show` / `browser_fetch` | ✅ 実装・実機検証済み |
| P1 記録 `record`/`record_reset` | ✅ |
| P2 終了コード `set_result` | ✅ |
| P3 サンドボックス `run_scoped` | ✅ |
| P4a 秘密ストア | ✅ |
| P4b 遮蔽 `redact` | ✅ |
| P4c 秘密登録GUI | ✅ 実機検証済み |
| P4d 許可リスト＋`browser_fill_secret` | ✅（ワークスペース許可リストのGUIは未実装、config直書きで有効） |
| P5 ベーシック認証 `browser_auth`（CDP Fetch） | ✅ 実機検証済み（401→200） |
| P6 CDP Network（遷移の生ステータス） | ⬜ 未着手 |
| P7 参照司令塔＋本ドキュメント | ✅ |

---

## 10. ⚠ 既知のバグ

### 複数ブラウザタブ同時ロードで on_load が最後の1枚しか本体実行されない

**症状**：ブラウザのページフック（`on_load`/`on_press`）は、複数のブラウザ子タブが
ほぼ同時にロード完了したとき、**最後の1枚しか関数本体が走らない**。他のタブは
`fire_page` が呼ばれ `func.call`/`resume` も成功（status=Finished）を返すのに、
**本体が1行も実行されない**（`log` も `record` も呼ばれず、エラーも出ない）。

**切り分け済み**：`page_ctx` は各ペインで Some（別index）、`resolve` は同一スクリプト・
`defines=true`。コルーチンを外して `func.call` 直接呼び出しでも同じ。→ mlua の
スレッド再利用の問題ではない。同一 func の連続呼び出しで片方だけ空振りする。原因未特定。

**影響とワークアラウンド**：**ラリー本体はブロックしない**。司令塔は AIセッションタブの
`on_done`（セッションフック、1件ずつ発火）に置く設計で、そちらは正常。ブラウザ操作は
`on_done` の中から `shikisha.browser_*(browser_name, …)` で行うため、ページフックに
依存しない。ページフックで複数ブラウザを同時に捌く用途だけ、この癖に当たる。

---

## 11. プロトコル（AI ⇔ 司令塔）

AIはCLI（claude/codex等）なので、決まった書式で読み書きする。司令塔は AIの確定応答
（`tab.output`）をこの書式で解析する。

- **ブラウザ操作**：Luaを囲みで出す（この囲みの中身がサンドボックスで実行される）
  ~~~
  ```shikisha-lua
  shikisha.browser_go("br", "to", "https://example.com/login")
  shikisha.browser_fill("br", "#user", "alice")
  shikisha.browser_fill_secret("br", "#pass", "example_login")
  shikisha.browser_click("br", "#submit")
  ```
  ~~~
- **人間の助け**（ログイン・reCAPTCHA等）：
  ```
  «HUMAN» ログインして、できたら画面の「できたら押す」を押してください
  ```
- **完了**（AIが達成/失敗を判定）：
  ~~~
  ```shikisha-done
  code = 0
  reason = ダッシュボードに投稿が表示された
  ```
  ~~~

書式外なら司令塔は「Luaブロックか完了マーカーを1つだけ出して」と促す。

---

## 12. プリミティブ一覧（実装済み）

ブラウザ操作（`name` は操作対象タブのid）：

| 関数 | 返り | 説明 |
|---|---|---|
| `browser_go(name, what, url?)` | – | `what`=`"to"`/`"reload"`/`"back"`/`"forward"`。その場で移動（webviewは作り直さない） |
| `browser_open(name, url)` | – | タブを開く/差し替え（webviewを作り直す。仕込んだ認証は消える） |
| `browser_digest(name)` | 文字列 | 操作可能要素の番号つき一覧（A11yツリー＋JSクリッカブル補完。秘密値はマスク） |
| `browser_find(name, sel)` | `"visible"`/`"off_screen"`/`"not_found"` | 要素の在否 |
| `browser_click(name, sel, opts?)` | 状態, エコー | クリック。`{ref=N}` では2値目に「実際に押した要素」(例: `link 「ヘルプ」`)が返る。`opts.on_missing="continue"`で無くても進む |
| `browser_fill(name, sel, value)` | 状態, エコー | 値を入れる。`{ref=N}` では2値目にどの欄か(属性由来・値は含まない)が返る |
| `browser_fill_secret(name, sel, 秘密名)` | 状態 | 秘密の値を入れる（値はAIに渡らない・記録は鍵名だけ） |
| `browser_text(name, sel)` | 文字列/nil | 要素のテキスト（秘密値はマスク） |
| `browser_html(name)` | 文字列 | ページ全体のHTML（秘密値はマスク） |
| `browser_fetch(name, url, opts?)` | `{status,ok,url,headers,body}` | ページ内fetch（ログイン済みクッキー使用・秘密値はマスク） |
| `browser_auth(name, 秘密名)` | – | ベーシック認証を仕込む（秘密は `user:pass` 形式・以後の401に自動応答） |
| `browser_ask(name, 文言, ボタン?)` / `browser_unask(name)` / `browser_pressed(name)` | – / – / bool | 人への帯 |

観戦・記録・終了・待機：

| 関数 | 説明 |
|---|---|
| `show(対象)` | 表示タブ切替（`0`=INDEX）。手番ごとに見える画面を動かす |
| `record(text)` / `record_reset()` | 実行Luaを `data/last-rally.lua` に追記／始め直し（貼れば再生） |
| `set_result(code, reason)` | 終了コードと理由（`data/last-result.json`＋ログ＋UI） |
| `sleep(ms)` / `wait(tab, 正規表現, ms)` / `browser_wait(name, opts)` | 待機（コルーチンでyield） |
| `run_scoped(name, code)` | AI製Luaを browser限定サンドボックスで実行。返りは `err, out` の2値（成功=`nil`＋返り値の文字列化、失敗=エラー文字列＋`nil`）。裸の式はREPL式に値になる |
| `send_to_tab(tab, text)` / `get_var` / `set_var` | タブへ送信 / スクリプト間共有変数 |

**サンドボックス（`run_scoped`）で見えるのは browser系＋log だけ**。`os/io/load/require`、
`record`/`set_result`/`send_to_tab`/`read_file`/`http`/秘密の生値、他タブには触れられない。

---

## 13. 参照司令塔（雛形）

`docs/rally-example/` に置いた3ファイルを、ワークスペースの **AIセッションタブ** の
`automation` に指すディレクトリへコピーして使う（`_shared.lua` を各自の環境に書き換える）。

- `_shared.lua` … `RALLY.browser`（操作対象ブラウザid）・目的・上限などの設定
- `on_start.lua` … 起動時にAIへ目的＋プロトコルを送る
- `on_done.lua` … AIの手番ごとに解析→サンドボックス実行→状態収集→記録→AIへ返す/終了

流れ（`on_done`）：`shikisha-done`があれば `set_result` で終了 → `«HUMAN»`があれば
`browser_ask`＋`browser_wait` で人へ → `shikisha-lua` があれば `show(browser)`＋`sleep`で
見せてから `run_scoped` で実行し `record`、状態を集めて `send_to_tab` でAIへ返す。

## 14. セットアップ手順

1. **秘密を登録**（必要なら）：設定GUIの「秘密」で `example_login` 等を登録（`user:pass` 形式なら
   ベーシック認証にも使える）。
2. **ワークスペースを用意**：AIセッションタブ（例 `claude`）＋ブラウザタブ（例 id=`br`）。
   AIタブの `automation` を雛形のコピー先ディレクトリに向ける。許可する秘密を
   `secrets_allow: ["example_login"]`（危険承知の全許可は `secrets_allow_all: true`）。
3. **`_shared.lua` を編集**：`RALLY.browser = "br"`、`goal` を目的に。
4. **AUTO ON で起動**。AIが手を出し、ブラウザで実行され、往復する様子を観戦できる。
5. うまくいったら `data/last-rally.lua` を保存 → 別タブの `on_done.lua` に貼れば AI無しで再生。

## 15. 記録と再生

各手番のAI製Luaは `data/last-rally.lua` に順に積まれる（値の秘密は鍵名で参照されるので
生値は残らない）。これを丸ごと `on_load.lua`/`on_done.lua` の本体に貼れば、AI無しで同じ
ブラウザ操作を再生できる。人間待ち（`browser_ask`/`browser_wait`）も記録されるので、
再生時もログイン待ちで止まる。

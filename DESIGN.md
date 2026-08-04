# ShikishaTerm-AI 設計書

コンセプト: ポータブル・マルチセッションAIオーケストレーションTUI —「高性能AI用のPuTTY」

Claude Code / Codex 等のCLIエージェントと、KIMI / DeepSeek / Ollama 等のLLM APIを、
一つのサイバー風TUIから複数タブで監視・操作・連携させるWindowsポータブルツール。

---

## 1. システム概要

- 複数のAIセッション（CLIエージェント / APIチャット）をタブで並行管理
- タブ間のデータ転送・自動パイプライン・Luaスクリプトによる柔軟な加工
- Google Drive / USBメモリから解凍即起動する完全ポータブル動作
- 映画のハッカー画面風（CRT / ネオン）の可視化UI

## 2. 動作環境・配布形式

| 項目 | 内容 |
|---|---|
| 対応OS | Windows 10 (1809以降) / 11 (64bit) ※ConPTY要件 |
| 配布形態 | 単一実行ファイル `Shikisha-Term-AI.exe` ＋ 設定・スクリプトフォルダ |
| 実行要件 | インストール不要・管理者権限不要・ランタイム依存なし |
| ポータブル要件 | exeからの相対パスのみ使用、データはフォルダ内で完結 |
| 前提 | ラップ対象のCLIエージェント（Claude Code等）は各PCにインストール済みであること（PuTTYにとってのSSHサーバーと同じ位置づけ） |

## 3. 技術スタック（Rust）

| 領域 | 採用技術 | 備考 |
|---|---|---|
| 言語 | Rust | 単一静的リンクexe（約10MB）、依存ゼロ |
| TUI | ratatui + crossterm | |
| ターミナル埋め込み | tui-term + vt100 | TUIペイン内に子プロセス端末を描画 |
| PTY | portable-pty (WezTerm由来) | Windows ConPTY対応 |
| 非同期ランタイム | tokio | |
| スクリプト | mlua (Lua 5.4, vendored静的リンク) | サンドボックス実行 |
| HTTP(API/通知) | reqwest + rustls | OpenSSL非依存 |
| Web GUI | axum | 内蔵ローカルサーバー |
| 設定 | serde / serde_json (config.json) | |
| 暗号化 | argon2 + aes-gcm | APIキー保護 |
| 文字幅 | unicode-width | 全角(日本語)対応 |

選定理由: ターミナルエミュレータを0から書かず、WezTerm/実績クレートを組み込むため。
Go案はエコシステム上、成熟した組み込み用端末エミュレータが無く断念。

## 4. セッションアーキテクチャ

**設計原則: ベンダー非依存。ターミナルで動くAIは全部使えること。**
特定ツールのヘッドレスAPI（stream-json等）には依存しない。

### 4.1 ターミナルタブ（コア）
- portable-pty で任意のCLI AI（claude / codex / gemini / aider / ollama run / 未知の
  新ツール）を起動し、tui-term でタブ内に端末画面をそのまま表示・対話操作
- 状態検出・応答取得・自動操作は4.2の検出エンジンが担う

### 4.2 状態検出エンジン（本システムの心臓部）
複数の独立した信号を重ねて、タブ状態（BUSY / DONE / QUESTION / WAIT / ERROR）を
状態機械で判定する:

| 信号 | 内容 | 信頼度 |
|---|---|---|
| 画面パターン | vt100画面バッファへのプロファイル定義正規表現（例: "esc to interrupt"=BUSY、"❯ 1."等の選択肢リスト=QUESTION） | 高（ルール次第） |
| 端末制御シーケンス | ベル文字(BEL)、ウィンドウタイトル変更(OSC)、代替スクリーン切替、カーソル表示/非表示 | 中 |
| 出力沈黙タイマー | N秒間出力なし＋カーソル入力位置 → 入力待ちとみなす | 中（汎用フォールバック） |
| プロセス終了 | 子プロセスのexitコード | 確実 |

ヘッドレスJSON連携の「100%確実」に対し本方式はヒューリスティックであるため、
誤検知しても事故に至らないよう7.5の自動実行バジェットと「迷ったら止まって
人間に返す」既定（9章）を前提に組む。

### 4.3 エージェントプロファイル（./profiles/*.json）
ツール毎の検出・操作ルールを宣言的に外部定義し、コード変更なしで新ツール対応:

```jsonc
// profiles/claude.json（イメージ）
{
  "name": "Claude Code",
  "launch": "claude {args}",              // 起動コマンドはユーザー編集可
  "busy_patterns":  ["esc to interrupt"],
  "question_patterns": ["Do you want", "❯ \\d+\\."],
  "done_signals":   ["bell", "silence:2s"],
  "answer": { "style": "number_enter" },  // 選択肢への回答方法(数字+Enter/矢印+Enter)
  "capture_cleanup": ["spinner_lines"],   // 応答キャプチャ時の除去ルール
  "detector_lua": null                    // 複雑な判定はLuaに委譲可(detect(screen))
}
```

- 同梱プロファイル: claude / codex / gemini / aider / ollama（ユーザー編集可・Driveで持ち運び可）
- ツールのUI変更で壊れた場合もプロファイル修正のみで復旧
- プロファイル未定義の未知ツールは汎用ヒューリスティック（沈黙タイマー＋BEL＋exit）で動作
- 各ツールの自動許可フラグ（`--full-auto` 等）は `launch` にユーザーが書けばよく、
  アプリ本体はその意味を知る必要がない（ベンダー非依存原則と両立）

### 4.4 ヘッドレスアダプタ（オプションの精度アップグレード）
主要CLIはサブスク認証のまま使える公式ヘッドレスモードを持つ
（Claude Code `claude -p` / Codex `codex exec` / Gemini CLI）。
プロファイルに `headless` 定義があるツールに限り、パイプライン・自動フック用途で
ヘッドレス実行を選択でき、状態検出・応答取得が100%正確になる。

- あくまでオプション。未対応ツールは常にPTY＋検出エンジンで動く（汎用性の原則は不変）
- アプリ本体は各社形式を知らず、パース規則もプロファイル側に記述する
- 注意: Claude Codeは環境変数 `ANTHROPIC_API_KEY` が存在するとサブスク(OAuth)より
  APIキー課金が優先される。子プロセス起動時の環境変数を明示制御して事故を防ぐ

### 4.5 APIチャットタブ（補助）
OpenAI互換API（KIMI / DeepSeek / Ollama serve等）を直接叩くチャットタブも提供する。
CLIを介さないぶん状態検出が正確で、軽量な下請けセッションに向く。

### 4.6 Session抽象化
```
trait Session {
    fn send(&mut self, input: Input);
    fn events(&mut self) -> EventStream;  // StateChanged / Question / Done / Error ...
}
```
実装: `PtySession`（コア） / `HeadlessSession`（オプション） / `ApiChatSession`（補助）

## 5. 画面レイアウト & UI仕様 (Cyber/Hacker Theme)

CRTモニター調ネオンカラー（グリーン/イエロー/ブルー/ブラック）、左側縦タブ構成。

```
┌─────────────────┬────────────────────────────────────────────────────────┐
│ [≡] 0. INDEX    │  [ACTIVE SESSION MAP & HOST CONNECTIVITY]              │
├─────────────────┼────────────────────────────────────────────────────────┤
│ [●] 1. Claude   │  User: タブ1の出力を踏まえてリファクタリングして        │
│ [●] 2. Codex    │  AI  : 了解しました。コード構造を整理します...          │
│ [●] 3. Local-Q4 │                                                        │
│                 │  >>>[RESPONSE COMPLETE]                                │
├─────────────────┴────────────────────────────────────────────────────────┤
│ KERNEL ACCESS GRANTED... PORTABLE_MODE_ON...                    [READY]  │
└──────────────────────────────────────────────────────────────────────────┘
```

### ステータスインジケータ
- `0. INDEX`: 全体ダッシュボード（最上部固定）
- 🟡 黄 BUSY: スピナー（⠋⠙⠹）、AI応答受信中
- 🟢 緑 DONE: 応答受領・処理完了
- 🔵 青 WAIT: 転送待ち / 人間の判断待ち（自動応答が判断保留した場合を含む）
- `A` マーク: オートパイロット（自動YES）有効タブの常時表示

### キー入力ルーティング
ターミナルタブでは打鍵を子プロセスへ透過し、アプリ操作（タブ切替等）は
プレフィックスキー方式（tmux風、例: Ctrl+B）で分離する。

## 6. INDEX（ダッシュボード）

- 全タブの一覧: モデル / 役割 / ステータス / オートパイロット状態
- 接続ホスト疎通状態の可視化（登録API・ローカルLLM）
- 自動実行バジェットの残量表示
- 緊急停止キー: 全タブの自動動作を即時停止
- 設定Web GUI起動（`e` キー）

## 7. パイプライン & 自動フック

### 7.1 手動パイプ（ワンショット）
プロンプト欄の構文で即時転送:
- `@tab2 このログを要約して` → タブ2に送信
- `@tab1 | @tab2` → タブ1に投げ、結果をタブ2へ連鎖

### 7.2 自動フック（常設ルール・イベントドリブン）
config.jsonで「タブNのイベントに `scripts/*.lua` を紐付け」。
タブ完了(DONE)時に加工→転送→自動実行までを無人実行できる。

### 7.3 応答テキストのキャプチャ（パイプラインの入力源）
- ターミナルタブ: 送信時点からのスクロールバック差分を取得 → ANSI制御を除去 →
  プロファイルの `capture_cleanup`（スピナー行除去等）を適用してテキスト化
- ヘッドレスアダプタ有効時: 構造化出力から正確に取得
- APIチャットタブ: APIレスポンスをそのまま使用（正確）

### 7.4 スタートアップ自動化（Expect）
「アプリを起動するだけで前日の作業状態まで自動復旧する」ためのタブ毎の自動入力機能。
例: SSHログイン → 作業フォルダへcd → `claude --resume` → 一番上のセッションを選択。

タブはconfig.jsonで定義し、`startup` に「画面待ち→送信」のステップを並べる:

```jsonc
{
  "tabs": [
    {
      "name": "dev-server",
      "command": "ssh root@example.com",
      "profile": "claude",          // 検出プロファイルの手動指定 (ssh先のAI用)
      "startup": [
        { "wait_for": "\\$ $",            "send": "cd /srv/myproj\r" },
        { "wait_for": "\\$ $",            "send": "claude --resume\r" },
        { "wait_for": "Select a session", "send": "\r" }
      ]
    }
  ]
}
```

より複雑な分岐 (初回だけ--resumeが無い、等) はLuaスタートアップスクリプトで:

```jsonc
{ "startup_lua": "scripts/resume_work.lua" }
```

```lua
function on_start(tab)
  if not shikisha.wait(tab, "\\$ $", 15000) then return end  -- ms。falseならタイムアウト
  shikisha.send(tab, "cd /srv/myproj\r")
  shikisha.wait(tab, "\\$ $", 5000)
  shikisha.send(tab, "claude --resume\r")
  if shikisha.wait(tab, "Select a session", 5000) then
    shikisha.send(tab, "\r")                                 -- 一番上のセッションを選択
  end
end
```

- `wait_for` は検出エンジンの画面バッファに対する正規表現
- 各ステップにタイムアウト (既定10秒)。超過時は自動入力を中断して青WAITで人間に引き渡す
  (誤爆で暴走しない「迷ったら止まる」原則を踏襲)
- 自動入力中はタブに専用インジケータを表示し、任意のキー入力で即座に人間が介入できる
- パスワードの平文埋め込みは非推奨 (SSHは鍵認証を推奨)。どうしても必要な場合は
  10章の暗号化ストアから参照する形とし、config.json平文には書かせない

### 7.5 暴走対策（自動実行バジェット）
自動フック連鎖・自動YES・時間あたりAPI呼び出しを単一バジェットで統合管理:
- パイプライン定義時の循環検出（DAG強制）＋ 実行時最大チェーン深度（既定10）
- 連続自動応答の上限回数（既定10、超過で青WAITに落とし人間へ返す）
- 緊急停止キーで全自動動作を即時停止
- 上限・既定値はconfig.jsonで変更可（自己責任）

## 8. Luaスクリプト（フックエンジン）

フックの唯一の実行エンジンはLua。テンプレートモード（`{{ .TabA.Output }}` 等）は
初心者向けの糖衣であり、内部的にはLua相当に展開される。

### 8.1 サンドボックス（ケーパビリティ注入パターン）
- mluaで `os` / `io` / 生ソケットを一切ロードしない
- メモリ上限・命令数フック・タイムアウトで無限ループを遮断
- Go...Rust側が実装した安全な関数のみを注入。任意URL通信・ファイル操作は不可能

### 8.2 Lua API
| API | 内容 |
|---|---|
| `tab.output` / `tab.model` / `tab.name` | イベント発生タブの情報 |
| `shikisha.send_to_tab(n, text)` | 他タブへ送信＋自動実行 |
| `shikisha.notify(dest, text)` | 登録済みSlack/Telegramへ通知（登録先限定） |
| `shikisha.log(text)` | logs/ への記録 |
| `shikisha.get_var(k)` / `set_var(k, v)` | フック間共有変数 |
| `shikisha.wait(tab, pattern, timeout_ms)` | 画面に正規表現が現れるまで待つ (Expect) |
| `shikisha.send(tab, text)` | タブへキー入力を送信 |
| `shikisha.sleep(ms)` | 待機 |

### 8.3 フックイベント
```lua
function on_start(tab) ... end                   -- タブ起動時 (スタートアップ自動化, 7.4章)
function on_done(tab) ... end                    -- タブ完了時
function on_question(tab, question, options) ... end  -- 選択肢検出時(9章)
function on_error(tab, err) ... end              -- エラー時
```

### 8.4 通知（Slack / Telegram）
- 実体はRust側notifier。config.jsonに登録したSlack Webhook / Telegram Bot APIへのみ送信
- Luaを書かなくても、タブ設定のチェックボックスで「完了時に通知」をON可能
  （内部的に同じnotifierを呼ぶ）

## 9. 自動YES（オートパイロット）

対象: AIが「1: OK, 2: NG」等の選択肢型確認を出してくるケース。

### 検出と応答（ターミナルタブ・汎用）
- 検出エンジンがプロファイルの選択肢パターンでQUESTION状態を判定
- 回答はプロファイルの `answer.style` に従いキー入力を送信
  （数字＋Enter / 矢印キー＋Enter / y＋Enter）
- APIチャットタブでは応答末尾の選択肢パターン（`1:` `2:` / `[Y/n]` 等）を検出し、
  設定定型文（既定「はい、続けてください」）を自動送信

### 回答決定ポリシー
1. シンプルモード: 肯定的選択肢（OK/はい/Yes/続行）が明確な時のみ自動応答。
   判別不能なら青WAITで人間に返す（「迷ったら止まる」が既定）
2. Luaモード: `on_question` で判断を委譲。`nil` 返却で人間へ
   （例: 「削除」「上書き」を含む確認だけ人間に回す等のルールが書ける）

### 補助手段
- 各ツールの許可フラグ（`--full-auto`、`--permission-mode acceptEdits` 等）を
  プロファイルの起動コマンドに書く方法も併用可
  （アプリはフラグの意味を知らない＝ベンダー非依存原則と両立）
- 全モード共通で7.5の自動実行バジェットが適用される

## 10. セキュリティ

### 10.1 APIキー保管
- 標準: マスターパスワード方式（Argon2idで鍵導出 → AES-GCMでキー部分を暗号化）。
  起動時にパスワード入力
- 自己責任: `"encryption": "none"` で平文保存を許可。平文時は起動時に警告表示

### 10.2 Web GUI設定サーバー
- `127.0.0.1` バインド ＋ ランダムポート ＋ 起動毎ワンタイムトークン付きURL
  （`http://127.0.0.1:PORT/?token=...`）
- CSRF / DNS rebinding / 他ローカルプロセスからの設定API叩きを遮断
- localhostバインドのためWindowsファイアウォールのポップアップも回避

### 10.3 GUIで設定可能な項目
- APIキー・接続ホスト(URL)の追加編集
- システムプロンプト・パイプライン（転送先・使用Luaスクリプト）のビジュアル設定
- 通知先（Slack Webhook / Telegram Bot）の登録

## 11. ポータブル動作 & Google Drive競合対策

- 書き込みは「一時ファイル → アトミックリネーム」方式
- ログはセッション毎の追記専用JSONL（`logs/`）— 同期競合してもマージ不要
- 起動時ロックファイルで多重起動検出
- 全パスはexe基準の相対パス、設定・ログ・スクリプトはフォルダ内完結

## 12. マルチバイト（日本語）対応

- unicode-widthによる全角幅計算、罫線レイアウトは日本語入りでテスト
- config / logs / スクリプトは全てUTF-8
- 日本語IME入力はWindows Terminal推奨（素のconhostはIME挙動に癖あり）と明記

## 13. ディレクトリ構成

```
[ShikishaTerm-AI Directory]
 ├── Shikisha-Term-AI.exe   # アプリ本体（単一バイナリ）
 ├── config.json            # ホスト接続・セッション・パイプライン設定（相対パス）
 ├── scripts/               # ユーザー作成のLuaフック
 │     ├── filter_code.lua
 │     └── tabA_to_tabB.lua
 └── logs/                  # 対話履歴バックアップ（セッション毎JSONL）
```

## 14. 開発フェーズ

| Phase | 内容 | 主なリスク検証 |
|---|---|---|
| 1 | PTYスパイク: 単一タブでClaude Codeを表示・対話。日本語幅検証 | tui-term描画品質（本計画の最大リスクを最初に潰す） |
| 2 | 状態検出エンジン＋プロファイル（claude / codex / gemini） | 検出精度、沈黙タイマー調整（第二のリスク） |
| 3 | マルチタブ＋INDEX＋ステータスインジケータ | キー入力ルーティング |
| 4 | パイプライン・応答キャプチャ・Luaフック・通知・自動YES・スタートアップ自動化(Expect) | キャプチャ品質、バジェット制御 |
| 5 | ヘッドレスアダプタ（オプション）・Web GUI・暗号化・仕上げ | 配布と署名 |

## 15. 既知の注意事項

- 未署名exeはSmartScreen / アンチウイルスに誤検知されがち。配布時はコード署名を検討
- ラップ対象CLIのバージョンアップで画面表示・出力形式が変わる可能性があるため、
  検出ルールはプロファイル（外部ファイル）に置き、修正をバイナリ更新から切り離す
- Claude Codeは `ANTHROPIC_API_KEY` が設定されているとサブスク(OAuth)よりAPI課金が
  優先される（高額請求の報告事例あり）。ヘッドレスアダプタ使用時は子プロセスの
  環境変数を明示的に制御する

<p align="center">
  <img src="https://raw.githubusercontent.com/styleio/ShikishaTerm/main/assets/banner.png" alt="SHIKISHA-TERM — 複数のAI CLIを並べて動かし、仕事を渡し合わせる" width="820">
</p>

<p align="center">
  <b>Claude Code / Codex / Gemini を同時に動かし、AI同士に仕事を渡させる。</b><br>
  Windows用のポータブルな単一 <code>.exe</code>。インストール不要・管理者権限不要・APIキー不要。
</p>

<p align="center">
  <a href="LICENSE"><img alt="MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <a href="https://github.com/styleio/ShikishaTerm/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/styleio/ShikishaTerm?include_prereleases"></a>
  <a href="README.md"><img alt="English" src="https://img.shields.io/badge/README-English-blue.svg"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/styleio/ShikishaTerm/main/assets/demo.gif" alt="SHIKISHA-TERM の動作 — 複数AIの並走、AI同士の討論、そしてダウンロードできる結果" width="820">
</p>

---

> **外出先からスマホで、AIの様子を見て「続きをやっといて」と言える。**
> QRコードを読むだけで、全タブの状況確認と指示送信ができます。

## なぜ作ったか

ターミナルのAIを1つ動かすのは簡単です。**4つ**動かすと途端に破綻します。

ウインドウがAIの数だけ増え、どれが自分の返事を待っているのか分からなくなり、
結果を手でコピペして回ることになります。SHIKISHA-TERM は
**処理中 / 完了 / 確認待ち** を区別できるターミナルで、AI同士で仕事を渡すこともできます。

ターミナルで動くものなら何でも扱えるので、特定のベンダーに縛られません。
Claude Code、Codex CLI、Gemini CLI、DeepSeek、Ollama、Aider、SSH越しの素のシェルも同じように扱えます。

## 導入

1. [Releases](https://github.com/styleio/ShikishaTerm/releases/latest) から `SHIKISHA-TERM.zip` を落とす
2. 好きな場所に展開する（USBメモリでも同期フォルダでも動きます）
3. `SHIKISHA-TERM.exe` を起動する

**自前のウィンドウが開きます。** 何かの中で動かす必要はありません。
ターミナルを用意することも、先にフォントや配色を整えることもいりません。

そのフォルダの外には何も書きません。インストーラも追加ランタイムも不要です。

> **手持ちのAIのサブスク認証をそのまま使います。** APIキーは保存しませんし、必要ありません。

### Windowsの警告について

初回起動時に「**WindowsによってPCが保護されました**」が出ます。実行ファイルに
コード署名をしていないためです。この警告をすぐ消せる証明書は年間数万円かかり、
無料のツールに対しては見合わないと判断しています。

続けるには「**詳細情報**」→「**実行**」を押してください。その前に確認したい場合は
——**素性の分からない未署名のexeを警戒するのは正しい判断です**——こちらをどうぞ:

- リリースはすべて[GitHub Actions](.github/workflows/release.yml)がこのソースから
  ビルドしています。誰かの手元のPCで作ったものではなく、**ビルドログは公開されています**
- zipの隣に `SHIKISHA-TERM.zip.sha256` を置いてあります:

  ```powershell
  Get-FileHash SHIKISHA-TERM.zip -Algorithm SHA256
  ```

## 使いはじめ

初回起動時、画面に「`[e]` で設定画面が開きます」と出ます。押すと**この窓の中に**設定画面が開き、
どのAIをどのフォルダで動かすかをそこで決められます。JSONを書く必要はありません。

設定だけを開きたいときは **`Settings.cmd`** をダブルクリックしてください。
設定を壊して本体が起動しなくなったときの復旧にも使えます。

## 主な機能

- **状態が分かるタブ** — 各タブが処理中か、完了か、あなたの返事待ちかを表示します。
  ベンダーのAPIではなく画面そのものから判定するので、どのAIでも動きます
- **画面分割（ペイン）** — 操縦しているブラウザの隣にAIを置く、AI同士を並べる、を
  1つの窓で。各ペインは中身に合わせた寸法になります
- **ワークスペース** — プロジェクトごとにタブ構成を丸ごと切り替え（仮想デスクトップの感覚）。
  自動化スクリプトごと1枚のファイルに書き出して、別のPCや他の人へ渡せます
- **自動化** — 「応答が完了したら検査タブへ渡す」「確認には自動で答える」等。
  Luaで書くか、日本語で指示して手持ちのAIに書いてもらえます
- **暴走対策** — 自動転送の連鎖回数の上限、緊急停止、タブ単位の入力ロック
- **通知** — 作業完了をSlack / Telegramへ
- **スマホから監視・指示** — 外出先から状況確認と指示（後述）
- **本物のターミナル** — SSH・Docker・WSL・踏み台・鍵ファイル・ポート転送・セッションログ・
  文字コード（Shift_JIS等）・日本語入力・マウス操作に対応

## スマホから使う

設定画面の「スマホから使う」を有効にすると、その場にQRコードが出ます。
スマホのカメラで読み取れば、全タブの状況と、指示を送る入力欄が出ます。
本体側では INDEX で `i` を押しても同じQRが出ます。

**繋がる範囲を理解してください** — これはスマホからあなたのPCでコマンドを実行できる機能です。

- 接続できるのは**同じネットワークにいる人だけ**です。インターネットには公開されません
- **[Tailscale](https://tailscale.com/)**（無料）を入れておくと、外出先からでも
  **自分の端末だけ**が暗号化された経路で繋がります。いちばん安全で、推奨はこれです
- Tailscaleが無い場合は**家庭内LANだけ**で繋がります。同じWi-Fiにいる人がURLとトークンを
  知れば操作できるため、共用Wi-Fiや公衆Wi-Fiでは有効にしないでください
- インターネットへ直接公開する設定（`remote.allow_public`）は、設定ファイルに自分で
  書かない限り行いません

詳しい脅威モデルは [SECURITY.md](SECURITY.md) にあります。

## 操作

プレフィックスキーは `Ctrl+B` です（tmux風）。`Ctrl+B ?` でヘルプが開きます。

| キー | 動作 |
|---|---|
| `Ctrl+B q` | 終了 |
| `Ctrl+B 0`〜`9` | タブ切替（0 = INDEX） |
| `Ctrl+B w` / `W` | ワークスペース一覧 / 次へ |
| `Ctrl+B %` / `"` | 画面を横に割る / 下に割る (`<` `>` で幅を変える) |
| `Ctrl+B o` / 矢印 | ペインを移動 |
| `Ctrl+B X` | このペインを閉じる (タブは動いたまま) |
| `Ctrl+B =` | 仕切りを全部半々に戻す |
| `Ctrl+B l` | 入力ロック切替 |
| `Ctrl+B r` | タブの再起動 |
| `Ctrl+B [` | コピーモード（`c` で最新の応答をコピー） |
| `Ctrl+B a` / `x` | 自動化のON/OFF / 緊急停止 |
| `Ctrl+B 0` → `i` | スマホ接続用のQRコードを表示 |
| `Ctrl+B 0` → `k` | マスターパスワードの設定・変更（`secrets.json` を暗号化） |

マウスでも操作できます（ホイールでスクロール、ドラッグでコピー、右クリックで貼り付け、
タブ名クリックで切替）。画面を分割したあとは、各ペインの見出しの ▥ ▤ でさらに割る・✕ で
そのペインを閉じる、**仕切りはドラッグで動かせます**（ダブルクリックで半々に戻ります）。

## 自動化

イベント名を付けたファイルに数行のLuaを置くだけです。

```lua
-- on_done.lua — 完了したら検査タブへ渡し、5往復で打ち切る
if tab.chain_depth == 0 then return end          -- 人間が始めた会話には反応しない

local rounds = shikisha.get_var("rounds") or 0
if tab.output:match("LGTM") or rounds >= 5 then
  shikisha.notify("slack", "レビュー完了（" .. rounds .. "往復）")
  return
end
shikisha.set_var("rounds", rounds + 1)
shikisha.send_to_tab("reviewer", "指摘を修正して:\n" .. tab.output)
```

自動化はサンドボックスで動きます。**既定ではファイル操作もインターネット接続もできません。**
通知先も、設定に登録済みのものにしか送れません。

同じ命令は **アプリの外からも** 呼べます（名前付きパイプ経由・既定はこのアプリが起動した
プロセスのみ）。タブの中のCLIが、人の操作なしに画面を分割したり、ブラウザを操ったり、
別のタブへ仕事を渡したりできます。名前はLuaと同じで、覚え直すものはありません。
詳しくはリファレンスの7章。

詳細は **[docs/AUTOMATION.ja.md](docs/AUTOMATION.ja.md)**（設定画面の「書き方を見る」からも開けます）。

## フォルダ構成

```
SHIKISHA-TERM.exe   アプリ本体
Settings.cmd           設定画面だけを開く
config.json            全体設定 + ワークスペース一覧
secrets.json           通知先やトークン（暗号化可・共有厳禁）
workspaces/            ワークスペース定義（プロジェクト単位で配布できます）
profiles/              AIごとの状態検出ルール
scripts/               自動化スクリプト
logs/                  セッションログ・自動化のログ
lang/                  表示言語のファイル（en.json が基本）
docs/AUTOMATION.md     自動化の書き方
```

`config.example.json` などをコピーして使ってください。

## 表示言語

既定ではWindowsの言語設定に従い、対応する翻訳が無ければ英語になります。
設定画面、または `config.json` の `"language"` でも切り替えられます。

`lang/en.json` をコピーして `lang/<コード>.json` を作れば、それだけで新しい言語が増えます。
訳し漏れたキーは英語で表示されるので、途中まででも動きます
（[docs/TRANSLATING.md](docs/TRANSLATING.md)）。

## 向き・不向き

意図的に守備範囲を絞っています。

- **今のところWindows専用**です。ConPTYの上に作っているため、他OSは未対応です
- **CLIを置き換えるものではなく、動かすものです。** 手持ちのサブスク・ログイン・設定は
  そのままで構いません
- **IDEではなくターミナルです。** セッションを束ね、その間でテキストを動かします

## 貢献

翻訳、未対応のAI CLI用プロファイル、バグ報告、どれも歓迎です。
[CONTRIBUTING.md](CONTRIBUTING.md) からどうぞ（[行動規範](CODE_OF_CONDUCT.md) にご協力ください）。
質問やアイデアは [Discussions](https://github.com/styleio/ShikishaTerm/discussions) へ。

## ビルド

```
cargo build --release
```

Rust（MSVC）が必要です。生成物は単一の実行ファイルで、追加のランタイムは要りません。

## ドキュメント

- [自動化リファレンス](docs/AUTOMATION.ja.md) — イベント・変数・命令・例
- [翻訳の手引き](docs/TRANSLATING.md) — 言語を追加したい方へ
- [設計書](DESIGN.ja.md) — 用語・アーキテクチャ・安全設計（[English](DESIGN.md)）
- [セキュリティ](SECURITY.md) — 脅威モデルと報告方法
- [コード署名ポリシー](SIGNING.md) — リリースの署名方針（英語）

## 応援

SHIKISHA-TERM が役に立ったら、[Ko-fi](https://ko-fi.com/styleio) で開発を応援いただけます。ありがとうございます。

## ライセンス

[MIT](LICENSE)

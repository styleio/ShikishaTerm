import type { LpCopy } from "./types";

const STORE = "https://apps.microsoft.com/detail/9PB8XQVM87Z0";
const REPO = "https://github.com/styleio/ShikishaTerm";

export const ja: LpCopy = {
  lang: "ja",
  meta: {
    title: "SHIKISHA-TERM — AIネイティブ時代のマルチタスク開発ターミナル",
    description:
      "AIネイティブ時代に最適化された、無料の次世代のマルチタスク開発ターミナル。Claude Code、Codex、ローカルLLMなど多彩なAIに対応。PCからもスマホからも操作でき、複数の作業フォルダでエージェントを同時に動かせます。",
    ogImage: "https://shikisha-term.com/og.png",
  },
  nav: {
    links: [
      { label: "手引き", href: "/ja/manual/" },
      { label: "自動化", href: "/ja/automation/" },
      { label: "スマホから", href: "/ja/phone/" },
      { label: "GitHub", href: REPO, external: true },
    ],
    cta: "Store で入手",
    menu: "メニュー",
  },
  hero: {
    badge: "無料・オープンソース・Windows 10/11",
    title: ["複数のAIを、", "1つの画面で指揮する。"],
    lead:
      "AIネイティブ時代のマルチタスク開発ターミナル。Claude Code、Codex、Gemini、ローカルLLMを並行して稼働させ、各AIのステータス（待機中・処理中など）を一目で把握できます。PCからも、スマホからも利用可能。",
    install: {
      label: "コマンドで入れるなら",
      command: "winget install --id 9PB8XQVM87Z0 -s msstore",
      copy: "コピー",
      copied: "コピーしました",
    },
    store: "Microsoft Store から入手",
    fineprint: "Microsoft署名付き。アカウント登録不要、データ収集なし。",
    zip: { label: "ポータブル版（zip）", href: "/ja/get/" },
    github: { label: "ソースを読む(GitHub)", href: REPO, external: true },
    works: {
      label: "動くもの",
      names: [
        { name: "Claude Code", logo: "claudecode" },
        { name: "Codex", logo: "codex" },
        { name: "Gemini CLI", logo: "geminicli" },
        { name: "Aider", logo: "terminal" },
        { name: "OpenAI", logo: "openai" },
        { name: "Anthropic (Claude)", logo: "anthropic" },
        { name: "Google Gemini", logo: "gemini" },
        { name: "DeepSeek", logo: "deepseek" },
        { name: "xAI (Grok)", logo: "xai" },
        { name: "Mistral", logo: "mistral" },
        { name: "Moonshot AI (Kimi)", logo: "moonshot" },
        { name: "Ollama", logo: "ollama" },
        { name: "MiniMax", logo: "minimax" },
        { name: "Groq", logo: "groq" },
        { name: "Cerebras", logo: "cerebras" },
        { name: "OpenRouter", logo: "openrouter" },
        { name: "Together AI", logo: "together" },
        { name: "Hugging Face", logo: "huggingface" },
        { name: "NVIDIA NIM", logo: "nvidia" },
        { name: "LM Studio", logo: "lmstudio" },
        { name: "Jev (TypeSafe)", logo: "choice" },
        { name: "Laya (impossibl)", logo: "choice" },
        { name: "SSH 越しのシェル", logo: "terminal" },
      ],
    },
    image: { src: "/lp/hero.webp", alt: "指揮者が、ノートPCに向かう4体の小さなロボットを指揮しているイラスト" },
  },
  scenes: {
    eyebrow: "どんな開発スタイルにも",
    title: "ローカルでも、クラウドエージェントでも、スマホからでも。",
    items: [
      {
        id: "parallel",
        tab: "複数AIを並列駆動",
        title: "複数のAIを1画面で並行稼働。待機中のAIだけを的確に拾い上げる。",
        body:
          "画面は好きなだけ分割でき、Claude Code・Codex・Gemini・Aider などに、同じリポジトリの別々のタスクを割り当てられます。各タブには「処理中」「完了」「確認待ち」のステータスが常に表示されるため、AIの迷子を探して回る必要はありません。",
        points: [
          "ソフトを閉じても、前回の会話から再開できます",
          "様々なAIエージェントを動かすことが可能です",
          "Gitワークツリーによる複数ブランチの同時並行作業",
          "作業が完了したらSlackなどに通知"
        ],
        image: { src: "/lp/shot-quad.webp", alt: "1つの窓を4分割し、Claude Code・Codex・Gemini・Aider が同時に答えている画面", width: 1760, height: 970 },
      },
      {
        id: "cloud",
        tab: "クラウドエージェント",
        title: "安心してYOLOモード",
        body:
          "クラウド上の MicroVM にプロジェクトを複製し、AIによる安全な並行開発を実現。あなたのPCのスペックを問わず何体でも同時に動かせます。完全に隔離された環境のため、万が一AIが暴走してもPCには一切影響しません。",
        points: [
          "隔離されているから、許可待ちなしで走らせられる",
          "数十体のAIを同時稼働しても私のPCは静か",
          "どのPCでも同じ環境・設定で即起動",
          "URLを発行して、チームと簡単にプレビュー共有"
        ],
        image: { src: "/lp/shot-microvm.ja.webp", alt: "「MicroVM にクローン」の画面。Git の URL、MicroVM、git のアカウント、入れる AI、サーバーの公開範囲を選んでクローンする", width: 558, height: 683 },
      },
      {
        id: "tools",
        tab: "便利な内蔵機能",
        title: "ブラウザも、Gitも、SFTPも。ターミナルを出る理由がない。",
        body:
          "開発に必要な道具は、ターミナルと同じ画面に最初から入っています。開発中のページはブラウザのタブで確かめ、AIに操作させることもできます。Git パネルでは差分の一部だけを選んでコミットでき、SFTP ではサーバーのファイルを左右に並べてやり取りできます。",
        points: [
          "内蔵ブラウザで、AIが自分でデバッグ",
          "差分の一部だけコミット。コミット文はAIが作成",
          "コンフリクトもAIが自動解決",
          "SFTPで、PCとサーバーのファイルを送受信"
        ],
        image: { src: "/lp/shot-browser.ja.webp", alt: "内蔵ブラウザで旅行の予約フォームを開き、AI が項目を入力している画面", width: 1280, height: 900 },
      },
      {
        id: "away",
        tab: "スマホからのリモート指揮",
        title: "移動中に「確認待ち」をチェックし、一言返す。",
        body:
          "QRコードを読み込むだけで、どのタブが何をしているかがスマホからリアルタイムに見え、指示も送れます。デスクに戻る頃には次の処理が完了。Tailscaleを利用してセキュアなプライベート接続で安全にアクセスできます。",
        points: [
          "専用アプリのインストール不要",
          "作業完了をプッシュ通知で",
          "通信経路はエンドツーエンドで暗号化",
          "PC でできる操作は、スマホからもすべて実行可能"
        ],
        image: { src: "/lp/shot-phone.webp", alt: "スマホで見る SHIKISHA-TERM。複数のAIの状態が一覧で並んでいる", width: 533, height: 986, bare: true },
      },
    ],
  },
  problem: {
    eyebrow: "課題",
    title: "4つのAI、4つのウィンドウ。どれが待機中か分からない。",
    body: [
      "ターミナル上で1つのAIを動かすのは簡単ですが、4体を並行して動かそうとすると途端に破綻します。",
      "AIの数だけウィンドウが増え、処理が終わったAIを探すために Alt+Tab を繰り返すことに。結果を手作業でコピペして回るハメになり、10分前に処理を終えたAIが、人間が気づくまで放置されることも珍しくありません。",
    ],
    image: { src: "/lp/problem.webp", alt: "重なった4つの窓に囲まれて、汗をかいている人のイラスト" },
  },
  solution: {
    eyebrow: "解決策",
    title: "ターミナルを読み解き、AIの「今」を教えます。",
    body: [
      "各タブには常に「処理中」「完了」「確認待ち」のステータスが表示されます。判定基準は「ツールが画面に出力したテキスト」のみ。特定のAPIに依存しないため、あらゆるコマンドラインツールで同じように動作します。",
      "明日新しいCLIツールが登場しても、タブの接続先をそれに向けるだけで即使えます。",
    ],
    states: { working: "処理中", done: "完了", waiting: "確認待ち" },
    image: { src: "/lp/solution.webp", alt: "4つに分かれた1つの窓を、コーヒーを片手に落ち着いて眺めている人のイラスト" },
  },
  features: {
    eyebrow: "コア機能",
    title: "本当に手間がかかるのは、AI同士の「受け渡し」でした。",
    image: { src: "/lp/relay.webp", alt: "2体のロボットが書類をリレーのバトンのように渡しているイラスト" },
    items: [
      {
        id: "mention",
        tone: "purple",
        eyebrow: "@メンション",
        title: "チャットツールのように「@」でAI同士をメンション",
        body:
          "入力欄に「@」を入力して対象のタブを指定すれば、AI同士が自律的に仕事を引き継ぎ、返答を読み取って作業を再開します。人間が「コピペの運び屋」になる必要はありません。長時間の処理も、完了時に結果が通知されます。",
        points: ["「@」と入力するだけでタブの候補を一覧表示", "チャットのメンション感覚でAI同士をシームレスに連携", "ブラウザのタブ等にも「@」で操作を引き継げる"],
        image: { src: "/lp/mention.webp", alt: "入力欄に @codex と書いて呼ぶロボットと、チェックマークで応えるもう1体のロボットのイラスト" },
      },
      {
        id: "loop",
        tone: "green",
        eyebrow: "レビューループ",
        title: "指摘がなくなるまで、AI同士で往復",
        body:
          "「この機能を作って @codex にレビューを頼んで、指摘がなくなるまでループして」。この一文で、実装とレビューの往復が回り始めます。往復の回数はアプリが数え、人はいつでも割り込めます。",
        points: ["往復の回数はアプリが管理。上限も自分で決められる", "人はいつでも割り込み・停止できる", "やり取りはすべて、それぞれのAIの会話として残る"],
        image: { src: "/lp/debate.webp", alt: "丸いテーブルを囲む4体のロボットと、笛を持った審判のロボットのイラスト" },
      },
      {
        id: "brake",
        tone: "yellow",
        eyebrow: "安全制御",
        title: "暴走を防ぎ、安全に止まる",
        body:
          "非常停止ボタン、タブ単位の入力ロック、自律実行の連鎖上限など、安全対策も万全。AIの自動化プロセスがファイルシステムやネットワークに触れるのは、あなたが明示的に許可した時だけです。",
        points: ["ワンクリックで全処理を停止できる非常ボタン", "不用意な操作を防ぐタブごとの入力ロック", "許可外のリソースにはアクセスさせない安全設計"],
        image: { src: "/lp/brake.webp", alt: "大きな赤い非常停止ボタンを押す手と、止まったロボットのイラスト" },
      },
      {
        id: "worktree",
        tone: "blue",
        eyebrow: "ワークツリー",
        title: "ブランチごとに、AIの「持ち場」を分離",
        body:
          "コンテキストを見失ったAIは、平気で無関係なファイルを書き換えます。本ツールでは設定画面から Git の worktree を作成可能。ブランチごとに専用の作業空間（フォルダ）を割り当て、競合を防ぎます。",
        points: ["複数のAIが同じ作業コピーを奪い合う事故を防止", "作業履歴はそれぞれのブランチ環境に正確に保持", "SSH, Docker, WSL, IME等に対応する本格ターミナル"],
        image: { src: "/lp/worktree.webp", alt: "3本の枝それぞれに小さな家があり、中でロボットが働いている木のイラスト" },
      },
    ],
  },
  phone: {
    eyebrow: "モバイル連携",
    title: "開発は、机の前だけで終わらない。",
    body: [
      "移動中の電車、カフェ、あるいはベッドの中からでも。QRコードを1回読み込むだけで、各タブの状況がスマホにリアルタイム同期され、そのまま指示を送れます。",
      "Tailscale 経由なら、画面情報はあなた自身の端末にのみ届きます。通信経路はエンドツーエンドで暗号化されており、場所を問わずセキュア。同一 Wi-Fi 環境下であれば、アプリのインストールすら不要です。",
    ],
    image: { src: "/lp/phone.webp", alt: "電車の座席でスマホを見ている人のイラスト" },
    shot: { src: "/lp/shot-phone.webp", alt: "スマホで見る SHIKISHA-TERM。複数のAIの状態が一覧で並び、処理中・完了・確認待ちが分かる" },
    link: { label: "設定のしかた", href: "/ja/phone/" },
  },
  trust: {
    eyebrow: "透明性と信頼",
    title: "ここに書かれたすべては、あなた自身の手で検証可能です。",
    items: [
      {
        title: "Microsoft 署名付きの安心感",
        body: "Microsoft Store 経由での配布のため、煩わしい SmartScreen 警告（「Windows によって PC が保護されました」）は出ません。発行元 (WIRED & ECO, K.K.) は Microsoft によって認証されています。",
      },
      {
        title: "アカウント不要・データ収集なし",
        body: "利用データの収集（テレメトリ）は一切行いません。そもそも、データを送受信するための自社サーバーを持っていません。",
        link: { label: "プライバシーポリシー", href: "/ja/privacy/" },
      },
      {
        title: "完全なオープンソース (MIT)",
        body: "ソースコードはすべて GitHub で公開中。ログイン情報、設定、操作履歴などの機密データは、すべてあなたのローカル環境に留まります。",
        link: { label: "GitHub で読む", href: REPO, external: true },
      },
      {
        title: "クリーンで公開されたビルド",
        body: "すべての実行ファイルは、タグ付きコミットから GitHub Actions 上で自動ビルドされます。改ざん防止のため SHA256 ハッシュも併記しています。",
        link: { label: "リリース一覧", href: `${REPO}/releases`, external: true },
      },
    ],
    stars: {
      label: "GitHub のスター",
      ask: "気に入ったらGitHubのスターを付けてください",
      button: "スターを付ける",
      why: "ZIP版の警告を消すために、力を貸してください。警告を消すために必要な「コード署名」をオープンソース枠で無償取得するには、アプリに一定の評判があることを証明しなければなりません。GitHubのスターは、その最も強力な証明になります。ワンクリックの応援が、次にダウンロードする誰かの手間を減らす助けになります。",
    },
  },
  steps: {
    eyebrow: "クイックスタート",
    title: "導入は3ステップ。あとはウィザードが案内します。",
    items: [
      { title: "入れる", body: "Microsoft Store から入手するか、ポータブル版（ZIP）を好きな場所に展開するだけ。" },
      { title: "使うAIを選ぶ", body: "初回起動で「はじめの設定」が開きます。PCに入っているAIから優先するAIを選ぶだけ。まだ入っていないAIも、その場でインストール方法を開けます。" },
      { title: "プロジェクトを追加", body: "フォルダを選ぶか、Git の URL からクローン。MicroVM や SSH のサーバーにも置けます。あとは案内どおりに押せば、そのフォルダでAIが起動します。" },
    ],
    image: { src: "/lp/shot-setup.ja.webp", alt: "「はじめの設定」の画面。PCに入っているAIから優先するAIを選び、入っていないAIはインストール方法のページを開ける" },
  },
  faq: {
    eyebrow: "よくある質問",
    title: "FAQ",
    items: [
      {
        q: "サブスクのまま利用可能ですか？",
        a: "サブスクのまま利用可能です。PCにインストール済みのAIを、そのままのログイン状態・契約中のサブスクリプションで動かします。この使い方では、本アプリがAPIキーを要求することも、保存することもありません。",
      },
      {
        q: "自分のAPIキーを使うこともできますか？（BYOK）",
        a: "できます。OpenAI互換（またはTypeSafe互換）のエンドポイントを「モデル接続先」として登録すると、タブからそのモデルを直接呼び出せます。利用者が自分の契約とキーを持ち込むこの方式を BYOK（Bring Your Own Key）と呼びます。登録したキーはあなたのPCの中だけに保存され、登録後は画面に表示されません。",
      },
      {
        q: "Claude Code や Codex の代わりになるものですか？",
        a: "いいえ、それらを「束ねて動かす」ためのツールです。各ツールの設定や履歴は元のまま。明日、全く新しいAIツールが登場しても、タブの接続先をそちらに向けるだけで即座に連携できます。",
      },
      {
        q: "Windows 専用ですか？",
        a: "現時点では Windows のみ対応しています。クロスプラットフォーム用の抽象化レイヤーを挟まず、Windows ネイティブの疑似コンソール（ConPTY）を直接叩いているためです。SSH、WSL、IME入力、レガシーな文字コードなどが極めて素直に動くのは、このアーキテクチャのおかげです。",
      },
      {
        q: "利用データはどこかに送信されますか？",
        a: "既定では一切送信しません。アカウント登録も不要で、収集用サーバーも存在しません。通信が発生するのは、あなたが意図して設定した先（各AIのエンドポイント、登録した Webhook、自身のスマートフォンなど）だけです。詳細はプライバシーポリシーに明記しています。",
      },
      {
        q: "これはターミナルですか、それとも IDE ですか？",
        a: "本質的にはターミナルです。Git パネルや簡易ブラウザも内蔵していますが、これは「AIが自律作業するために必要だったから」であり、フル機能の IDE（統合開発環境）を目指しているわけではありません。",
      },
      {
        q: "Store 版とポータブル版、どちらを選ぶべきですか？",
        a: "Store 版は Microsoft のデジタル署名済みのため警告が出ず、自動アップデートにも対応しています。ポータブル版はインストール不要で、GoogleドライブやUSBメモリなど、お好きな場所に展開するだけで使えます。ただし署名がないため、初回起動時に Windows の SmartScreen 警告が出ます。その場合は「詳細情報」→「実行」をクリックして進めてください。",
      },
    ],
  },
  closing: {
    title: "待機中のAIを探し回るのも、人間がコピペの運び屋になるのも、今日で終わりにしよう。",
    store: "Microsoft Store から入手",
    fineprint: "完全無料・Windows 10/11 対応・ポータブル版あり",
    image: { src: "/lp/crowd.webp", alt: "たくさんのロボットと人が肩を並べて手を振っているイラスト" },
  },
  footer: {
    columns: [
      {
        title: "ダウンロード",
        links: [
          { label: "Microsoft Store", href: STORE, external: true },
          { label: "ポータブル版（ZIP）", href: "/ja/get/" },
          { label: "リリース一覧", href: `${REPO}/releases`, external: true },
        ],
      },
      {
        title: "ドキュメント",
        links: [
          { label: "操作ガイド", href: "/ja/manual/" },
          { label: "設定リファレンス", href: "/ja/settings/" },
          { label: "自動化スクリプト", href: "/ja/automation/" },
          { label: "スマホ連携", href: "/ja/phone/" },
        ],
      },
      {
        title: "ポリシー・規約",
        links: [
          { label: "プライバシーポリシー", href: "/ja/privacy/" },
          { label: "MIT ライセンス", href: `${REPO}/blob/main/LICENSE`, external: true },
          { label: "翻訳への参加", href: "/translating/" },
        ],
      },
      {
        title: "コミュニティ",
        links: [
          { label: "GitHub Issues", href: `${REPO}/issues`, external: true },
          { label: "X (Twitter)", href: "https://x.com/SHIKISHATERM", external: true },
        ],
      },
    ],
    note: "SHIKISHA-TERM は WIRED & ECO, K.K. が MIT ライセンスで公開しているオープンソースソフトウェアです。",
  },
};
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
    title: ["複数のAIを、", "1つの窓で指揮する。"],
    lead:
      "AIネイティブ時代のマルチタスク開発ターミナル。Claude Code、Codex、Gemini、ローカルLLMを並べて動かし、どれが待っているかを画面が教えます。PCからも、スマホからも。",
    install: {
      label: "コマンドで入れるなら",
      command: "winget install --id 9PB8XQVM87Z0 -s msstore",
      copy: "コピー",
      copied: "コピーしました",
    },
    store: "Microsoft Store から入手",
    fineprint: "Microsoft の署名付き。アカウント不要、計測なし。",
    zip: { label: "ポータブル版（zip）", href: "/ja/get/" },
    github: { label: "ソースを読む", href: REPO, external: true },
    works: {
      label: "動くもの",
      names: ["Claude Code", "Codex", "Gemini CLI", "Aider", "Ollama", "DeepSeek", "Qwen", "SSH 越しのシェル"],
    },
    image: { src: "/lp/hero.webp", alt: "指揮者が、ノートPCに向かう4体の小さなロボットを指揮しているイラスト" },
  },
  scenes: {
    eyebrow: "どんな開発スタイルでも",
    title: "一人でも、4体でも、外からでも。",
    items: [
      {
        id: "parallel",
        tab: "4体を並べて進める",
        title: "1つの窓に4体。待っているのだけ見に行く。",
        body:
          "1つの窓を4つに分けて、Claude Code・Codex・Gemini・Aider に同じリポジトリの別の仕事をさせます。どのタブにも「処理中」「完了」「確認待ち」のどれかが出るので、探して回る必要がありません。",
        points: ["タブごとに別のAI、別のフォルダ", "Ctrl+B 0 で4体の一覧に切り替え", "負荷と、どれだけ黙っているかも並ぶ"],
        image: { src: "/lp/shot-quad.webp", alt: "1つの窓を4分割し、Claude Code・Codex・Gemini・Aider が同時に答えている画面", width: 1760, height: 970 },
      },
      {
        id: "review",
        tab: "書かせて、レビューさせる",
        title: "書く役とレビュー役を、通るまで往復させる。",
        body:
          "「これが終わったら結果をレビュー用のタブへ」。一度そう決めておけば、受け渡しは自動で起きます。決め方は Lua で書いても、普通の言葉で説明してタブの中のAIに書かせても構いません。",
        points: ["終わったら Slack に知らせる", "何回まで渡すかの上限は自分で決める", "やり取りは丸ごと読める記録に残る"],
        image: { src: "/lp/shot-desktop.webp", alt: "複数のAIを一覧に並べ、各行が処理中・完了・確認待ちのどれかを示している画面", width: 1600, height: 687 },
      },
      {
        id: "git",
        tab: "git は同じ窓の中で",
        title: "差分の一部だけステージして、判断だけ人がする。",
        body:
          "git パネルは端末と同じ場所に開きます。ステージはファイル単位ではなく、差分の一部だけを選べます。コミットメッセージは自分で書いても、AIに書かせても構いません。コンフリクトはAIに解きほぐさせます。",
        points: ["同じパネルの裏側は履歴", "ブランチごとに専用の作業フォルダ", "フォルダが無いタブは、無いと言って止まる"],
        image: { src: "/lp/shot-git.webp", alt: "git パネル。ブランチ、次のコミットに入るファイル、一部だけ外せる差分、コミットメッセージを書かせるボタン", width: 1600, height: 657 },
      },
      {
        id: "away",
        tab: "外出先から返事をする",
        title: "電車の中で「確認待ち」に気づいて、一言返す。",
        body:
          "QRコードを1枚読むだけで、どのタブが何をしているかがスマホに見えて、指示も送れます。机に戻る頃には次の分が終わっています。Tailscale 越しなら、画面が届く先は自分の端末だけです。",
        points: ["同じ Wi-Fi の中なら、入れるものはありません", "経路は暗号化されたまま", "PC でできる操作は、スマホからも全部できる"],
        image: { src: "/lp/shot-phone.webp", alt: "スマホで見る SHIKISHA-TERM。複数のAIの状態が一覧で並んでいる", width: 533, height: 986 },
      },
    ],
  },
  problem: {
    eyebrow: "困りごと",
    title: "AIが4体、窓が4つ。待っているのはどれか分からない。",
    body: [
      "ターミナルのAIを1体動かすのは簡単です。4体になると途端に破綻します。",
      "窓はAIの数だけ増えます。止まっている1つを探して Alt+Tab を繰り返し、出てきた結果は手でコピペして回るはめになります。10分前に答え終えたAIは、こちらが画面を見に行くまで、そのまま放置されます。",
    ],
    image: { src: "/lp/problem.webp", alt: "重なった4つの窓に囲まれて、汗をかいている人のイラスト" },
  },
  solution: {
    eyebrow: "解決",
    title: "画面を読んで、教えます。",
    body: [
      "どのタブにも「処理中」「完了」「確認待ち」のどれかが出ます。判定に使うのは、そのツールが実際に画面へ出した文字だけです。特定の会社のAPIに頼らないので、どのコマンドラインツールでも同じように動きます。",
      "明日新しいCLIが出たら、タブをそれに向ければ済みます。",
    ],
    states: { working: "処理中", done: "完了", waiting: "確認待ち" },
    image: { src: "/lp/solution.webp", alt: "4つに分かれた1つの窓を、コーヒーを片手に落ち着いて眺めている人のイラスト" },
  },
  features: {
    eyebrow: "できること",
    title: "本当に手が取られるのは、AI同士の受け渡し。",
    items: [
      {
        id: "relay",
        tone: "purple",
        eyebrow: "受け渡し",
        title: "AI同士で仕事を渡す",
        body:
          "どれが待っているか分かっても、まだ半分です。あとの半分は、自分が運び屋をやめること。書く役とレビュー役を、通るまで往復させます。終わったら Slack に知らせます。",
        points: ["Lua で書いても、言葉で頼んでも", "何回まで渡し続けるかの上限", "記録は丸ごと読める"],
        image: { src: "/lp/relay.webp", alt: "2体のロボットが書類をリレーのバトンのように渡しているイラスト" },
      },
      {
        id: "debate",
        tone: "green",
        eyebrow: "議論",
        title: "同じ問いを、4体に投げる",
        body:
          "Claude・Codex・Gemini・DeepSeek に同じ問いについて議論させ、審判役にまとめさせます。人はいつでも割り込めて、いつでも止められます。",
        points: ["異なるAI同士の往復", "審判役がまとめる", "人の割り込みと停止"],
        image: { src: "/lp/debate.webp", alt: "丸いテーブルを囲む4体のロボットと、笛を持った審判のロボットのイラスト" },
      },
      {
        id: "brake",
        tone: "yellow",
        eyebrow: "ブレーキ",
        title: "ちゃんと止まる",
        body:
          "非常停止、タブ単位の入力ロック、連鎖の上限。自動化がファイルやネットワークに触れるのは、許可したときだけです。",
        points: ["非常停止は1つのボタン", "タブごとの入力ロック", "許可していない場所には触れない"],
        image: { src: "/lp/brake.webp", alt: "大きな赤い非常停止ボタンを押す手と、止まったロボットのイラスト" },
      },
      {
        id: "worktree",
        tone: "blue",
        eyebrow: "持ち場",
        title: "ブランチごとに、AIの持ち場を分ける",
        body:
          "自分がどこにいるか分かっていないAIは、平気で別のプロジェクトを書き換えます。タブは作業フォルダごとにまとまり、設定画面から git の worktree を作れるので、ブランチに専用のフォルダを持たせられます。",
        points: ["2体が1つの作業コピーを取り合わない", "履歴は作業した場所に残る", "本物のターミナル。SSH、Docker、WSL、IME、古い文字コード"],
        image: { src: "/lp/worktree.webp", alt: "3本の枝それぞれに小さな家があり、中でロボットが働いている木のイラスト" },
      },
    ],
  },
  phone: {
    eyebrow: "スマホから",
    title: "いつも机の前にいるわけではない。",
    body: [
      "電車の中でも、カフェでも、寝床からでも、どのタブが何をしているかが見えて、指示も送れます。QRコードを1枚読むだけです。",
      "Tailscale 越しなら、画面が届く先は自分の端末だけです。経路は暗号化されたままで、どこにいても使えます。同じ Wi-Fi の中だけなら、インストールするものはありません。",
    ],
    image: { src: "/lp/phone.webp", alt: "電車の座席でスマホを見ている人のイラスト" },
    shot: { src: "/lp/shot-phone.webp", alt: "スマホで見る SHIKISHA-TERM。複数のAIの状態が一覧で並び、処理中・完了・確認待ちが分かる" },
    link: { label: "設定のしかた", href: "/ja/phone/" },
  },
  trust: {
    eyebrow: "確かめられること",
    title: "書いてあることは、全部その場で確かめられます。",
    items: [
      {
        title: "Microsoft の署名付き",
        body: "Store で配布しているので「WindowsによってPCが保護されました」は出ません。発行元は WIRED & ECO, K.K. で、Microsoft が確認しています。",
      },
      {
        title: "アカウント不要・計測なし",
        body: "利用状況は何も集めません。送り先になるサーバーも、こちらにはありません。",
        link: { label: "プライバシーポリシー", href: "/ja/privacy/" },
      },
      {
        title: "MIT・全部読めます",
        body: "ソースは GitHub にあります。ログインも設定も履歴も、使っているCLIの場所にそのまま残ります。",
        link: { label: "GitHub で読む", href: REPO, external: true },
      },
      {
        title: "公開の場でビルド",
        body: "配布物はタグ付きコミットから GitHub Actions が作ります。SHA256 も並べて公開しています。",
        link: { label: "リリース一覧", href: `${REPO}/releases`, external: true },
      },
    ],
    stars: {
      label: "GitHub のスター",
      ask: "この道具が役に立つなら、GitHub でスターを付けてください。",
      button: "スターを付ける",
      why: "zip の警告を消すコード署名の証明書は、オープンソース向けに無料で発行されますが、「検証できる一定の評判」が条件です。GitHub でいちばん見える形がスターです。通りすがりの人でも1秒でできて、次に zip を落とす人のためにもなります。",
    },
  },
  steps: {
    eyebrow: "はじめかた",
    title: "3手で動きます。JSON は書きません。",
    items: [
      { title: "入れる", body: "Microsoft Store から入れます。ポータブル版を好きな場所に展開しても構いません。USBメモリでも動きます。" },
      { title: "起動する", body: "端末のタブが最初から1つ開いています。まずはいつものシェルとして使えます。" },
      { title: "[e] を押す", body: "設定画面はこのアプリの中に開きます。どのAIをどのフォルダで動かすかを、フォームで選びます。" },
    ],
    image: { src: "/lp/shot-settings.webp", alt: "設定画面。タブごとの名前・コマンドや SSH 接続先・作業フォルダ・自動化をフォームで設定できる" },
  },
  faq: {
    eyebrow: "最初によく聞かれること",
    title: "よくある質問",
    items: [
      {
        q: "APIキーは要りますか？",
        a: "要りません。すでにインストールしてログインしてあるコマンドラインツールを、契約中のサブスクのまま動かします。キーは保存もしませんし、要求もしません。",
      },
      {
        q: "Claude Code や Codex の代わりになるものですか？",
        a: "いいえ、それらを動かす側です。ログインも設定も履歴もそのままの場所に残ります。明日新しいCLIが出たら、タブをそれに向ければ済みます。",
      },
      {
        q: "Windows だけですか？",
        a: "いまのところ Windows だけです。土台にしているのは Windows の疑似コンソール（ConPTY）そのもので、複数のOSに共通の薄い層はかぶせていません。SSH・WSL・IME入力・古い文字コードが Windows で素直に動くのは、そのためでもあります。",
      },
      {
        q: "どこかに何か送りますか？",
        a: "既定では何も送りません。アカウントもなく、こちらのサーバーもありません。通信が起きるのは自分で設定したときだけで、相手も自分で選んだ先だけです。キーを入れたAIの提供元、登録した Webhook、あるいは自分のスマホ。全部プライバシーポリシーに並べてあります。",
      },
      {
        q: "ターミナルですか、IDEですか？",
        a: "ターミナルです。git パネルもブラウザも付いていますが、AIが必要とするから付けただけで、エディタになろうとしているわけではありません。",
      },
      {
        q: "Store 版とポータブル版、どちらを選べば？",
        a: "Store 版は Microsoft が署名しているので警告が出ず、更新も自動です。ポータブル版は同じビルドを、何もインストールせずに使うものです。zip を展開して起動し、フォルダごと消せば跡が残りません。ただし署名が無いぶん、Windows が初回に一度だけ「WindowsによってPCが保護されました」と警告します。これが出るのが正常で、「詳細情報」→「実行」で進めてください。",
      },
    ],
  },
  closing: {
    title: "どれが待っているかを探すのも、AIとAIの間を往復するのも、もうやめる。",
    store: "Microsoft Store から入手",
    fineprint: "無料・Windows 10/11・ポータブル版もあります",
    image: { src: "/lp/crowd.webp", alt: "たくさんのロボットと人が肩を並べて手を振っているイラスト" },
  },
  footer: {
    columns: [
      {
        title: "手に入れる",
        links: [
          { label: "Microsoft Store", href: STORE, external: true },
          { label: "ポータブル版（zip）", href: "/ja/get/" },
          { label: "リリース一覧", href: `${REPO}/releases`, external: true },
        ],
      },
      {
        title: "読む",
        links: [
          { label: "手引き", href: "/ja/manual/" },
          { label: "設定の事典", href: "/ja/settings/" },
          { label: "自動化", href: "/ja/automation/" },
          { label: "スマホから使う", href: "/ja/phone/" },
        ],
      },
      {
        title: "決まりごと",
        links: [
          { label: "プライバシーポリシー", href: "/ja/privacy/" },
          { label: "MIT ライセンス", href: `${REPO}/blob/main/LICENSE`, external: true },
          { label: "翻訳に参加する", href: "/translating/" },
        ],
      },
      {
        title: "話す",
        links: [
          { label: "GitHub Issues", href: `${REPO}/issues`, external: true },
          { label: "X", href: "https://x.com/SHIKISHATERM", external: true },
        ],
      },
    ],
    note: "SHIKISHA-TERM は WIRED & ECO, K.K. が MIT ライセンスで公開しているオープンソースソフトウェアです。",
  },
};

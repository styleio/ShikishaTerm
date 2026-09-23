import type { LpCopy } from "./types";

const STORE = "https://apps.microsoft.com/detail/9PB8XQVM87Z0";
const REPO = "https://github.com/styleio/ShikishaTerm";

export const en: LpCopy = {
  lang: "en",
  meta: {
    title: "SHIKISHA-TERM — The multitasking terminal for AI-native development",
    description:
      "The free, next-generation multitasking terminal built for AI-native development. Works with Claude Code, Codex, local LLMs and more. Drive it from your PC or your phone, and run agents in several working folders at once.",
    ogImage: "https://shikisha-term.com/og.png",
  },
  nav: {
    links: [
      { label: "Manual", href: "/manual/" },
      { label: "Automation", href: "/automation/" },
      { label: "From your phone", href: "/phone/" },
      { label: "GitHub", href: REPO, external: true },
    ],
    cta: "Get it on the Store",
    menu: "Menu",
  },
  hero: {
    badge: "Free · open source · Windows 10/11",
    title: ["Conduct all your AIs", "from one window."],
    lead:
      "The multitasking terminal for AI-native development. Run Claude Code, Codex, Gemini and local LLMs side by side, and let the screen tell you which one is waiting for you. From your PC, or from your phone.",
    install: {
      label: "Or from the command line",
      command: "winget install --id 9PB8XQVM87Z0 -s msstore",
      copy: "Copy",
      copied: "Copied",
    },
    store: "Get it from the Microsoft Store",
    fineprint: "Signed by Microsoft. No account, no telemetry.",
    zip: { label: "Portable zip", href: "/get/" },
    github: { label: "Read the source", href: REPO, external: true },
    works: {
      label: "Works with",
      names: ["Claude Code", "Codex", "Gemini CLI", "Aider", "Ollama", "DeepSeek", "Qwen", "Any shell over SSH"],
    },
    image: { src: "/lp/hero.webp", alt: "A conductor with a baton directing four small robots, each at a laptop" },
  },
  scenes: {
    eyebrow: "For every way of working",
    title: "Alone, with four agents, or from the train.",
    items: [
      {
        id: "parallel",
        tab: "Run four at once",
        title: "Four agents in one window. Look only at the one that is waiting.",
        body:
          "Split one window four ways and give Claude Code, Codex, Gemini and Aider different jobs on the same repository. Every tab says whether it is working, done, or waiting for you, so there is nothing to go looking for.",
        points: ["A different AI and a different folder per tab", "Ctrl+B 0 turns the four into a list", "What each one costs the machine, and how long it has been quiet"],
        image: { src: "/lp/shot-quad.webp", alt: "Claude Code, Codex, Gemini and Aider running side by side in four panes of one window, each answering a question about the same repository", width: 1760, height: 970 },
      },
      {
        id: "review",
        tab: "Write, then review",
        title: "A writer and a reviewer, looping until it passes.",
        body:
          "Say it once: when this one finishes, send the result to the review tab. From then on the hand-off happens without you. Write the rule in Lua, or describe it in plain language and let an AI you already have in a tab write it.",
        points: ["Messages you on Slack when it is done", "You set how many times work may pass along", "The whole exchange is a transcript you can read"],
        image: { src: "/lp/shot-desktop.webp", alt: "Several AI agents in one window, each row showing whether it is working, done, or waiting", width: 1600, height: 687 },
      },
      {
        id: "git",
        tab: "git in the same window",
        title: "Stage a piece of a file. Keep the decisions for yourself.",
        body:
          "The git panel opens where a terminal would. You stage a piece of a diff, not the whole file. Write the commit message yourself or have it written for you, and let an AI untangle a merge while you decide.",
        points: ["The history is on the back of the same panel", "A folder of its own for every branch", "A tab whose folder is missing says so and stops"],
        image: { src: "/lp/shot-git.webp", alt: "The git panel: branches, files staged for the next commit, a diff with a hunk that can be dropped, and a button that writes the commit message", width: 1600, height: 657 },
      },
      {
        id: "away",
        tab: "Answer from the train",
        title: "See “waiting for you” on the train, and answer in one line.",
        body:
          "Scan one QR code and your phone shows what every tab is doing and lets you send instructions. By the time you are back at the desk, the next piece is done. Over Tailscale, only your own devices can reach it.",
        points: ["On your own Wi-Fi there is nothing to install", "Encrypted all the way", "Everything you can do at the PC, you can do from the phone"],
        image: { src: "/lp/shot-phone.webp", alt: "SHIKISHA-TERM on a phone: a live list of several agents and their states", width: 533, height: 986 },
      },
    ],
  },
  problem: {
    eyebrow: "The problem",
    title: "Four agents. Four windows. No idea who is waiting.",
    body: [
      "Running one terminal AI is easy. Running four is not.",
      "You end up with a window per agent, a lot of alt-tabbing to find the one that stopped, and a lot of copy-pasting between them. The agent that finished ten minutes ago sits there finished, and you find out when you happen to look.",
    ],
    image: { src: "/lp/problem.webp", alt: "A person sweating in the middle of four overlapping windows" },
  },
  solution: {
    eyebrow: "The fix",
    title: "It reads the screen and tells you.",
    body: [
      "Every tab says whether it is working, done, or waiting for you. The judgement comes from what the tool actually printed, not from one vendor’s API, so it works the same with any command-line tool.",
      "If a new CLI comes out tomorrow, point a tab at it.",
    ],
    states: { working: "Working", done: "Done", waiting: "Waiting for you" },
    image: { src: "/lp/solution.webp", alt: "A person with a cup of coffee, calmly watching one window split into four" },
  },
  features: {
    eyebrow: "What it does",
    title: "The hand-off is the work.",
    items: [
      {
        id: "relay",
        tone: "purple",
        eyebrow: "Hand-off",
        title: "Agents that pass work to each other",
        body:
          "Knowing who is waiting is half of it. The other half is not being the courier. A code-and-review loop runs until it passes, then messages you on Slack.",
        points: ["Write it in Lua, or just ask for it", "A cap on how many times work may pass along", "Every exchange kept as a readable transcript"],
        image: { src: "/lp/relay.webp", alt: "Two robots passing a document between them like a relay baton" },
      },
      {
        id: "debate",
        tone: "green",
        eyebrow: "Debate",
        title: "Put the same question to four of them",
        body:
          "Have Claude, Codex, Gemini and DeepSeek debate a question, with a judge to sum it up. You can cut in at any point, and stop it at any point.",
        points: ["Different AIs, going back and forth", "A judge that writes the summary", "Room for you to interrupt, and to stop"],
        image: { src: "/lp/debate.webp", alt: "Four robots around a round table and a referee robot with a whistle" },
      },
      {
        id: "brake",
        tone: "yellow",
        eyebrow: "Brakes",
        title: "Brakes that hold",
        body:
          "An emergency stop, per-tab input locks, and a cap on chained hand-offs. Automation touches files and the network only where you granted it.",
        points: ["The emergency stop is one button", "An input lock on any tab", "No access anywhere you did not grant it"],
        image: { src: "/lp/brake.webp", alt: "A hand on a big red emergency stop button and a robot frozen mid-step" },
      },
      {
        id: "worktree",
        tone: "blue",
        eyebrow: "Its own folder",
        title: "A branch per agent, a folder per branch",
        body:
          "An agent that does not know where it is will happily edit the wrong project. Tabs are grouped by working folder, and the settings screen cuts a git worktree so that a branch gets a folder of its own.",
        points: ["Two agents stop editing one checkout out from under each other", "Session history stays where the work was", "A real terminal: SSH, Docker, WSL, IME input, legacy encodings"],
        image: { src: "/lp/worktree.webp", alt: "A tree with three branches, a small house on each with a robot working inside" },
      },
    ],
  },
  phone: {
    eyebrow: "From your phone",
    title: "You are not always at the desk.",
    body: [
      "From the train, from a café, from bed: you can see what every tab is doing and send instructions. It takes one QR code.",
      "Over Tailscale, only your own devices can reach it, encrypted, from anywhere. On your own Wi-Fi there is nothing to install.",
    ],
    image: { src: "/lp/phone.webp", alt: "A person on a train seat looking at their phone" },
    shot: { src: "/lp/shot-phone.webp", alt: "SHIKISHA-TERM on a phone: a live list of several agents, each showing whether it is working, done, or waiting for you" },
    link: { label: "How to set it up", href: "/phone/" },
  },
  trust: {
    eyebrow: "Things you can check",
    title: "Everything on this page can be verified on the spot.",
    items: [
      {
        title: "Signed by Microsoft",
        body: "Published in the Store, so no “Windows protected your PC”. Publisher identity verified: WIRED & ECO, K.K.",
      },
      {
        title: "No account. No telemetry.",
        body: "Nothing is collected, and there is no server of ours to send it to.",
        link: { label: "Read the privacy policy", href: "/privacy/" },
      },
      {
        title: "MIT, and readable",
        body: "Every line is on GitHub. Your logins, settings and history stay where your CLIs keep them.",
        link: { label: "Read it on GitHub", href: REPO, external: true },
      },
      {
        title: "Built in public",
        body: "Each release is built by GitHub Actions from a tagged commit, with a SHA256 beside it.",
        link: { label: "Releases", href: `${REPO}/releases`, external: true },
      },
    ],
    stars: {
      label: "stars on GitHub",
      ask: "If you like it, give it a star on GitHub",
      button: "Star it",
      why: "Help us remove the warning on the ZIP file. To get a free open-source code signing certificate that silences it, we need to prove the app has a solid reputation. GitHub stars are the best way to show this. Your one-click support will save the next user the hassle.",
    },
  },
  steps: {
    eyebrow: "Getting started",
    title: "Three steps. No JSON.",
    items: [
      { title: "Install it", body: "From the Microsoft Store, or unzip the portable copy anywhere you like. A USB stick works." },
      { title: "Open it", body: "A terminal tab is already there. Use it as your everyday shell to begin with." },
      { title: "[e] to configure", body: "The settings screen opens inside the window. Pick which AI runs in which folder, in a form." },
    ],
    image: { src: "/lp/shot-settings.webp", alt: "The settings screen: each tab’s name, command or SSH host, working folder and automation, set in a form" },
  },
  faq: {
    eyebrow: "Questions people ask first",
    title: "Frequently asked",
    items: [
      {
        q: "Can I keep using the subscriptions I already pay for?",
        a: "Yes. It drives the AI tools already installed on your PC, signed in as they are, on the subscriptions you already pay for. Used that way, it asks for no API key and stores none.",
      },
      {
        q: "Can I bring my own API key? (BYOK)",
        a: "Yes. Register an OpenAI-compatible (or TypeSafe-compatible) endpoint as a model connection, and a tab can call that model directly. Bringing your own contract and your own key is what BYOK — bring your own key — means. The key you register is kept on your own PC, and it is never shown again once it is registered.",
      },
      {
        q: "Does it replace Claude Code or Codex?",
        a: "No. It runs them. Your logins, settings and history stay exactly where they are. If a new CLI comes out tomorrow, point a tab at it.",
      },
      {
        q: "Windows only?",
        a: "Today, yes. It is built on ConPTY, the Windows pseudo-console, rather than on a cross-platform shim, which is also why SSH, WSL, IME input and legacy encodings behave the way they do on Windows.",
      },
      {
        q: "What does it send anywhere?",
        a: "By default, nothing. There is no account and no server of ours. Connections happen only where you set them up, and go straight to the party you chose: the AI provider whose key you entered, a webhook you registered, or your own phone. The privacy policy lists every one of them.",
      },
      {
        q: "Is it a terminal or an IDE?",
        a: "A terminal. It has a git panel and a browser because agents need them, but it is not trying to become your editor.",
      },
      {
        q: "Store copy or portable zip?",
        a: "The Store copy is signed by Microsoft, shows no warning and updates itself. The zip is the same build with nothing installed: unzip it, run it, delete the folder to remove it. Because it is not code-signed, Windows says “Windows protected your PC” the first time. That is expected; More info → Run anyway gets past it.",
      },
    ],
  },
  closing: {
    title: "Stop hunting for the one that is waiting. Stop being the courier between them.",
    store: "Get it from the Microsoft Store",
    fineprint: "Free · Windows 10/11 · portable zip available",
    image: { src: "/lp/crowd.webp", alt: "A crowd of robots and people standing shoulder to shoulder, waving" },
  },
  footer: {
    columns: [
      {
        title: "Get it",
        links: [
          { label: "Microsoft Store", href: STORE, external: true },
          { label: "Portable zip", href: "/get/" },
          { label: "Releases", href: `${REPO}/releases`, external: true },
        ],
      },
      {
        title: "Read",
        links: [
          { label: "Manual", href: "/manual/" },
          { label: "Settings reference", href: "/settings/" },
          { label: "Automation", href: "/automation/" },
          { label: "From your phone", href: "/phone/" },
        ],
      },
      {
        title: "The rules",
        links: [
          { label: "Privacy policy", href: "/privacy/" },
          { label: "MIT license", href: `${REPO}/blob/main/LICENSE`, external: true },
          { label: "Help translate", href: "/translating/" },
        ],
      },
      {
        title: "Talk",
        links: [
          { label: "GitHub Issues", href: `${REPO}/issues`, external: true },
          { label: "X", href: "https://x.com/SHIKISHATERM", external: true },
        ],
      },
    ],
    note: "SHIKISHA-TERM is open-source software published by WIRED & ECO, K.K. under the MIT license.",
  },
};

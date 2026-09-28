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
        { name: "Any shell over SSH", logo: "terminal" },
      ],
    },
    image: { src: "/lp/hero.webp", alt: "A conductor with a baton directing four small robots, each at a laptop" },
  },
  scenes: {
    eyebrow: "For any workflow",
    title: "On your PC, as cloud agents, or from your phone.",
    items: [
      {
        id: "parallel",
        tab: "Run many at once",
        title: "Multiple agents in one window. Instantly spot the ones waiting for you.",
        body:
          "Split the screen as many ways as you need and assign Claude Code, Codex, Gemini, Aider, and others to different tasks on the same repository. Every tab shows whether the agent is working, done, or waiting for input, so nothing gets lost.",
        points: [
          "Close the app and pick up exactly where you left off",
          "Works with the AI agents you already use",
          "Git worktrees: work on multiple branches simultaneously",
          "Get Slack notifications when tasks complete"
        ],
        image: { src: "/lp/shot-quad.webp", alt: "Claude Code, Codex, Gemini and Aider running side by side in four panes of one window, each answering a question about the same repository", width: 1760, height: 970 },
      },
      {
        id: "cloud",
        tab: "Cloud agents",
        title: "YOLO mode, without the worry.",
        body:
          "Clone your project into a cloud MicroVM and let your agents work side by side in a secure environment. No matter your PC's specs, you can run as many as you want. Each instance is fully isolated, so an agent that goes off the rails will never touch your local machine.",
        points: [
          "Fully isolated for permission-free execution",
          "Run dozens of agents at once with zero local load",
          "Consistent environments ready on any PC",
          "Share a preview with your team via URL"
        ],
        image: { src: "/lp/shot-microvm.en.webp", alt: "The Clone onto a MicroVM dialog: a Git URL, the MicroVM, the account for the git server, the AI to install, and who can open what the machine serves", width: 558, height: 683 },
      },
      {
        id: "tools",
        tab: "Built in",
        title: "Browser, Git, and SFTP built right in. No need to leave the terminal.",
        body:
          "Everything you need to build is right inside the terminal. Check your work in a browser tab and let the AI drive it. Stage partial diffs in the Git panel, and transfer files seamlessly to a server over SFTP.",
        points: [
          "AI debugs autonomously in the built-in browser",
          "Stage partial diffs and let AI write the commit message",
          "AI-assisted merge conflict resolution",
          "Transfer files seamlessly via SFTP"
        ],
        image: { src: "/lp/shot-browser.en.webp", alt: "A travel booking form open in the built-in browser, with an AI filling in the fields", width: 1280, height: 900 },
      },
      {
        id: "away",
        tab: "Answer from the train",
        title: "See who's “waiting for you” on the go, and answer in one line.",
        body:
          "Scan a QR code to see live tab activity and send instructions right from your phone. By the time you return to your desk, the next task is done. Connections are fully private and secure over Tailscale.",
        points: [
          "No dedicated app to install",
          "Push notifications upon task completion",
          "End-to-end encrypted",
          "Full terminal control from your mobile device"
        ],
        image: { src: "/lp/shot-phone.webp", alt: "SHIKISHA-TERM on a phone: a live list of several agents and their states", width: 533, height: 986, bare: true },
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
    image: { src: "/lp/relay.webp", alt: "Two robots passing a document between them like a relay baton" },
    items: [
      {
        id: "mention",
        tone: "purple",
        eyebrow: "@mention",
        title: "@mention one agent from another, like in a chat app",
        body:
          "Type @ in the input bar and pick a tab: the agent in front hands the work to that one, reads its answer and carries on. No more being the copy-paste courier. A long job sends its answer back when it is done.",
        points: ["Type @ to see the list of tabs", "Agents team up the way people do in a chat", "Hand work to a browser tab and more with @"],
        image: { src: "/lp/mention.webp", alt: "A robot calling @codex in a chat input box, and another robot answering with a check mark" },
      },
      {
        id: "loop",
        tone: "green",
        eyebrow: "Review loop",
        title: "Back and forth until the review comes back clean",
        body:
          "“Build this feature, ask @codex to review it, and loop until there is nothing left to fix.” That one line starts the write-and-review loop. The app counts the rounds, and you can step in at any time.",
        points: ["The app counts the rounds, up to a limit you set", "Step in or stop it whenever you like", "Every exchange stays in each agent's own conversation"],
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
    title: "Three steps. A wizard walks you through the rest.",
    items: [
      { title: "Install it", body: "From the Microsoft Store, or unzip the portable copy anywhere you like." },
      { title: "Choose your AI", body: "The first launch opens Getting set up. Pick your main AI from the ones on your PC. One you do not have yet opens its install page right there." },
      { title: "Add a project", body: "Pick a folder or clone from a Git URL — on a MicroVM or an SSH server too. Then press where it points, and the AI starts in that folder." },
    ],
    image: { src: "/lp/shot-setup.en.webp", alt: "The Getting set up screen: choose your main AI from the ones installed on this PC, with a link to install the ones that are not" },
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
        a: "The Store copy is signed by Microsoft, shows no warning and updates itself. The zip needs no installing: unzip it anywhere you like -- Google Drive, a USB stick -- and run it. Because it is not code-signed, Windows says “Windows protected your PC” the first time. That is expected; More info → Run anyway gets past it.",
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

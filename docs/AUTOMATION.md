# Automation reference

A way to run something automatically when a tab changes state.
It is written in Lua, a small scripting language, but a few lines is all you need.

This document is both an **explanation for humans** and the **specification** that the
"let an AI write it" button in the settings screen hands to the AI.

Translations live next to this file as `docs/AUTOMATION.<code>.md` (for example
`docs/AUTOMATION.ja.md`) and are picked automatically from your language setting.

---

## 1. When it runs (events)

Put a file named after an event into the automation folder and it runs at that moment.
Only add the ones you need.

| File name | When it runs |
|---|---|
| `on_start.lua` | Once the tab has started and settled (see below) |
| `on_done.lua` | When the AI has finished answering something it was asked |
| `on_question.lua` | When the AI asks something or offers choices |
| `on_exit.lua` | When the session ends (including disconnects and crashes) |
| `on_busy.lua` | When an answer starts (advanced). Set **Checking on a tab that keeps working** in Settings and it runs again, at that interval, for as long as the tab is still working |
| `on_notify.lua` | When the program rings the terminal — a bell, an OSC notification, even over ssh. The second variable holds the text. Forward it with `shikisha.notify(...)`, route it, or log it; the on-screen toast still shows |
| `_shared.lua` | Loaded before all of the above. Put shared helper functions here |

`on_done.lua` and `on_busy.lua` only run once something has been sent to the tab. Every
program prints something as it starts, which makes the screen move and then stop — the same
shape as an answer — so without this a banner would be forwarded as if it were a reply.

`on_start.lua` does not run the instant the tab appears. An AI CLI ignores input until
it has drawn its own prompt, so it runs once the program has produced output and the
screen has stopped changing — usually a second or two. You do not have to wait yourself.

Write **only the body** of the work in the file. No `function ... end` wrapper.

```lua
-- example of on_done.lua
shikisha.send_to_tab(2, "Please review this code:\n" .. tab.output)
```

---

## 2. Variables you can use

`tab` is available in every event.

| Variable | Contents |
|---|---|
| `tab.index` | Tab number (starting at 1) |
| `tab.name` | Tab name |
| `tab.id` | The automation name from the settings, if it has one. The one handle that survives a rename — branch on this, not the number or the display name. `nil` when unset |
| `tab.output` | **The latest response text** (no earlier history) |
| `tab.state` | `"BUSY"` / `"DONE"` / `"QUESTION"` / `"WAIT"` / `"EXIT"` |
| `tab.profile` | Name of the profile in effect |
| `tab.chain_depth` | How many times this was handed on automatically. **0 means a human started it** |
| `tab.locked` | Whether input is locked |
| `tab.is_model` | Whether this tab talks to a model over an API rather than running a CLI |
| `tab.reply` | A model tab's reply, exactly as it came back (only on such a tab). `tab.output` is the same text as the screen drew it, wrapped |

`on_question.lua` gets a second variable `screen` holding the whole screen text; `on_notify.lua` gets a second variable holding the notification text.

---

## 3. Commands you can use

### How to point at a tab

Numbers **change when you reorder tabs**, so pointing by name is the default.

```lua
shikisha.send_to_tab("Review", "please review")   -- recommended
shikisha.send_to_tab(2, "please review")          -- numbers work too (they shift on reorder)
```

If you plan to rename a tab, or if **several tabs share a name**, give it an
"automation name" (`id`) in the settings. With an id you can rename the tab freely
and the automation keeps working.

```jsonc
{ "name": "Review", "id": "reviewer", "command": "codex" }
```

```lua
shikisha.send_to_tab("reviewer", "please review")   -- survives renaming
```

| Command | Description |
|---|---|
| `shikisha.send_to_tab(tab, "text")` | **Give a tab an instruction and run it.** Works on this tab too (automatic chain +1) |
| `shikisha.send(tab, "text")` | Send raw keystrokes (newline is `\r`). For answering prompts, not for instructions |
| `shikisha.note(tab, "text")` | Write a line **on** that tab's screen for the person watching. Nothing is sent to what runs there and no answer is expected |
| `shikisha.wait(tab, "pattern", ms)` | Wait until the text appears on screen; `true` if it did |
| `shikisha.sleep(ms)` | Wait (other tabs keep running while you wait) |
| `shikisha.state(tab)` | Read the state **right now** (use this as a loop condition) |
| `shikisha.wait_state(tab, "DONE", ms)` | Wait until it reaches that state |
| `shikisha.notify("target", "text")` | Notify Slack / Telegram (only configured targets) |
| `shikisha.restart(tab)` | Restart that tab, carrying its conversation over. `shikisha.restart(tab, "fresh")` starts a new one |
| `shikisha.log("text")` | Record in `logs/hooks.log` |
| `shikisha.set_session("id")` | Say which conversation THIS tab's CLI is running, so a restart can pick it up. No tab argument: the caller is the tab |
| `shikisha.set_state("BUSY")` | Say what THIS tab is doing, instead of leaving it to be read off the screen: `BUSY`, `QUESTION`, `DONE` or `WAIT`. This is how an AI CLI's own hooks drive the state dot. A second argument is the sender's clock in milliseconds, so reports that overtake each other still apply in the order they were said |
| `shikisha.set_status("key", "text", tab)` | Say what a tab is doing, in its own words, under its name in the tab bar. `key` lets several sources speak without overwriting each other; an empty text removes that one. Leave `tab` out and it is THIS tab |
| `shikisha.set_progress(0.4, "label", tab)` | How far along, 0..1, shown beside the status. `nil` removes it. Leave `tab` out and it is THIS tab |

**A CLI that has never heard of this app can say it too.** The notification escapes
every terminal understands land in the same place, with nothing to set up — useful
over ssh, or inside a container, where nothing of ours is installed:

```sh
printf '\e]777;notify;Build;3 tests failed\a'    # title and body
printf '\e]9;build finished\a'                    # body only
```

It appears under that tab's name and, if you are looking at a different tab, as a
one-line toast. Looking at the tab already is not news, so the toast is held back.
| `shikisha.get_var("key")` / `shikisha.set_var("key", value)` | Remembered variables, shared inside the workspace |

If `on_question.lua` **returns a string**, that string is sent automatically.
Returning `nil` (or nothing) leaves the decision to the human.

### Instructing an AI: use `send_to_tab`

An AI CLI takes a pasted instruction and the Enter that runs it as two separate events,
and drops the Enter if it arrives before the paste has been taken in. `send_to_tab`
handles that for you.

```lua
-- Right. One call: the text is entered and run
shikisha.send_to_tab(tab, "You are on Bianca's side. Argue your case.")

-- Wrong. The text lands in the input box and stays there
shikisha.send(tab, "You are on Bianca's side. Argue your case.")
shikisha.send(tab, "\r")
```

**Do not paper over it with `sleep`.** A fixed wait is a guess about how long the other
program takes to be ready, and that changes with the machine, the model and the length of
the prompt — it will hold until the day it does not. `send_to_tab` waits on the actual
event instead of on the clock.

`send` remains the right tool for keystrokes an AI is already waiting for — answering a
confirmation with `"1\r"`, or driving a shell.

---

## 4. Common examples

### Give a tab its opening instruction (on_start.lua)

```lua
shikisha.send_to_tab(tab, "Summarise what changed in this project yesterday.")
```

Nothing else is needed — the hook already waits until the program is ready to be typed at.

### Resume yesterday's work just by starting (on_start.lua)

```lua
if not shikisha.wait(tab, "%$ $", 15000) then return end
shikisha.send(tab, "cd /srv/myproj\r")
shikisha.wait(tab, "%$ $", 5000)
shikisha.send(tab, "claude --continue\r")   -- pick the previous conversation back up
```

To choose which past conversation to resume, use `claude --resume`. It shows a list,
and picking from that list can be automated too:

```lua
shikisha.send(tab, "claude --resume\r")
if shikisha.wait(tab, "[Ss]elect", 8000) then
  shikisha.send(tab, "\r")     -- choose the topmost session
end
```

### Approve automatically, but hand risky questions to a human (on_question.lua)

```lua
if screen:match("delete") or screen:match("rm %-rf") then
  return nil          -- leave it to the human
end
return "1\r"          -- pick choice 1
```

### Bounce a review between A and B, stopping after 5 rounds (on_done.lua)

```lua
-- do nothing when a human gave the instruction directly
if tab.chain_depth == 0 then return end

local rounds = shikisha.get_var("rounds") or 0
if tab.output:match("LGTM") or rounds >= 5 then
  shikisha.notify("slack", "Review finished (" .. rounds .. " rounds)")
  return                                   -- doing nothing = the loop ends
end
shikisha.set_var("rounds", rounds + 1)
shikisha.send_to_tab(1, "Please fix these points:\n" .. tab.output)
```

### Reconnect automatically after a disconnect (on_exit.lua)

```lua
local n = (shikisha.get_var("retry") or 0) + 1
if n > 5 then
  shikisha.notify("slack", tab.name .. " keeps dying")
  return
end
shikisha.set_var("retry", n)
shikisha.sleep(2000)
shikisha.restart(tab)      -- after the restart, on_start runs again
```

### Check in periodically (on_busy.lua)

The screen and the other tabs keep running while you `sleep`. You choose the interval.

```lua
-- while it is working, record every 30 seconds
while shikisha.state(tab) == "BUSY" do
  shikisha.sleep(30000)
  shikisha.log(tab.name .. " is still working")
end
```

`tab.state` is the state **at the moment you were called**, so use
`shikisha.state(tab)` (the state right now) as the loop condition.
When a tab exits or restarts, waiting loops are discarded automatically.

Without the loop: set **Checking on a tab that keeps working** in Settings, and
`on_busy.lua` is simply run again at that interval while the tab keeps working.
Each run is told the state and the screen as they are now, so a watchdog can be
written as one `if` instead of a loop:

```lua
-- a turn that has run this long has stopped answering, not started thinking
local since = shikisha.epoch_ms() - (shikisha.get_var("since_" .. tab.index) or 0)
if shikisha.get_var("since_" .. tab.index) == nil then
  shikisha.set_var("since_" .. tab.index, shikisha.epoch_ms())
elseif since > 900000 then
  shikisha.notify(tab.name .. " has been working for 15 minutes without a word")
  shikisha.set_var("since_" .. tab.index, shikisha.epoch_ms())
end
```

You are only asked again about a tab you were told about in the first place, and
never about one waiting on a person.

### Just notify Slack when it is done (on_done.lua)

```lua
shikisha.notify("slack", tab.name .. " finished:\n" .. tab.output)
```

---

### Stop before it starts, and leave a note (on_done.lua)

```lua
if shikisha.state(2) ~= "WAIT" then
  shikisha.skip("tab 2 is still working")   -- nothing below this line runs
end
shikisha.send_to_tab(2, tab.output)
```

`skip` ends this run where it is called and puts one line on the tab's screen and
in `logs/hooks.log`. Use it whenever an automation decides not to act: a hand-over
that quietly does nothing looks exactly like a hand-over that is broken.

---

### What gets passed on

Only the reply is forwarded — not the terminal furniture around it. Startup banners,
input-box borders, and the hint and status lines a CLI keeps at the bottom
(`? for shortcuts`, the model and directory readout) are dropped.

They are found by **position and by change, never by matching their text**. Rows below
the cursor belong to the input box whatever they say; everything else is compared
against a snapshot taken the instant the prompt was submitted, so anything already on
screen before the reply existed is not part of the reply. A CLI can reword or
translate its status line and this keeps working, and a reply can contain any wording
at all without risk of being eaten.

The instruction itself is not sent back either. A reply begins on the line *after* the
one that was submitted, which matters when the instruction was long enough to wrap: the
second half of it used to arrive at the top of the answer, on its own, looking like
something the other side had said.

One thing is beyond reach: **narrowing the window while a reply is arriving truncates
it**, because the terminal clips every stored row to the new width and the discarded
text is gone. Widening and height changes are harmless. A narrowing mid-reply is
recorded in `logs/hooks.log` so a short answer is not a mystery.

### Watching it happen

Handing work to a tab does not move the screen. Say so when you want to be watched:

```lua
shikisha.show("Review")             -- put this tab on screen
shikisha.send_to_tab("Review", msg) -- ...and hand it the work
```

Two lines, in that order, and nothing moves behind your back. `shikisha.show(0)` goes
back to the board.

The person always outranks the script. `show` does nothing if they turned **Auto-switch**
off in the general settings, if they moved the view themselves in the last few seconds,
or while the settings screen is open — you are never pulled away from something you are
reading.

The ball still flies on the board whether or not the screen follows it: it shows who is
holding the work, which is not the same question as where you are looking.

---

### Leaving a draft for a person to finish

`send_to_tab` types and submits. To leave something in the box for a person to add to,
send it as a paste and never send the newline:

```lua
shikisha.draft_to_tab("ai", "Read lp.html.

")
```

The text lands in the input box and stays there. The newlines are characters, not
keypresses, so nothing is sent and the person can add their own instructions before
pressing Enter. This program does not count it as a submission either, so `on_done`
will not fire on that tab.

**A draft does not end the chain — it puts a person in it.** The ball moves to that
tab and waits there, carrying its depth, and the view follows so you arrive where you
are needed. Typing there does not break the chain the way typing into any other tab
does; you are taking your turn, not taking over. When you send, the count continues
from where it was, and the chain limit still applies — a loop with a person in it is
still a loop.

**It refuses to draft into a shell**, and says so in `logs/hooks.log`. A terminal
program declares whether it understands pasted text; a shell does not, and the same
bytes there would run as a command. Measured: `cmd.exe` and `powershell.exe` do not
declare it, Claude Code does. The check reads that declaration rather than guessing
from the command name.

Keep the draft short. A long paste is collapsed to `[Pasted text #1 +N lines]`, and the
person cannot read what they are about to send.

---

### Driving a browser

A browser can join the orchestra. Windows already carries the engine, so nothing is
downloaded and nothing is installed.

Declare one alongside the tabs of a workspace. A browser you declare becomes a tab,
numbered after the sessions — `Ctrl+B` and its number switches to it like any other.

```json
{
  "name": "LP review",
  "browsers": [{ "id": "br", "url": "https://example.com/login" }],
  "tabs": [{ "name": "Claude", "id": "ai", "command": "claude" }]
}
```

### Hooks on a browser

A page has its own vocabulary. Session states — working, done, asking — say nothing
about a document, so browsers get their own names.

| File | When |
|---|---|
| `on_load.lua` | the page finished loading (**on every navigation**) |
| `on_press.lua` | the human pressed the banner button |

The banner does not appear on its own. `shikisha.browser_ask` puts it along the bottom
of the page — your words on the left, the button on the right — and pressing it calls
`on_press`. It is immune to the site's own CSS (it lives in a shadow root).

To let a person choose the page before handing it over, `shikisha.browser_nav` puts
back / forward / reload / an address box in a row above it. Unlike the banner this is
**not** injected into the page: the page moves down and the app draws in the gap, so it
survives navigation and never covers the site's own sticky header.

```lua
shikisha.browser_nav(page.id)                                 -- all of them
shikisha.browser_nav(page.id, { reload = true, url = true })  -- pick some
-- back / forward / reload / reload_hard (fetch it all again) / url
shikisha.browser_unnav(page.id)                               -- take it away
```

Back and forward grey out when there is nowhere to go. The address box only opens
http/https. The same four checkboxes live in the settings screen for a browser tab, so
this works with no Lua at all; a call from Lua wins over the setting.

The banner works the same way: fill in its words and button text under "Banner" in the
settings and it is there from the moment the page opens. Then the only file you write is

```lua
-- scripts/lp/on_press.lua
shikisha.draft_to_tab("ai", shikisha.browser_html(page.id))
```

Typing an address still fires `on_load`, so if you only want the page handed over when
a person says so, leave `on_load.lua` empty and write `on_press.lua`. None of this
touches the chain depth — that only counts handoffs to another tab.

What arrives is `page`, not `tab`: `page.index` (the number on screen), `page.id` (the
name automation points at), `page.name` (what a person reads), `page.url`, and
`page.complete` — false means `load` never came and this fired at DOM-ready instead.

`draft_to_tab` and `send_to_tab` wait until the other side can actually take input, so
handing work to a CLI that has not finished starting does not silently vanish.

Then drive it from automation:

```lua
-- Let a person log in. The bar appears across the bottom of the page.
local why = shikisha.browser_wait("br", {
  selector = "#dashboard",            -- reaching this ends the wait
  ask      = "Please sign in",        -- so does pressing the button
  timeout_ms = 300000,
})
shikisha.log("ended by: " .. why)     -- selector / button / timeout

shikisha.browser_fill("br", "#title", answer)
shikisha.browser_click("br", { xpath = '//button[text()="Save"]' })
local html = shikisha.browser_html("br")
```

A selector is `"#id"` (CSS), `{ xpath = "..." }`, or `{ ref = N }`. XPath earns its
place on forms and admin pages, where "the cell beside the label that reads 名前" has
no CSS spelling.

The numbers for `{ ref = N }` come from `browser_digest`:

```lua
local list = shikisha.browser_digest("br")
-- [1] textbox "Search" placeholder="Search"
-- [2] button "Search"
-- [3] link "Help" https://example.com/help
shikisha.browser_fill("br", { ref = 1 }, "haiku")
shikisha.browser_click("br", { ref = 2 })
```

The digest distills the page down to **only its operable elements**. Roles and names
come from the browser's own accessibility tree (the same computation a screen reader
sees), and JS-clickables with no standard role (a `cursor:pointer` `<div>`, say) are
supplemented with a `*` mark, like `div*`. It is orders of magnitude shorter than raw
HTML, and removes the need to guess selectors.

Operations on `{ ref = N }` are **genuine input** (trusted mouse/key events over CDP):
sites that ignore synthetic events cannot tell them from a human's click or typing.
Multibyte text lands one committed character at a time, no IME involved.

Numbers are bound to the page as it was digested. Navigation or a re-render voids
them, and an operation on a stale number stops with a clear "take a new digest"
error — it never silently clicks something else.

On top of that, click / fill on `{ ref = N }` return **an echo of what was really
operated on as their second value** (e.g. `visible, link 「Help」`; a fill's echo names
the field by its attributes only, never the value). A mixed-up number denounces
itself in its own answer.

**On replay and portability**: `{ ref = N }` is an ordinary selector with the same
meaning in every execution mode (automation scripts, the composer's ▶ Lua run mode,
an operate rally). But the numbers refer to "the latest `browser_digest` listing" —
they are not what you carry around.

That is why **execution and recording are independent**. During an operate rally,
every executed op is **rewritten in a durable form** and appended to the run's
`replay.lua`: a `{ ref = N }` becomes an anchor derived from the element it actually
touched (a human-made `#id`, else a unique text/attribute XPath — same hygiene as the
📼 recorder, machine-minted ids refused), and `browser_digest` never appears. So:

- **the currency of execution = refs** (maximum capability: shadow DOM reach,
  genuine input, friendly to small models)
- **the currency of portability = replay.lua** (plain css / xpath only; paste it into
  the ▶ run mode, wire it into an automation, or run it on another PC's SHIKISHA as-is)

Download replay.lua from the "⬇ Replay Lua" button beside the 🎯 target dropdown, or
from the same button at the top right of the result view that opens when an operation
finishes. An op with no derivable durable anchor is never silently dropped — it stays
as a `-- click (…): what was clicked` comment.

Looking for an element answers with three states — `visible`, `off_screen`,
`not_found` — because which one it is decides whether to doubt the selector or the
waiting.

**Whether a missing element stops the script is chosen per call.** The default raises;
`{ on_missing = "continue" }` returns the state instead. A cookie banner that is
sometimes absent is not a failure, and only the caller knows that.

**The button is offered for the whole wait, even when a selector is given.** A
condition that stops matching after a site redesign should cost a click, not a hang.
And since the wait reports which of the three ended it, a selector that has quietly
stopped working shows up as every wait ending on the button.

**click / fill auto-wait.** An action waits until the element **appears → is
visible → stops moving (identical rect on consecutive frames) → is enabled**
before acting. Retries back off 0/20/100/100/500ms
and cycle scroll alignments to shake off sticky overlays; when navigation destroys
the JS world, an outer retry re-enters the new document. That is why a replay.lua
fired line-after-line with no pauses — acting on the next page right after
`browser_go` — just works. The wait is capped at 10s per action, and an element that
exists but never settles is acted on anyway (no new failure modes).

**Values are never spliced into code.** Everything handed to `fill` goes to the page
as data, so an answer full of quotes and angle brackets arrives intact and stays
inert. There is deliberately no way to hand raw JavaScript to a page.

Only `http` and `https` pages open. A single-line `<input>` cannot hold a newline —
that is HTML, not this program — so multi-line values need a `textarea`.

---

## 5. Safety mechanisms

Several brakes keep automation from running away.

- **Automatic chain limit** … the number of consecutive automatic hand-offs between AIs is
  counted and stops at the limit (10 by default). Typing something yourself resets it to 0
- **Manual work wins** … nothing is sent automatically for 5 seconds after you touch a tab
- **Emergency stop** … `Ctrl+B x` halts all automation at once, `Ctrl+B a` toggles it.
  The status bar carries the same button, in the same corner on every screen
- **Input lock** … put 🔒 on the middle tabs so nobody instructs them by mistake
- **Sandbox** … automation can **neither touch files nor reach the internet by default**.
  Notifications only go to the Slack / Telegram targets you registered

---

## 6. Files and network access (advanced, off by default)

When you really need it, register a "gateway" in `config.json` and it becomes available.
It cannot be edited from the settings screen (the impact is large, so it is only for
people who edit the file directly).

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
shikisha.http("github-issue", '{"title":"Findings","body":"..."}')
```

| Command | Description |
|---|---|
| `shikisha.now([format])` | The local date and time as text |
| `shikisha.write_file(gateway, filename, text)` | Write into a registered folder |
| `shikisha.read_file(gateway, filename)` | Read from a registered folder |
| `shikisha.http(gateway, body)` | Send to a registered URL (the app adds the credentials) |

**Why this is safe**: scripts cannot assemble paths or URLs — they can only call registered
names. Auth tokens are invisible to scripts; the app attaches them.
`config.json`, `secrets.json`, `.env` and `.lua` files can never be read or written, even
when they sit inside an allowed folder.

If you need more freedom, raw paths and raw URLs are available too (**empty by default =
everything denied**):

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

Hosts are matched **exactly** and only `https` is allowed
(tricks like `api.example.com.evil.com` are rejected).
Every file and network operation is recorded in `logs/hooks.log`.

---

## 7. Driving it from outside (the external API)

A program outside the app can call the same commands you write in Lua. Same names, same
arguments — there is no second vocabulary to learn.

The door is a **named pipe**, `\\.\pipe\shikisha-<pid>`. One JSON object per line, one
line of answer back:

```text
→ {"token":"…"}                                                  the handshake, once
← {"ok":true,"result":"hello"}

→ {"id":"1","method":"send_to_tab","params":["reviewer","status?"]}
← {"id":"1","ok":true,"result":null}

→ {"id":"2","method":"list"}
← {"id":"2","ok":true,"result":["browser_click","browser_close", … ]}
```

`method` is a command from section 9 with the `shikisha.` taken off. `params` are its
arguments in order. `list` answers with every command **the caller is allowed to run**, read off the app's own
table — so it can never fall behind what the app can actually do.

For loops and branches, hand over a whole chunk in one call:

```text
→ {"id":"3","method":"lua","params":["for i=1,3 do shikisha.send_to_tab(i,'ping') end"]}
← {"id":"3","ok":true,"result":[null,null]}
```

The answer to `lua` is always a pair: **the first value is the error, or `null` when the
chunk ran**, followed by whatever it returned.

### Who is allowed in

On the settings screen it is the **External control** card; changing it there takes
effect the moment you save, with no restart. In the file it is one line:

```jsonc
"external_api": { "access": "children" }   // the default
```

| Value | Who can call |
|---|---|
| `children` | Only what the app started — a tab's CLI, and whatever that starts in turn |
| `user` | Anything running as you. The token is also written to `data\api-token` |
| `off` | Nothing. The pipe is not created at all |

Every tab's process is launched knowing three things, so an AI sitting in a tab needs no
setup at all:

| Variable | Holds |
|---|---|
| `SHIKISHA_PIPE` | The pipe to connect to |
| `SHIKISHA_TOKEN` | That tab's own key, minted for it at launch |
| `SHIKISHA_TAB` | Which tab it is sitting in |

Because the key is the tab's own, a call arrives already knowing who is making it — and
what that tab sends counts against **the same chain limit** (section 5) as work handed over
on screen. The API is not a way around the brakes. If an AI is what is running in that tab,
what it may call is what **Settings > Automation permissions** (section 9) allows an AI.

**What the token protects, and what it does not.** The pipe is created with an access list
naming your account and nobody else, so another account cannot reach it. Another program
running as *you* can read the environment of your own processes, and an AI in a tab can
copy its key into its own log. What this stops is an accident, and another account — not
someone who is already you.

The first caller of each session is written to `logs/hooks.log`, along with any connection
that presented no valid key.

---

## 8. Tips for writing it

- **Join strings with `..`** (not `+`)
- **`tab.output` holds only the latest response**, never the earlier conversation
- **Lua patterns are their own thing**: `%d` (digit), `%s` (space), `.-` (shortest match).
  Write `%d`, not `\d`
- **When you want to do nothing, write `return`** and it stops right there
- If you get lost, sprinkle `shikisha.log()` and read `logs/hooks.log`

---

## 9. Every command

Everything automation can call, in one place. The sections above teach the common
ones; this is the complete list.

### Whether it may run is decided in Settings > Automation permissions

The same command can be allowed for **you** and refused for **an AI**. The settings
card lists every command with two boxes: one for a person, one for an AI.

- **An AI** — a call from an **AI tab** (a tab whose command is an AI: `claude`,
  `codex`, `gemini`, `aider` and the like, or a tab talking to a model over an
  API), and Lua an AI wrote (inside `run_scoped`)
- **A person** — everything else: the hooks and scripts you wrote, the run
  button, and programs you started yourself

> **An AI you start by hand inside a terminal tab counts as you.** Open a `cmd`
> or PowerShell tab, type `claude` in it, and **unticking the AI column will not
> stop that AI** — it is calling with the tab's own key, and the tab is a
> terminal. To have it counted as an AI, **make the tab's own command the AI**.

Nearly everything is ticked in both columns to begin with. **Six commands start out
closed to an AI**, and each one either steps outside this table or destroys
something you own:

| Command | Why it starts closed to an AI |
|---|---|
| `lua` | Runs code with nothing walled off. Open it and the table means nothing |
| `read_path` / `write_path` / `http_raw` | Raw paths and raw URLs, past the gateways. Allowed folders and hosts are the escape hatch you opened for your own scripts |
| `close_pane` | Takes away a place you were looking at |
| `restart` | Throws away the conversation running in a tab |

A command that is switched off answers with a sentence saying so, and the refusal
is written to `logs/hooks.log`. Nothing ever fails in silence. `shikisha.list()`
answers for whoever asked, too: it leaves out what that caller may not call.

Only the rows you changed are written to the config file. Anything left standard is
not written at all.

```jsonc
"automation_permissions": {
  "lua": { "ai": true },          // open it to an AI as well
  "send_to_tab": { "ai": false }  // close it to an AI
}
```

### Tabs and turns

| Command | Description |
|---|---|
| `shikisha.send_to_tab(tab, "text")` | **Give a tab an instruction and run it.** Works on this tab too (chain +1) |
| `shikisha.send(tab, "text")` | Raw keystrokes (newline is `\r`). For answering a prompt, not for instructing |
| `shikisha.draft_to_tab(tab, "text")` | Leave the text in the tab's input box **without** running it — a person finishes and sends |
| `shikisha.note(tab, "text")` | Write a line **on** that tab's screen. For the person watching only: nothing reaches what runs there, and nobody is asked to answer |
| `shikisha.state(tab)` | The state right now: `WAIT` / `BUSY` / `DONE` / `ASK` / `EXIT` |
| `shikisha.wait_state(tab, "DONE", ms)` | Wait until it reaches that state; `true` if it did |
| `shikisha.tab_output(tab)` | Another tab's latest reply (`""` if there is none yet) |
| `shikisha.tab_screen(tab)` | What is on that tab's screen right now. The reply is what a turn produced; this is the glass -- for a pager, a menu or any full-screen program it is the only output there is |
| `shikisha.tab_read(tab, mark)` | That tab's recorded output from `mark` onward. Returns the text and the next mark, so a long run is followed in pieces without reading the same piece twice. Starts at `0`; a tab that is not being recorded reads as `""` and gives the mark back |
| `shikisha.restart(tab)` | Restart that tab, carrying its conversation over. `shikisha.restart(tab, "fresh")` starts a new one |

### The screen

| Command | Description |
|---|---|
| `shikisha.show(tab)` | Put that tab on screen. `0` is the board. Ignored if the person turned Auto-switch off, just moved the view themselves, or is in the settings |
| `shikisha.open_result(run)` | Open that run's transcript as a result page and go to it |
| `shikisha.split_pane("right")` | Divide the pane in focus. `"right"` beside, `"down"` below. The new half takes focus |
| `shikisha.close_pane()` | Close the pane in focus. The tab behind it keeps running |
| `shikisha.focus_pane("left")` | Move focus to the neighbouring pane (`"left"` `"right"` `"up"` `"down"`) |
| `shikisha.equalize_panes()` | Put every divider back to even halves |

**Put a browser beside the agent** — two commands, in the order you would say them:

```lua
shikisha.split_pane("right")   -- divide, and the new half takes focus
shikisha.show("br")            -- ...so this puts the browser there
```

There is no one command for that on purpose. `split_pane` and `show` each do one
thing, and every arrangement anyone wants is some order of the two — a combined
"split and open a browser" would only ever be the first arrangement somebody
thought of.


### Waiting and time

| Command | Description |
|---|---|
| `shikisha.wait(tab, "pattern", ms)` | Wait until the text appears on that tab's screen; `true` if it did |
| `shikisha.sleep(ms)` | Wait (other tabs keep running) |
| `shikisha.now("%Y-%m-%d")` | The local date/time, formatted. Sorts chronologically by default — good in file names |
| `shikisha.epoch_ms()` | Milliseconds since the epoch, as a number, for measuring elapsed time |

### Remembering, logging, telling someone

| Command | Description |
|---|---|
| `shikisha.get_var("key")` / `shikisha.set_var("key", value)` | Remembered variables, shared within the workspace |
| `shikisha.log("text")` | Write a line to `logs/hooks.log` |
| `shikisha.notify("text")` / `shikisha.notify("target", "text")` | Notify Slack / Telegram (only targets you configured). With no target named, it goes to the default one |
| `shikisha.remote_url()` | The URL a phone can reach this app on, or `nil` while remote is off. Put it in a notification so "come and help" is one tap away |
| `shikisha.t("key")` / `shikisha.tf("key", {name="..."})` | Look up a translated string (`tf` also substitutes `{name}`). Used by the built-in orchestrators so they speak the app's language |

### Reporting your own state

By default these are about **the tab that called them**, which is why no tab is
usually named. `set_status` and `set_progress` take one as a last argument, to
report about another tab. An AI CLI's own hooks report through here too.

| Command | What it does |
|---|---|
| `shikisha.set_state("BUSY")` | **This tab** says what it is doing rather than leaving it to be read off the screen (`BUSY` / `QUESTION` / `DONE` / `WAIT`). The second argument is the sender's clock in milliseconds, so reports that overtake each other still apply in the order they were said |
| `shikisha.set_status("key", "text", tab)` | Say what is being worked on, in its own words (shown under the tab name). Separate keys let several writers speak without overwriting each other; an empty string clears one. Leave `tab` out and it is **this tab** |
| `shikisha.set_progress(0.4, "label", tab)` | How far along it is (0..1), shown beside the state. `nil` clears it. Leave `tab` out and it is **this tab** |
| `shikisha.set_session("id")` | **This tab** says which conversation its CLI is running, so a restart can pick it back up |

### Browsing

A page is addressed by the id you gave it. See "Driving a browser" above.

| Command | Description |
|---|---|
| `shikisha.browser_open(id, url, profile, private)` | Open a page. `profile` names its cookie store; `private` makes a throwaway one |
| `shikisha.browser_close(id)` | Close it |
| `shikisha.browser_go(id, "back"/"forward"/"reload"/"to", url)` | Navigate |
| `shikisha.browser_nav(id, {...})` / `shikisha.browser_unnav(id)` | Show / hide back-forward-reload-address above the page |
| `shikisha.browser_find(id, sel)` | Is it there? `"visible"` / `"hidden"` / `"missing"` |
| `shikisha.browser_click(id, sel, opts)` | Click it. `opts` is `{ on_missing = "continue" }` -- answer with the state instead of stopping |
| `shikisha.browser_fill(id, sel, "text", opts)` | Type into it. **Does not submit** — follow with `browser_press`. `opts` is the same as `browser_click`'s |
| `shikisha.browser_fill_secret(id, sel, "KEY")` | Fill from a registered secret. The value never reaches the script |
| `shikisha.browser_press(id, "enter")` | Press a key on the page |
| `shikisha.browser_text(id, sel)` | The visible text |
| `shikisha.browser_html(id)` | The whole document |
| `shikisha.browser_digest(id)` | The operable elements, numbered — what to read before deciding a move |
| `shikisha.browser_fetch(id, url, opts)` | Request from inside the page (keeps its cookies). Returns `{status, ok, url, headers, body}` |
| `shikisha.browser_auth(id, "KEY")` | Answer basic-auth from a registered secret |
| `shikisha.browser_state_save(id, "label")` | Save this page's login — its cookies and its localStorage — under a name. Returns how many cookies were saved. Sign in once, then a later rally can load it |
| `shikisha.browser_state_load(id, "label")` | Put a saved login back, so the page is signed in without logging in again |
| `shikisha.browser_snapshot(id, "label")` | Take a picture of the page (PNG) and save it. Returns the file path — a rally can keep a visual record of what it did |
| `shikisha.browser_ask(id, "text", "label")` | Put a banner with a button along the bottom of the page |
| `shikisha.browser_pressed(id)` | Has it been pressed? |
| `shikisha.browser_unask(id)` | Take the banner away |
| `shikisha.browser_wait(id, {ask=..., selector=..., timeout_ms=...})` | Wait for whichever comes first. Returns `"selector"` / `"button"` / `"timeout"` |

### Handing a run between participants

How the rally works: files in and out, plus a judge. You can build your own the same way.

| Command | Description |
|---|---|
| `shikisha.contract()` | The promises a tab is asked to keep while it holds a turn: say it in words rather than opening a confirmation prompt, report once, say what you did and what is left, then wait. Send it with the **opening** instruction, not every turn |
| `shikisha.exchange_new()` | Make a folder for this run and return its path |
| `shikisha.exchange_write(path, "text")` | Write a file (overwrites) |
| `shikisha.exchange_append(path, "text")` | Append to one |
| `shikisha.exchange_take(path)` | Read it, delete it, return it. `nil` if absent — this is the hand-over |
| `shikisha.ai_ask("what you want")` | Ask the **assistant AI** from Settings > Basic and get the answer as text; `nil` and a reason when there is none. **The app keeps running while it thinks** (the same machinery as `sleep`: other tabs and the screen carry on). Three minutes by default, `{timeout_ms=…}` to change it |
| `shikisha.lint(code)` | Compile-check Lua without running it. An error string, or `nil` if sound |
| `shikisha.run_scoped(id, code)` | Run AI-written Lua against one page, in a jail: no files, no network, no other tabs. Returns `err, out` |
| `shikisha.lua(code)` | Run a whole chunk with everything in reach — loops, branches, several commands at once. Returns `err` (`nil` when it ran) followed by whatever the chunk returned. The unwalled twin of `run_scoped`, so never hand it code you didn't write |
| `shikisha.list()` | The commands the caller may run, by name (see automation permissions above). Read off the table itself, so it is never out of date |
| `shikisha.record(text)` / `shikisha.record_reset()` | Keep a pasteable record of the run |
| `shikisha.take_replay()` | Drain the replay journal — the durable spelling of every operation since the last drain |
| `shikisha.set_result(code, "reason")` | The run's verdict. Written to `data/last-result.json` and shown on screen |
| `shikisha.skip("reason")` | Stop this run here and say so: one line on the tab's screen and in the log. For when an automation decides there is nothing to do |

### Working with git

**Which repository** is named by the tab sitting in it. Leave the tab out and it is the tab
that called. No path is accepted.

The reading side launches git. The branch shown in the sidebar comes from somewhere else --
that path never launches git, which is why it still answers during a rebase.

| Command | What it does |
|---|---|
| `shikisha.git_status(tab)` | The changed files, one row each: `{path, index, work, staged, unstaged, conflict, from}`. `index` and `work` are git's own two letters (staged side, working-tree side). `staged` and `unstaged` are not opposites — stage one hunk of a file and both are true |
| `shikisha.git_diff(tab, {path=…, staged=…})` | The diff, as text. `staged=true` reads the staged side; `path` narrows it to one file |
| `shikisha.git_log(tab, count)` | Recent commits: `{hash, short, author, date, subject}`. 20 by default |
| `shikisha.git_conflicts(tab)` | Just the paths of the files with a conflict |
| `shikisha.git_branch(tab)` | The branch: `{name, protected}`. `protected` marks one this folder guards, so committing straight onto it is worth asking about. `nil` when the head is detached |
| `shikisha.git_graph(tab, {all=…, remotes=…, count=…})` | The history: `{graph, hash, short, author, date, subject}`. `graph` is git's own drawing, and the rows with no commit on them (a merge closing) are kept |
| `shikisha.git_detail(tab, hash)` | One commit in full: `{hash, parents, author, author_date, committer, commit_date, subject, body, files}` |
| `shikisha.git_branches(tab)` | Every branch: `{name, current, protected}` |
| `shikisha.git_checkout(tab, "name")` | Move onto that branch |
| `shikisha.git_merge(tab, "name")` | Bring that branch in. A conflict stops it, and shows up in `git_conflicts` |
| `shikisha.git_fetch(tab)` / `shikisha.git_pull(tab)` / `shikisha.git_push(tab)` | Talk to the server. **Everything else waits** until it answers (up to three minutes). `git_push` sets the upstream and retries when the branch has never been sent, and says so in its answer |
| `shikisha.git_hunks(tab, {path=…, staged=…})` | The diff cut into hunks: `{file, header, start, end, patch}`. Each `patch` is a whole patch on its own |
| `shikisha.git_apply(tab, patch, {cached=…, reverse=…})` | Apply a patch. `cached` puts it in the next commit, `reverse` takes it back out. **Staging one hunk is these two together** |
| `shikisha.git_stage(tab, paths)` | Add to the next commit. One path as a string, or several in a table |
| `shikisha.git_unstage(tab, paths)` | Take back out of the next commit |
| `shikisha.git_branch_create(tab, "name")` | Make a branch and move onto it. Staged work moves with you, which is what makes this **the way out of a refusal on a shared branch** |
| `shikisha.git_commit(tab, "message", opts)` | Commit what was added and answer with the short hash. **It stops on a protected branch** -- make a branch, or pass `{allow_protected=true}` to say you meant it. Which branches those are comes from Settings > Protected branches (`main` and `master` until somebody says otherwise), and each working folder may name its own |
| `shikisha.git_run(tab, "args…")` | Run any git and answer with its output. **No shell is involved**: `;` and `&&` arrive as arguments and git refuses them |

**All of these are open to a person only, to begin with** (automation permissions). If an AI
is to be let in, `git_status` / `git_diff` / `git_log` are the place to start. Opening
`git_run` is the same as handing it all of git.

### Files and the network

Off unless you register a gateway — see section 6.

| Command | Description |
|---|---|
| `shikisha.read_file(name, rel)` / `shikisha.write_file(name, rel, data)` | Through a registered file gateway |
| `shikisha.http(name, body)` | Through a registered HTTP gateway |
| `shikisha.read_path(p)` / `shikisha.write_path(p, data)` / `shikisha.http_raw(url, body)` | Raw path / raw URL. Always fails unless `allow_dirs` / `allow_hosts` says otherwise |

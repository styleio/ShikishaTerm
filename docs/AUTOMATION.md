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
| `on_busy.lua` | When an answer starts (advanced) |
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
| `tab.output` | **The latest response text** (no earlier history) |
| `tab.state` | `"BUSY"` / `"DONE"` / `"QUESTION"` / `"WAIT"` / `"EXIT"` |
| `tab.profile` | Name of the profile in effect |
| `tab.chain_depth` | How many times this was handed on automatically. **0 means a human started it** |
| `tab.locked` | Whether input is locked |

Only `on_question.lua` gets a second variable, `screen`, holding the whole screen text.

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
| `shikisha.wait(tab, "pattern", ms)` | Wait until the text appears on screen; `true` if it did |
| `shikisha.sleep(ms)` | Wait (other tabs keep running while you wait) |
| `shikisha.state(tab)` | Read the state **right now** (use this as a loop condition) |
| `shikisha.wait_state(tab, "DONE", ms)` | Wait until it reaches that state |
| `shikisha.notify("target", "text")` | Notify Slack / Telegram (only configured targets) |
| `shikisha.restart(tab)` | Restart that tab |
| `shikisha.log("text")` | Record in `logs/hooks.log` |
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

### Just notify Slack when it is done (on_done.lua)

```lua
shikisha.notify("slack", tab.name .. " finished:\n" .. tab.output)
```

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

The view follows the ball: when one tab hands work to another, the screen switches to
whichever tab was just given it. Turn it off in the general settings if you would rather
stay put. It holds off for a few seconds after you move around yourself, so it will not
pull you away from something you are reading.

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

A selector is either `"#id"` (CSS) or `{ xpath = "..." }`. XPath earns its place on
forms and admin pages, where "the cell beside the label that reads 名前" has no CSS
spelling.

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

## 7. Tips for writing it

- **Join strings with `..`** (not `+`)
- **`tab.output` holds only the latest response**, never the earlier conversation
- **Lua patterns are their own thing**: `%d` (digit), `%s` (space), `.-` (shortest match).
  Write `%d`, not `\d`
- **When you want to do nothing, write `return`** and it stops right there
- If you get lost, sprinkle `shikisha.log()` and read `logs/hooks.log`

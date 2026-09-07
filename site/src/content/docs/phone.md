---
title: "From your phone"
description: "See what every agent is doing, and answer it, from a train. On the same Wi-Fi this needs nothing installed; from anywhere it needs Tailscale, which is free."
sidebar:
  order: 0
---

An agent that finishes while you are out has finished ten minutes ago by the
time you sit back down. The phone view is the answer to that: every tab, what
it is doing, and a box to reply in.

There are three levels to it, and you can stop at any of them.

<img src="/phone.png" alt="SHIKISHA-TERM on a phone — a live dashboard of several agents, each showing whether it is working, waiting or done" style="display:block;margin:1.4em auto;width:320px;max-width:100%" />

## 1. On the same Wi-Fi — nothing to install

Open the settings, find **Phone**, and turn on *Let me check status and send
instructions from my phone*. A QR code appears. Point your phone's camera at
it.

That is the whole setup. Nothing is installed on the phone, no account is
made, and nothing of yours leaves the house: the page comes from a small web
server inside the program on your own PC, and your phone connects to it
directly.

**What you give up:** it stops at the front door. Leave the Wi-Fi and the
address is not reachable any more. Anyone else on that same network could
reach the address too — they would still need the access code inside the QR,
but the door is visible to them.

## 2. From anywhere — Tailscale

To reach your PC from a café, something has to connect the two ends. This
program will not put your machine on the public internet to do it; that is a
door that stays open whether you are at it or not.

[Tailscale](https://tailscale.com/) solves it a different way. It builds a
private network out of *your own devices only* — your PC, your phone, your
laptop — and gives each one an address that exists nowhere else. Traffic is
encrypted end to end and goes directly between the two devices. It is free for
personal use, and it is not ours: we neither run it nor see it.

1. Install it on the PC and on the phone —
   [tailscale.com/download](https://tailscale.com/download)
2. Sign in with the **same account** on both
3. Open the settings on the PC again. The Phone card now says
   *Tailscale (100.x.y.z) is available*
4. Scan the QR code as before

Nothing about this program changes. It notices the private address and offers
that one instead.

:::note[About the account]
This program has no account and never asks for one. Tailscale does — it signs
you in with Google, Microsoft, GitHub or Apple, because it has to know which
devices are yours. That is the honest price of reaching your own PC from a
train. On the same Wi-Fi you pay neither.
:::

## 3. On the home screen, and buzzing in your pocket

Two things a phone browser will only do for a page it considers *secure*:
keep it on the home screen like an app, and deliver notifications while it is
closed. "Secure" here means HTTPS, and a plain address on a private network —
however private — does not count.

Tailscale can put a real certificate in front of it. **One command, once**,
in any tab of the terminal:

```
tailscale serve --bg 8787
```

(If you changed the port in the settings, use that number instead.) Restart
the program and the Phone card will say *Served over HTTPS*. The QR code now
carries a `https://your-pc.your-tailnet.ts.net` address.

### Keep it on the home screen

Open the board on the phone and use the browser's **Add to Home Screen**. It
opens without an address bar, with its own icon, like an installed app.

For that icon to be worth having, the pairing has to outlive a browser tab —
otherwise it opens and asks to be paired again, which can only be done by
scanning the QR, which opens the browser rather than the app. So the offer
only appears with **Fixed token** turned on in the Phone settings.

That setting is labelled *accepting the risk*, and it means it: the access
code stops changing and stays in that phone's storage, so whoever holds the
phone can open the board. It is the same trade as staying logged in to
anything else. If that is not a trade you want, skip this step — the rest
works without it.

### Get notified when a tab answers

In the settings, under **Notifications**, add a destination and choose
**Phone**. Then, *on the phone*, open the same settings page and press
**Register this device**. The browser will ask for permission; say yes.

Then set any tab to notify when it answers. What arrives is the tab's name and
the first line of what it said — and tapping it opens a small page with that
answer and a box to reply in, so "yes, go ahead" does not mean walking back to
the PC.

On Android and on a desktop, notifications do not need the fixed token; only
the home-screen icon does.

**On iPhone and iPad it is the other way round.** Apple delivers web
notifications only to something that has been added to the home screen — in a
Safari tab the machinery is not there at all. So on an iPhone the step above
is the prerequisite for this one, fixed token included. Add it to the home
screen, open it from that icon, and press the button there.

The message is encrypted for that phone before it leaves your machine. It goes
through your browser vendor's push service, which can carry it and cannot read
it. There is no account of ours in the middle, because there is no server of
ours at all.

## What this does not do

- **Your PC has to be running.** All of this is a web server inside the
  program. Nothing is queued for a machine that is off, and nothing of yours
  is stored anywhere else to be delivered later.
- **We see none of it.** Read the [privacy policy](/privacy/) — the short
  version is that there is nothing for us to see.

# Translating SHIKISHA-TERM

English is the base language. Every other language is a JSON file that overlays it,
so a translation never has to be complete: any key you leave out falls back to English.

## Add a language

1. Copy `lang/en.json` to `lang/<code>.json`, where `<code>` is the two-letter
   language code (`de`, `fr`, `ko`, `zh`, …).
2. Translate the **values**. Never change the keys.
3. Keep the placeholders exactly as they are — `{name}`, `{n}`, `{error}` and so on are
   filled in at runtime. You may move them around inside the sentence.
4. Send a pull request. Only `lang/<code>.json` needs to change; nothing has to be
   registered anywhere.

```jsonc
// lang/de.json
{
  "common.save": "Speichern",
  "state.busy": "Arbeitet",
  "msg.restarted": "{name} wurde neu gestartet"
}
```

## Try it out

Set the language in the settings screen, or put it in `config.json`:

```jsonc
{ "language": "de" }
```

`"language"` is optional. Without it the system language is used, falling back to English.
Files under `lang/` next to the executable are read at startup, so you can drop your file
in and restart to see it.

## Rules the code enforces

- A key that exists in no file at all shows up on screen as the key itself
  (`settings.title`), which makes a missing entry obvious.
- `cargo test` checks that every key in every `lang/*.json` also exists in `lang/en.json`.
  A typo in a key name therefore fails the build, rather than silently doing nothing.

## The automation manual

`docs/AUTOMATION.md` is both the human documentation and the specification handed to the
AI by the "let an AI write it" button. Translate it as `docs/AUTOMATION.<code>.md` and it
is picked up automatically for that language.

To ship a translated manual inside the single `.exe`, add one line to `EMBEDDED_MANUALS`
in `src/webui.rs`. Without that line the file is still used when it sits in `docs/` next
to the executable.

## Notes for translators

- `state.*` are the words users see for tab status. The internal labels
  (`BUSY`, `DONE`, `QUESTION`, `WAIT`, `EXIT`) are **not** translated — automation scripts
  and logs depend on them.
- Key prefixes tell you where the text appears: `tui.*` in the terminal UI, `settings.*` in
  the browser settings screen, `phone.*` on the phone screen, `msg.*` in the status line,
  `automation.*` in the automation editor, `ai.*` in the prompt sent to the code-writing AI.
- Terminal width is limited. Keep `tui.status.*` and `tui.help.*` short.

//! A conversation as the panel beside the terminal shows it: the words from
//! the CLI's own record, each thing "the user" said matched to whoever really
//! sent it, and the waits and stops that happened in between.
//!
//! Everything here runs on a thread of its own: reading a record waits on the
//! disk, or on another machine. It reads the record of conversations over a
//! read-only connection (`db::Store::open_read`) and writes nothing to it; a
//! record found to be gone is handed back to the main loop to forget
//! ([`Found::Forget`]).
//!
//! **Newest first.** The panel shows the last thing said at the top and reads
//! further back as it is asked to, a page at a time (`reader::read_back`).
//! Opened at one place -- a search found it there -- it reads both ways from
//! that place (`reader::read_after` for what came next).
//!
//! **Who sent it.** A person's line in the record is matched to a send by the
//! fingerprint of how it begins (`db::head`), the send nearest in time winning
//! and each send used once ([`attribute`]). A line no send matches was typed
//! straight at the CLI, which only a person does.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use serde_json::{Value, json};

use super::db::{self, Send, Store};
use super::marks;
use crate::reader::{Record, Stretch, Turn, Who};

/// How many things said a page holds
pub const PAGE: usize = 30;

/// How far apart in time a send and the line it became may be. A message
/// typed while the AI works is written to the record when the AI gets to it;
/// a record on another machine is timed by that machine's clock
const NEAR_MS: i64 = 10 * 60 * 1000;

/// The most a search hands back, and the most pages it reads looking
const FIND_MOST: usize = 200;
const FIND_PAGES: usize = 400;
const FIND_PAGE: usize = 100;

/// What the panel is looking at
#[derive(Clone, Debug)]
pub struct Target {
    /// The panel that asked (the tab's id, or the conversation's), handed back
    /// so an answer for a panel since moved on can be told apart
    pub panel: String,
    /// The tab the conversation was carried on, by id: what the record of
    /// sends is kept against. `None` when no tab is known for it
    pub tab: Option<String>,
    /// The conversation the tab is on now, when the panel follows a tab
    pub live: Option<Record>,
    /// How the CLI's records are found, and the machine they are on: what
    /// makes a `Record` of every earlier conversation of the tab
    pub glob: String,
    pub machine: Option<crate::elsewhere::Elsewhere>,
    /// One conversation, opened by its id rather than followed in a tab
    pub past: Option<String>,
    /// The followed tab's local folder. Historical associations can have
    /// been written by another tab through an older shared CLI process.
    /// Explicitly opened records and the live record are still read as asked.
    pub cwd: Option<std::path::PathBuf>,
    /// Where the CLI records that folder, when its profile names the field.
    pub cwd_field: Option<String>,
}

/// What reading found: an answer for the panel, and records that are gone
pub struct Found {
    pub answer: Value,
    pub forget: Vec<(String, String)>,
}

/// One request from the panel, answered
pub fn answer(target: &Target, act: &str, args: &Value, db_path: &std::path::Path, marks_path: &std::path::Path) -> Found {
    let store = Store::open_read(db_path).ok();
    let mut ctx = Ctx { target, store: store.as_ref(), marks_path, forget: Vec::new(), marks: HashMap::new() };
    let req = args.get("req").cloned().unwrap_or(Value::Null);
    let answer = match act {
        "page" => ctx.page(args),
        "newer" => ctx.newer(args),
        "open" => ctx.open(args),
        "find" => ctx.find(args),
        "work" => ctx.work(args),
        "mark" => ctx.mark(args),
        other => Err(format!("no such request: {other}")),
    };
    let mut answer = match answer {
        Ok(v) => v,
        Err(e) => json!({"ok": false, "error": e}),
    };
    if let Some(o) = answer.as_object_mut() {
        o.entry("ok").or_insert(Value::Bool(true));
        o.insert("panel".into(), json!(target.panel));
        o.insert("act".into(), json!(act));
        o.insert("req".into(), req);
    }
    Found { answer, forget: ctx.forget }
}

/// The conversations to read, the newest first, and whether each ran without
/// asking
struct Chain {
    records: Vec<(Record, bool)>,
}

struct Ctx<'a> {
    target: &'a Target,
    store: Option<&'a Store>,
    marks_path: &'a std::path::Path,
    forget: Vec<(String, String)>,
    marks: HashMap<String, Vec<marks::Mark>>,
}

impl Ctx<'_> {
    fn chain(&self) -> Chain {
        let t = self.target;
        let named = |id: &str| Record::named(&t.glob, id, t.machine.clone());
        let mut records: Vec<(Record, bool)> = Vec::new();
        let known = match (self.store, t.tab.as_deref()) {
            (Some(s), Some(tab)) => s.conversations(tab).unwrap_or_default(),
            _ => Vec::new(),
        };
        if let Some(id) = &t.past {
            let yolo = known.iter().any(|c| &c.record_id == id && c.is_yolo);
            records.extend(named(id).map(|r| (r, yolo)));
            return Chain { records };
        }
        if let Some(live) = &t.live {
            let yolo = known.iter().any(|c| c.record_id == live.id() && c.is_yolo);
            records.push((live.clone(), yolo));
        }
        for c in &known {
            if records.iter().any(|(r, _)| r.id() == c.record_id) {
                continue;
            }
            if t.machine.is_none()
                && let Some(at) = crate::vault::record_folder(&t.glob, &c.record_id, t.cwd_field.as_deref())
            {
                let at = std::path::Path::new(&at);
                let matches = |p: &std::path::Path| crate::uistate::same_folder(at, p);
                // Judge a record against where its tab was then. For rows
                // from before folders were kept, other sightings of this uid
                // also establish its earlier places; the current folder alone
                // cannot distinguish a move from an old crossed association.
                let belongs = match c.observed_cwd.as_deref() {
                    Some(cwd) => matches(std::path::Path::new(cwd)),
                    None => t.cwd.as_deref().is_none_or(matches)
                        || known.iter().filter_map(|c| c.observed_cwd.as_deref()).any(|p| matches(std::path::Path::new(p))),
                };
                if !belongs {
                    // Keep the record available in all conversations.
                    continue;
                }
            }
            records.extend(named(&c.record_id).map(|r| (r, c.is_yolo)));
        }
        Chain { records }
    }

    fn marks_of(&mut self, record: &str) -> &[marks::Mark] {
        if !self.marks.contains_key(record) {
            let got = marks::on(self.marks_path, record).unwrap_or_default();
            self.marks.insert(record.to_string(), got);
        }
        &self.marks[record]
    }

    /// A record that is not there any more. Forgotten when it was an earlier
    /// conversation of a tab on this PC -- the live one is only not written yet
    fn gone(&mut self, record: &Record) {
        if let (Some(tab), Record::Here { id, .. }) = (&self.target.tab, record)
            && self.target.live.as_ref().is_none_or(|l| l.id() != id)
        {
            self.forget.push((tab.clone(), id.clone()));
        }
    }

    /// A page further back, or the newest one. The cursor is the conversation
    /// and the place in its record to read back from, and `until` -- the time
    /// the page before this one reached back to, so the waits and stops in
    /// between are shown once, on the page they fall in
    fn page(&mut self, args: &Value) -> Result<Value, String> {
        let chain = self.chain();
        if chain.records.is_empty() {
            return Ok(json!({"rows": [], "older": null, "empty": true}));
        }
        let cursor = args.get("older");
        let mut index = match cursor.and_then(|c| c.get("record")).and_then(Value::as_str) {
            Some(id) => chain.records.iter().position(|(r, _)| r.id() == id).ok_or("that conversation is not here any more")?,
            None => 0,
        };
        let mut before = cursor.and_then(|c| c.get("before")).and_then(Value::as_u64).unwrap_or(u64::MAX);
        let until = cursor.and_then(|c| c.get("until")).and_then(Value::as_i64).unwrap_or(i64::MAX);
        let want = args.get("want").and_then(Value::as_u64).map_or(PAGE, |n| (n as usize).clamp(1, 200));
        let mut said: Vec<(String, Turn)> = Vec::new();
        let mut works: Vec<(String, Stretch)> = Vec::new();
        let mut begins: Vec<(String, bool, Option<i64>)> = Vec::new();
        let mut older = Value::Null;
        // Read back until a page is full, crossing into an earlier
        // conversation when this one runs out
        loop {
            let (record, yolo) = &chain.records[index];
            // Part of this conversation was on the page before this one
            let continued = before != u64::MAX;
            let page = match record.page(before, want - said.len().min(want - 1)) {
                Some(Ok(p)) => p,
                Some(Err(e)) => return Err(e.to_string()),
                None => {
                    self.gone(record);
                    crate::reader::Page { turns: Vec::new(), from: 0, more: false, work: Vec::new() }
                }
            };
            let first_when = page.turns.first().and_then(|t| t.when);
            // A conversation nothing was said in -- a tab started and left, a
            // record not written yet -- has no start to mark
            let had_words = continued || !page.turns.is_empty();
            for t in page.turns.into_iter().rev() {
                said.push((record.id().to_string(), t));
            }
            works.extend(page.work.into_iter().map(|w| (record.id().to_string(), w)));
            if page.more {
                older = json!({"record": record.id(), "before": page.from});
                break;
            }
            if had_words {
                begins.push((record.id().to_string(), *yolo, first_when));
            }
            index += 1;
            before = u64::MAX;
            if index >= chain.records.len() {
                break;
            }
            if said.len() >= want {
                older = json!({"record": chain.records[index].0.id(), "before": u64::MAX});
                break;
            }
        }
        // Newest first as collected; the time this page reaches back to
        let reached_start = older.is_null();
        let lo = if reached_start { i64::MIN } else { said.iter().filter_map(|(_, t)| t.when).min().unwrap_or(until) };
        if let Some(o) = older.as_object_mut() {
            o.insert("until".into(), json!(lo));
        }
        said.reverse();
        let rows = self.rows(said, works, begins, lo, until);
        Ok(json!({"rows": rows, "older": older}))
    }

    /// What came after a place, read forwards (a conversation opened at a
    /// search's find, read on toward its end)
    fn newer(&mut self, args: &Value) -> Result<Value, String> {
        let chain = self.chain();
        let cursor = args.get("newer").ok_or("newer needs a place")?;
        let id = cursor.get("record").and_then(Value::as_str).ok_or("newer needs a conversation")?;
        let to = cursor.get("to").and_then(Value::as_u64).unwrap_or(0);
        let (record, _) = chain.records.iter().find(|(r, _)| r.id() == id).ok_or("that conversation is not here any more")?;
        let later = match record.after(to, PAGE, "") {
            Some(r) => r.map_err(|e| e.to_string())?,
            None => return Ok(json!({"rows": [], "newer": null})),
        };
        let lo = later.turns.first().and_then(|t| t.when).unwrap_or(i64::MAX);
        let hi = if later.more { later.turns.iter().filter_map(|t| t.when).max().unwrap_or(lo) } else { i64::MAX };
        let newer = json!({"record": id, "to": later.to, "end": !later.more});
        let said = later.turns.into_iter().map(|t| (id.to_string(), t)).collect();
        let works = later.work.into_iter().map(|w| (id.to_string(), w)).collect();
        let rows = self.rows(said, works, Vec::new(), lo.saturating_sub(1), hi);
        Ok(json!({"rows": rows, "newer": newer}))
    }

    /// One conversation opened at a place in it: what came just before it and
    /// what came after, the place among them
    fn open(&mut self, args: &Value) -> Result<Value, String> {
        let chain = self.chain();
        let (record, yolo) = chain.records.first().ok_or("this conversation's record was not found")?;
        let at = args.get("at").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let id = record.id().to_string();
        if at == u64::MAX {
            return self.page(args);
        }
        let back = match record.page(at, PAGE / 2) {
            Some(r) => r.map_err(|e| e.to_string())?,
            None => return Err("this conversation's record was not found".into()),
        };
        let later = match record.after(at, PAGE, "") {
            Some(r) => r.map_err(|e| e.to_string())?,
            None => crate::reader::Later { turns: Vec::new(), to: at, more: false, work: Vec::new() },
        };
        let mut begins = Vec::new();
        if !back.more {
            begins.push((id.clone(), *yolo, back.turns.first().and_then(|t| t.when)));
        }
        let whens = back.turns.iter().chain(later.turns.iter()).filter_map(|t| t.when);
        let lo = if back.more { whens.clone().min().unwrap_or(i64::MIN) } else { i64::MIN };
        let hi = if later.more { whens.max().unwrap_or(i64::MAX) } else { i64::MAX };
        let older = if back.more { json!({"record": id, "before": back.from, "until": lo}) } else { Value::Null };
        let newer = json!({"record": id, "to": later.to, "end": !later.more});
        let said = back.turns.into_iter().chain(later.turns).map(|t| (id.clone(), t)).collect();
        let works = back.work.into_iter().chain(later.work).map(|w| (id.clone(), w)).collect();
        let rows = self.rows(said, works, begins, lo, hi);
        Ok(json!({"rows": rows, "older": older, "newer": newer, "at": at}))
    }

    /// Everything said in the conversations the panel looks at that holds
    /// `q` (and is pinned, when `pins`), the newest first
    fn find(&mut self, args: &Value) -> Result<Value, String> {
        let q = args.get("q").and_then(Value::as_str).unwrap_or_default().trim().to_lowercase();
        let pins = args.get("pins").and_then(Value::as_bool).unwrap_or(false);
        if q.is_empty() && !pins {
            return Ok(json!({"rows": [], "capped": false}));
        }
        let turn = Searching::begin(&self.target.panel, args.get("viewer").and_then(Value::as_str).unwrap_or_default());
        let chain = self.chain();
        // Read forwards, each conversation from its start, so the work between
        // the things said is looked through too -- with the same rule the
        // search of every conversation uses (`reader::mention`): words said,
        // tools called, what they gave back; never the record's bookkeeping.
        // The newest of what is found is what is kept
        let mut found: Vec<(String, Turn)> = Vec::new();
        let mut works: Vec<(String, Stretch)> = Vec::new();
        let mut pages = 0;
        let mut capped = false;
        'records: for (record, _) in &chain.records {
            let pinned: HashSet<u64> = match pins {
                true => self.marks_of(record.id()).iter().filter(|m| m.pinned).map(|m| m.at).collect(),
                false => HashSet::new(),
            };
            // The notes written on this conversation hold words too: a thing
            // said is found by what was written about it, as the panel finds
            // it among what it has already read
            let noted: HashSet<u64> = match q.is_empty() {
                true => HashSet::new(),
                false => self
                    .marks_of(record.id())
                    .iter()
                    .filter(|m| crate::reader::find_in(&m.note, &q).is_some())
                    .map(|m| m.at)
                    .collect(),
            };
            if pins && pinned.is_empty() {
                continue;
            }
            let (mut here, mut here_work) = (Vec::new(), Vec::new());
            let mut from = 0u64;
            loop {
                if !turn.current() {
                    return Ok(json!({"stale": true}));
                }
                if pages >= FIND_PAGES {
                    capped = true;
                    break;
                }
                pages += 1;
                let later = match record.after(from, FIND_PAGE, &q) {
                    Some(Ok(l)) => l,
                    Some(Err(e)) => return Err(e.to_string()),
                    None => break,
                };
                for t in later.turns {
                    let at = t.at.unwrap_or(u64::MAX);
                    if pins && !pinned.contains(&at) {
                        continue;
                    }
                    if !q.is_empty() && crate::reader::find_in(&t.text, &q).is_none() && !noted.contains(&at) {
                        continue;
                    }
                    here.push((record.id().to_string(), t));
                }
                if !pins {
                    here_work.extend(later.work.into_iter().filter(|w| w.hit).map(|w| (record.id().to_string(), w)));
                }
                if !later.more || later.to <= from {
                    break;
                }
                from = later.to;
            }
            // Newest records first, and in each the newest found first: what
            // is kept when there is too much is the most recent
            found.splice(0..0, here);
            works.splice(0..0, here_work);
            if capped || found.len() + works.len() >= FIND_MOST {
                capped |= found.len() + works.len() > FIND_MOST;
                break 'records;
            }
        }
        let rows = self.rows(found, works, Vec::new(), i64::MAX, i64::MAX);
        let rows: Vec<Value> = rows.into_iter().take(FIND_MOST).collect();
        Ok(json!({"rows": rows, "capped": capped, "q": q, "pins": pins}))
    }

    /// One stretch of the AI's work opened: the tools it reached for, what they
    /// gave back, and what it said on the way, each cut to what is shown
    fn work(&mut self, args: &Value) -> Result<Value, String> {
        let chain = self.chain();
        let id = args.get("record").and_then(Value::as_str).ok_or("work needs a conversation")?;
        let from = args.get("from").and_then(Value::as_u64).ok_or("work needs a place")?;
        let to = args.get("to").and_then(Value::as_u64).unwrap_or(from);
        let q = args.get("q").and_then(Value::as_str).unwrap_or_default().trim().to_lowercase();
        let (record, _) = chain.records.iter().find(|(r, _)| r.id() == id).ok_or("that conversation is not here any more")?;
        match record.work(from, to, &q) {
            None => Err("this conversation's record was not found".into()),
            Some(Err(e)) => Err(e.to_string()),
            Some(Ok(work)) => Ok(json!({"record": id, "from": from, "work": work, "q": q})),
        }
    }

    /// Pin, unpin, or write the note on one thing said
    fn mark(&mut self, args: &Value) -> Result<Value, String> {
        let record = args.get("record").and_then(Value::as_str).ok_or("mark needs a conversation")?;
        let at = args.get("at").and_then(Value::as_u64).ok_or("mark needs a place")?;
        let pin = args.get("pin").and_then(Value::as_bool);
        let note = args.get("note").and_then(Value::as_str);
        let kept = marks::set(self.marks_path, record, at, pin, note)?;
        Ok(json!({
            "record": record,
            "at": at,
            "pin": kept.as_ref().is_some_and(|m| m.pinned),
            "note": kept.as_ref().map(|m| m.note.clone()).unwrap_or_default(),
        }))
    }

    /// The rows the panel draws, the newest first: what was said (oldest
    /// first in `said`), the work between (`works`), where each conversation
    /// began (`begins`), and the waits and stops that happened in `(lo, hi]`
    fn rows(
        &mut self,
        said: Vec<(String, Turn)>,
        works: Vec<(String, Stretch)>,
        begins: Vec<(String, bool, Option<i64>)>,
        lo: i64,
        hi: i64,
    ) -> Vec<Value> {
        // Who sent each person's line
        let sends: Vec<Send> = match (self.store, self.target.tab.as_deref()) {
            (Some(s), Some(tab)) => {
                let whens = said.iter().filter_map(|(_, t)| t.when);
                let (from, to) = match (whens.clone().min(), whens.max()) {
                    (Some(a), Some(b)) if lo != i64::MAX => (a.min(lo.max(a - NEAR_MS)) - NEAR_MS, b.max(hi.min(b + NEAR_MS)) + NEAR_MS),
                    _ => (i64::MIN, i64::MAX),
                };
                s.sends(tab, from, to).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        // Another session's message says who sent it already; the rest are
        // matched to what the app sent
        let persons: Vec<(&str, Option<i64>, &str)> = said
            .iter()
            .filter(|(_, t)| t.who == Who::You && t.peer.is_none())
            .map(|(record, t)| (record.as_str(), t.when, t.text.as_str()))
            .collect();
        let matched = attribute(&persons, &sends);
        let mut matched = matched.into_iter();

        // Every row with the time it is placed at; a line the record gave no
        // time takes the time of the line before it
        let mut keyed: Vec<((i64, u8, u64), Value)> = Vec::new();
        // A stretch of work has no time of its own: it is placed just after
        // the thing said before it in the same record
        let placed: Vec<(String, u64, i64)> = {
            let mut carried = i64::MIN;
            said.iter()
                .map(|(r, t)| {
                    carried = t.when.unwrap_or(carried);
                    (r.clone(), t.at.unwrap_or(0), carried)
                })
                .collect()
        };
        for (record, w) in works {
            let when = placed
                .iter()
                .rev()
                .find(|(r, at, _)| *r == record && *at < w.from)
                .map(|(_, _, when)| *when)
                .or_else(|| placed.iter().filter(|(r, _, _)| *r == record).map(|(_, _, when)| *when).next())
                .unwrap_or(i64::MIN);
            keyed.push((
                (when, 1, w.from),
                json!({"k": "work", "record": record, "from": w.from, "to": w.to, "calls": w.calls, "hit": w.hit}),
            ));
        }
        let mut carried = i64::MIN;
        for (record, t) in said {
            let when = t.when.unwrap_or(carried);
            carried = when;
            let from = match (t.who, &t.peer) {
                (Who::You, Some(name)) => Some(json!({"by": "session", "via": "peer", "sender": name})),
                (Who::You, None) => matched.next().flatten().map(|i| origin_json(&sends[i])),
                (Who::Ai, _) => None,
            };
            let at = t.at.unwrap_or(0);
            let mark = self.marks_of(&record).iter().find(|m| m.at == at).cloned();
            let mut row = json!({
                "k": "say",
                "who": t.who,
                "text": t.text,
                "record": record,
                "at": at,
                "when": t.when,
            });
            if let Some(f) = from {
                row["from"] = f;
            }
            if let Some(m) = mark {
                row["pin"] = json!(m.pinned);
                if !m.note.is_empty() {
                    row["note"] = json!(m.note);
                }
            }
            keyed.push(((when, 1, at), row));
        }
        for (record, yolo, first) in begins {
            let when = first.unwrap_or(i64::MIN);
            keyed.push(((when, 0, 0), json!({"k": "begin", "record": record, "when": first, "yolo": yolo})));
        }
        if let (Some(s), Some(tab)) = (self.store, self.target.tab.as_deref())
            && lo < hi
        {
            for w in s.waits(tab, lo, hi).unwrap_or_default() {
                keyed.push((
                    (w.started_at, 2, 0),
                    json!({
                        "k": "wait", "when": w.started_at, "ended": w.ended_at, "answered": w.answered_at,
                        "by": w.by, "device": w.device, "via": w.via,
                    }),
                ));
            }
            for st in s.stops(tab, lo, hi).unwrap_or_default() {
                keyed.push((
                    (st.stopped_at, 2, 0),
                    json!({
                        "k": "stop", "when": st.stopped_at, "by": st.by, "device": st.device,
                        "how": st.how, "job": st.job, "why": st.why,
                    }),
                ));
            }
        }
        keyed.sort_by_key(|k| std::cmp::Reverse(k.0));
        keyed.into_iter().map(|(_, v)| v).collect()
    }
}

/// Who sent each of a person's lines in the conversation `record_id`: for
/// each line (its time and words, as the reader read them), where it came
/// from -- `{by, device, via, sender, job}` -- or `None` for a line no send
/// matches, which a person typed straight at the CLI. For the search of every
/// conversation, which reads the records itself
pub fn origins(record_id: &str, lines: &[(Option<i64>, &str)]) -> Vec<Option<Value>> {
    let Ok(store) = Store::open_read(&super::path()) else { return vec![None; lines.len()] };
    let Some(tab) = store.tab_of(record_id).ok().flatten() else { return vec![None; lines.len()] };
    let sends = store.sends(&tab, i64::MIN, i64::MAX).unwrap_or_default();
    let lines: Vec<_> = lines.iter().map(|(when, text)| (record_id, *when, *text)).collect();
    attribute(&lines, &sends).into_iter().map(|m| m.map(|i| origin_json(&sends[i]))).collect()
}

fn origin_json(s: &Send) -> Value {
    json!({"by": s.by, "device": s.device, "via": s.via, "sender": s.sender, "job": s.job})
}

/// Which send each of a person's lines was, in the order the lines are given
/// (their record, time where the record says, and words). A line matches a
/// send whose fingerprints include how the line begins and that was sent near
/// enough in time; where several could, the nearest in time wins, and no send
/// is used twice. A line with no time is matched to the earliest send left
/// that fits it
pub fn attribute(lines: &[(&str, Option<i64>, &str)], sends: &[Send]) -> Vec<Option<usize>> {
    let mut pairs: Vec<(u64, usize, usize)> = Vec::new();
    for (i, (record, when, text)) in lines.iter().enumerate() {
        let Some(h) = db::head(text) else { continue };
        for (j, s) in sends.iter().enumerate() {
            if s.record_id.as_deref().is_some_and(|id| id != *record) || !s.heads.contains(&h) {
                continue;
            }
            let apart = match when {
                Some(w) => w.abs_diff(s.sent_at),
                // Not known: after every pair that is, earliest send first
                None => u64::MAX / 2 + j as u64,
            };
            if when.is_some() && apart > NEAR_MS as u64 {
                continue;
            }
            pairs.push((apart, i, j));
        }
    }
    pairs.sort();
    let mut out = vec![None; lines.len()];
    let mut used = vec![false; sends.len()];
    for (_, i, j) in pairs {
        if out[i].is_none() && !used[j] {
            out[i] = Some(j);
            used[j] = true;
        }
    }
    out
}

/// A search a viewer is waiting for. A newer one from that viewer's panel makes
/// an older one stop reading: only the last thing typed is being looked for
struct Searching {
    key: (String, String),
    turn: u64,
}

static SEARCHES: Mutex<Option<HashMap<(String, String), u64>>> = Mutex::new(None);
static SEARCH_TURN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Searching {
    fn begin(panel: &str, viewer: &str) -> Self {
        let mut g = SEARCHES.lock().unwrap_or_else(|e| e.into_inner());
        let key = (panel.to_string(), viewer.to_string());
        let turn = SEARCH_TURN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        g.get_or_insert_with(HashMap::new).insert(key.clone(), turn);
        Searching { key, turn }
    }

    fn current(&self) -> bool {
        let g = SEARCHES.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().and_then(|m| m.get(&self.key)).is_some_and(|n| *n == self.turn)
    }
}

impl Drop for Searching {
    fn drop(&mut self) {
        let mut g = SEARCHES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = g.as_mut()
            && m.get(&self.key) == Some(&self.turn)
        {
            m.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convo::db::{By, Device, Origin, Stop};

    fn send(at: i64, text: &str, by: &str) -> Send {
        Send {
            id: 0,
            tab: "t".into(),
            record_id: None,
            sent_at: at,
            by: by.into(),
            device: None,
            via: "composer".into(),
            sender: None,
            job: None,
            heads: db::heads(&[text]),
            chars: text.chars().count() as i64,
        }
    }

    #[test]
    fn each_line_is_matched_to_the_nearest_send_that_fits_and_each_send_once() {
        let sends = vec![send(1_000, "yes", "person"), send(50_000, "yes", "job"), send(60_000, "fix it", "tab")];
        let lines = [("r", Some(51_000), "yes"), ("r", Some(2_000), "yes"), ("r", Some(70_000), "fix  it"), ("r", Some(80_000), "typed at the CLI")];
        assert_eq!(attribute(&lines, &sends), vec![Some(1), Some(0), Some(2), None]);
        // Too far apart in time is not the same thing sent
        assert_eq!(attribute(&[("r", Some(1_000 + NEAR_MS + 1), "yes")], &sends[..1]), vec![None]);
        // No time: the earliest send left
        assert_eq!(attribute(&[("r", None, "yes"), ("r", None, "yes")], &sends), vec![Some(0), Some(1)]);
    }

    #[test]
    fn two_viewers_can_search_one_panel_without_cancelling_each_other() {
        let a = Searching::begin("shared-panel-test", "desktop");
        let b = Searching::begin("shared-panel-test", "phone");
        assert!(a.current());
        assert!(b.current());
        let newer = Searching::begin("shared-panel-test", "desktop");
        assert!(!a.current());
        assert!(newer.current());
        assert!(b.current());
    }

    fn line(when: &str, who: &str, text: &str) -> String {
        format!(
            "{{\"timestamp\":\"{when}\",\"message\":{{\"role\":\"{who}\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n"
        )
    }

    fn ms(when: &str) -> i64 {
        crate::limits::epoch_ms_of(when).unwrap()
    }

    struct Place {
        dir: std::path::PathBuf,
    }

    impl Place {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("convo-read-{name}-{}-{}", std::process::id(), db::now_ms()));
            std::fs::create_dir_all(dir.join("records")).unwrap();
            Place { dir }
        }
        fn record(&self, id: &str, lines: &[String]) {
            std::fs::write(self.dir.join("records").join(format!("{id}.jsonl")), lines.concat()).unwrap();
        }
        fn glob(&self) -> String {
            format!("{}/records/{{id}}.jsonl", self.dir.display()).replace('\\', "/")
        }
        fn db(&self) -> std::path::PathBuf {
            self.dir.join("conversations.db")
        }
        fn marks(&self) -> std::path::PathBuf {
            self.dir.join("marks.json")
        }
        fn target(&self, live: &str) -> Target {
            Target {
                // Parallel tests are separate app instances and viewers.
                panel: self.dir.to_string_lossy().into_owned(),
                tab: Some("t".into()),
                live: Record::named(&self.glob(), live, None),
                glob: self.glob(),
                machine: None,
                past: None,
                cwd: None,
                cwd_field: None,
            }
        }
    }

    impl Drop for Place {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The whole of it: two conversations one tab carried on, what was sent
    /// by whom, a question answered from the phone and a stop -- read back
    /// newest first, a page at a time, across the /clear between them
    #[test]
    fn a_tab_s_conversations_read_back_as_one_with_who_sent_what() {
        let p = Place::new("chain");
        p.record("old", &[line("2026-09-28T01:00:00Z", "user", "first job"), line("2026-09-28T01:01:00Z", "assistant", "done one")]);
        p.record(
            "new",
            &[
                line("2026-09-28T02:00:00Z", "user", "brief for t2"),
                line("2026-09-28T02:05:00Z", "assistant", "working"),
                line("2026-09-28T02:10:00Z", "user", "typed straight in"),
                line("2026-09-28T02:11:00Z", "assistant", "ok"),
            ],
        );
        {
            let mut s = Store::open(&p.db()).unwrap();
            s.seen("t", "claude", "old", false, ms("2026-09-28T01:00:00Z")).unwrap();
            s.seen("t", "claude", "new", true, ms("2026-09-28T02:00:00Z")).unwrap();
            s.sent("t", Some("old"), &["first job"], &Origin::person(Device::Phone, "composer"), ms("2026-09-28T00:59:59Z"))
                .unwrap();
            s.sent("t", Some("new"), &["brief for t2"], &Origin::job(Some(3), Some("lead"), "brief"), ms("2026-09-28T01:59:58Z"))
                .unwrap();
            s.state("t", "QUESTION", ms("2026-09-28T02:06:00Z")).unwrap();
            s.touched("t", &Origin::person(Device::Phone, "keys"), ms("2026-09-28T02:07:00Z")).unwrap();
            s.state("t", "BUSY", ms("2026-09-28T02:07:01Z")).unwrap();
            s.stopped("t", &Stop { by: By::Person, device: Some(Device::Window), how: "esc", job: None, why: None }, ms("2026-09-28T02:12:00Z"))
                .unwrap();
        }
        let t = p.target("new");
        let first = answer(&t, "page", &json!({"want": 2}), &p.db(), &p.marks()).answer;
        let kinds = |v: &Value| v["rows"].as_array().unwrap().iter().map(|r| format!("{}:{}", r["k"].as_str().unwrap(), r["text"].as_str().or(r["how"].as_str()).unwrap_or(""))).collect::<Vec<_>>();
        assert_eq!(kinds(&first), vec!["stop:esc", "say:ok", "say:typed straight in"]);
        assert!(first["rows"][2].get("from").is_none(), "typed at the CLI: nobody sent it through the app");
        let second = answer(&t, "page", &json!({"want": 2, "older": first["older"]}), &p.db(), &p.marks()).answer;
        assert_eq!(kinds(&second), vec!["wait:", "say:working", "say:brief for t2", "begin:"]);
        let wait = &second["rows"][0];
        assert_eq!((wait["by"].as_str(), wait["device"].as_str()), (Some("person"), Some("phone")));
        let brief = &second["rows"][2]["from"];
        assert_eq!((brief["by"].as_str(), brief["job"].as_i64(), brief["sender"].as_str()), (Some("job"), Some(3), Some("lead")));
        assert_eq!(second["rows"][3]["yolo"], json!(true));
        let third = answer(&t, "page", &json!({"want": 2, "older": second["older"]}), &p.db(), &p.marks()).answer;
        assert_eq!(kinds(&third), vec!["say:done one", "say:first job", "begin:"]);
        assert_eq!(third["rows"][1]["from"]["device"].as_str(), Some("phone"));
        assert!(third["older"].is_null(), "the start of everything the tab said");
    }

    #[test]
    fn a_crossed_history_does_not_import_another_folders_words() {
        let p = Place::new("crossed-history");
        let mine = crate::local_path("D:/my-project");
        let other = crate::local_path("D:/other-project");
        for (id, cwd, text) in [("old", &mine, "my earlier request"), ("foreign", &other, "someone else's request"), ("live", &mine, "my current request")] {
            let meta = format!("{}\n", json!({"type": "session_meta", "payload": {"cwd": cwd}}));
            p.record(id, &[meta, line("2026-09-28T02:00:00Z", "user", text)]);
        }
        {
            let s = Store::open(&p.db()).unwrap();
            for (i, id) in ["old", "foreign", "live"].iter().enumerate() {
                s.seen("t", "codex", id, false, i as i64).unwrap();
            }
        }
        let mut target = p.target("live");
        target.cwd = Some(mine.into());
        target.cwd_field = Some("payload.cwd".into());
        let found = answer(&target, "page", &json!({}), &p.db(), &p.marks());
        let rows = found.answer["rows"].as_array().unwrap();
        assert!(rows.iter().any(|r| r["text"] == "my current request"));
        assert!(rows.iter().any(|r| r["text"] == "my earlier request"));
        assert!(!rows.iter().any(|r| r["record"] == "foreign"), "{}", found.answer);
        assert!(found.forget.is_empty(), "the other conversation must not be deleted");
        let search = answer(&target, "find", &json!({"q": "someone"}), &p.db(), &p.marks());
        assert_eq!(search.answer["rows"], json!([]));
        target.past = Some("foreign".into());
        assert!(got_all(&target, &p)["rows"].as_array().unwrap().iter().any(|r| r["text"] == "someone else's request"), "opening that record explicitly still works");
        target.past = None;
        target.live = Record::named(&p.glob(), "foreign", None);
        assert!(got_all(&target, &p)["rows"].as_array().unwrap().iter().any(|r| r["text"] == "someone else's request"), "an explicitly resumed live conversation is preserved");
    }

    #[test]
    fn moving_a_tab_keeps_its_history_and_search_without_importing_crossed_records() {
        let p = Place::new("moved-history");
        let before = crate::local_path("D:/before");
        let after = crate::local_path("D:/after");
        let other = crate::local_path("D:/other");
        let rows = [("legacy", &before), ("old", &before), ("foreign", &other), ("live", &after)];
        {
            let s = Store::open(&p.db()).unwrap();
            for (i, (id, cwd)) in rows.iter().enumerate() {
                let meta = format!("{}\n", json!({"type":"session_meta", "payload":{"cwd":cwd}}));
                p.record(id, &[meta, line("2026-09-28T02:00:00Z", "user", &format!("request {id}"))]);
                let observed = match *id { "legacy" => None, "live" => Some(after.as_str()), _ => Some(before.as_str()) };
                s.seen_in("t", "codex", id, false, i as i64, observed).unwrap();
            }
        }
        let mut target = p.target("live");
        target.cwd = Some(after.into());
        target.cwd_field = Some("payload.cwd".into());
        for act in ["page", "find"] {
            let found = answer(&target, act, &json!({"q":"request"}), &p.db(), &p.marks());
            let rows = found.answer["rows"].as_array().unwrap();
            for id in ["legacy", "old", "live"] {
                assert!(rows.iter().any(|r| r["record"] == id), "{act} lost {id}: {}", found.answer);
            }
            assert!(!rows.iter().any(|r| r["record"] == "foreign"), "{act} imported another tab's record");
            assert!(found.forget.is_empty());
        }
    }

    #[test]
    fn the_same_words_in_another_record_do_not_change_the_sender() {
        let p = Place::new("sender-record");
        let at = ms("2026-09-28T02:00:00Z");
        p.record("new", &[line("2026-09-28T02:00:00Z", "user", "continue")]);
        {
            let s = Store::open(&p.db()).unwrap();
            s.seen("t", "codex", "new", false, at).unwrap();
            s.sent("t", Some("old"), &["continue"], &Origin::job(Some(3), Some("lead"), "brief"), at-1).unwrap();
            s.sent("t", Some("new"), &["continue"], &Origin::person(Device::Phone, "composer"), at-1000).unwrap();
        }
        let got = got_all(&p.target("new"), &p);
        let said = got["rows"].as_array().unwrap().iter().find(|r| r["k"] == "say").unwrap();
        assert_eq!(said["from"]["by"], "person", "{got}");
    }

    /// A tab started again and again with nothing said is a conversation a
    /// start at a time, with no record behind any of them: the panel says
    /// nothing was said, not "a conversation began" once for each, and the
    /// ones before the live one are forgotten
    #[test]
    fn a_conversation_nothing_was_said_in_is_not_a_conversation() {
        let p = Place::new("empty");
        {
            let s = Store::open(&p.db()).unwrap();
            s.seen("t", "claude", "gone-1", false, 10).unwrap();
            s.seen("t", "claude", "gone-2", false, 20).unwrap();
            s.seen("t", "claude", "live", false, 30).unwrap();
        }
        let found = answer(&p.target("live"), "page", &json!({}), &p.db(), &p.marks());
        assert_eq!(found.answer["rows"], json!([]), "{}", found.answer);
        assert!(found.answer["older"].is_null());
        let mut forgotten: Vec<&str> = found.forget.iter().map(|(_, r)| r.as_str()).collect();
        forgotten.sort();
        assert_eq!(forgotten, vec!["gone-1", "gone-2"], "the live one is only not written yet");
    }

    /// Every row of a tab's newest page
    fn got_all(t: &Target, p: &Place) -> Value {
        answer(t, "page", &json!({"want": 200}), &p.db(), &p.marks()).answer
    }

    /// Where the thing said with these words begins
    fn found_at(page: &Value, text: &str) -> u64 {
        page["rows"].as_array().unwrap().iter().find(|r| r["text"] == json!(text)).unwrap()["at"].as_u64().unwrap()
    }

    #[test]
    fn a_search_finds_what_holds_the_words_and_a_pin_is_found_by_itself() {
        let p = Place::new("find");
        p.record(
            "a",
            &[
                line("2026-09-28T02:00:00Z", "user", "Fix the parser"),
                line("2026-09-28T02:01:00Z", "assistant", "The PARSER is fixed"),
                line("2026-09-28T02:02:00Z", "user", "thanks"),
            ],
        );
        let t = p.target("a");
        let got = answer(&t, "find", &json!({"q": "parser"}), &p.db(), &p.marks()).answer;
        let texts: Vec<&str> = got["rows"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap()).collect();
        assert_eq!(texts, vec!["The PARSER is fixed", "Fix the parser"], "the newest first, whatever the case");
        let at = got["rows"][1]["at"].as_u64().unwrap();
        let marked = answer(&t, "mark", &json!({"record": "a", "at": at, "pin": true}), &p.db(), &p.marks()).answer;
        assert_eq!(marked["pin"], json!(true));
        let pinned = answer(&t, "find", &json!({"pins": true}), &p.db(), &p.marks()).answer;
        assert_eq!(pinned["rows"].as_array().unwrap().len(), 1);
        assert_eq!(pinned["rows"][0]["text"], json!("Fix the parser"));
        assert_eq!(pinned["rows"][0]["pin"], json!(true));
        // A word only in a note finds the thing said it was written on
        let thanks = found_at(&got_all(&t, &p), "thanks");
        answer(&t, "mark", &json!({"record": "a", "at": thanks, "note": "Closed the Zanzibar ticket"}), &p.db(), &p.marks());
        let by_note = answer(&t, "find", &json!({"q": "zanzibar"}), &p.db(), &p.marks()).answer;
        assert_eq!(by_note["rows"].as_array().unwrap().len(), 1, "{by_note}");
        assert_eq!((by_note["rows"][0]["text"].as_str(), by_note["rows"][0]["note"].as_str()), (Some("thanks"), Some("Closed the Zanzibar ticket")));
    }

    #[test]
    fn a_conversation_opened_at_a_place_reads_both_ways_from_it() {
        let p = Place::new("open");
        let lines: Vec<String> = (0..100)
            .map(|i| {
                let when = format!("2026-09-28T{:02}:{:02}:00Z", 2 + i / 60, i % 60);
                line(&when, if i % 2 == 0 { "user" } else { "assistant" }, &format!("line {i}"))
            })
            .collect();
        p.record("a", &lines);
        let at: u64 = lines[..20].iter().map(|l| l.len() as u64).sum();
        let t = Target { past: Some("a".into()), live: None, ..p.target("a") };
        let got = answer(&t, "open", &json!({"at": at}), &p.db(), &p.marks()).answer;
        let rows = got["rows"].as_array().unwrap();
        assert!(rows.iter().any(|r| r["at"].as_u64() == Some(at) && r["text"] == json!("line 20")), "the place itself is there");
        assert!(!got["older"].is_null() && got["newer"]["end"] == json!(false), "and more both ways");
        let shown = |v: &Value| v["rows"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let opened = shown(&got);
        assert_eq!((opened.first().unwrap().as_str(), opened.last().unwrap().as_str()), ("line 49", "line 5"));
        let on = answer(&t, "newer", &json!({"newer": got["newer"]}), &p.db(), &p.marks()).answer;
        let next = shown(&on);
        assert_eq!((next.first().unwrap().as_str(), next.last().unwrap().as_str()), ("line 79", "line 50"), "on from where it stopped");
        let last = answer(&t, "newer", &json!({"newer": on["newer"]}), &p.db(), &p.marks()).answer;
        assert_eq!(shown(&last).first().unwrap(), "line 99");
        assert_eq!(last["newer"]["end"], json!(true));
    }
}

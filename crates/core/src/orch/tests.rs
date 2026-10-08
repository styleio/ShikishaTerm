//! The commands, played through with made-up tabs: a lead, its workers, and
//! the ways a chain of work goes wrong.

use super::*;
use std::sync::mpsc::{Receiver, channel};

/// Who the made-up tab called `id` is
fn uid(id: &str) -> String {
    format!("uid-{id}")
}

fn tab(id: &str, state: TabState) -> TabFact {
    TabFact {
        id: id.to_string(),
        uid: uid(id),
        called: format!("called-{id}"),
        title: id.to_string(),
        ai: true,
        cli: if id.starts_with("codex") { "codex".into() } else { "claude".into() },
        state,
        folder: Some("/work/repo".into()),
        far: false,
        reachable: true,
        bridge: false,
        incarnation: Some(1),
        typed_request: id.starts_with("claude"),
    }
}

struct World {
    o: Orchestra,
    scene: Scene,
    named: HashSet<String>,
    limits: Limits,
}

impl World {
    fn new() -> Self {
        Self {
            o: Orchestra::in_memory(),
            scene: Scene {
                tabs: vec![tab("lead", TabState::Busy), tab("claude", TabState::Done), tab("codex", TabState::Done)],
            },
            named: ["claude", "codex"].iter().map(|s| s.to_string()).collect(),
            limits: Limits { max_assignments: 40, max_depth: 1 },
        }
    }

    fn set(&mut self, id: &str, state: TabState) {
        self.scene.tabs.iter_mut().find(|t| t.id == id).unwrap().state = state;
    }

    /// A call from `from`, and its answer if it came at once
    fn send(&mut self, from: &str, method: &str, params: Vec<Value>) -> (Receiver<Result<Value, String>>, Vec<Effect>) {
        let (tx, rx) = channel();
        // Named by who each is, as the loop settles them
        let named: HashSet<String> = self.named.iter().map(|n| uid(n)).collect();
        let fx = self.o.call(
            Call {
                caller: Some(format!("called-{from}")),
                incarnation: Some(1),
                method: method.into(),
                params,
                reply: tx,
            },
            &self.scene,
            &named,
            self.limits,
        );
        (rx, fx)
    }

    fn ok(&mut self, from: &str, method: &str, params: Vec<Value>) -> Value {
        let (rx, _) = self.send(from, method, params);
        rx.try_recv().expect("answered at once").unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    fn err(&mut self, from: &str, method: &str, params: Vec<Value>) -> String {
        let (rx, _) = self.send(from, method, params);
        rx.try_recv().expect("answered at once").expect_err("refused")
    }

    /// Dispatch and play the tab starting on it
    fn assign(&mut self, task: &str, to: &str) -> Value {
        let (rx, _) = self.send("lead", "assign", vec![json!(task), json!(to)]);
        assert!(rx.try_recv().is_err(), "held until the tab has taken it");
        let fx = self.o.tick(&self.scene);
        assert!(
            fx.iter().any(|e| matches!(e, Effect::Type { tab, text, .. } if *tab == uid(to) && text.contains("--- t"))),
            "{fx:?}"
        );
        self.set(to, TabState::Busy);
        self.o.tick(&self.scene);
        rx.try_recv().unwrap().unwrap()
    }
}

#[test]
fn the_whole_loop_implement_review_fix_close() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("ship the parser fix")]);
    let t1 = w.ok("lead", "task_add", vec![json!("Implement the fix in parser.rs. Done when cargo test passes.")]);
    assert_eq!(t1["task"], "t1");
    let d = w.assign("t1", "claude");
    assert_eq!(d["assignment"], "a1");
    assert_eq!(d["next"][0], "shikisha inbox wait");

    // The lead waits; nothing yet
    let (rx, _) = w.send("lead", "inbox", vec![json!("wait")]);
    assert!(rx.try_recv().is_err());

    // The worker reports; the lead's wait is answered with it
    let r = w.ok("claude", "report", vec![json!("done"), json!("Fixed it."), json!("Tests pass."), json!("nothing")]);
    assert_eq!(r["state"], "reported");
    w.o.tick(&w.scene);
    let got = rx.try_recv().unwrap().unwrap();
    assert_eq!(got["mail"][0]["kind"], "report");
    let handover = got["handover"].as_i64().unwrap();

    // A second report is harmless
    assert_eq!(w.ok("claude", "report", vec![json!("done"), json!("again")])["state"], "already reported");

    // Review, then close
    w.ok("lead", "task_add", vec![json!("Review commit abc on branch fix."), json!({"deps": ["t1"]})]);
    w.ok("lead", "inbox", vec![json!({"dealt": handover})]);
    w.assign("t2", "codex");
    w.ok("codex", "report", vec![json!("done"), json!("Reviewed."), json!("No findings."), json!("nothing")]);

    // Loose ends: both tabs still undecided, inbox unread
    let e = w.err("lead", "job_close", vec![json!("done")]);
    assert!(e.contains("let_go a1") && e.contains("let_go a2") && e.contains("inbox"), "{e}");
    let b = w.ok("lead", "inbox", vec![]);
    w.ok("lead", "inbox", vec![json!({"dealt": b["handover"]})]);
    // Neither tab was opened by this job: released means kept, not closed
    let (rx, fx) = w.send("lead", "let_go", vec![json!("a1")]);
    assert_eq!(rx.try_recv().unwrap().unwrap()["closed"], false);
    assert!(fx.is_empty());
    w.ok("lead", "keep", vec![json!("a2")]);
    assert_eq!(w.ok("lead", "job_close", vec![json!("done")])["state"], "closed");
}

#[test]
fn a_report_from_a_restarted_tab_is_not_the_one_asked_for() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    let (tx, rx) = channel();
    w.o.call(
        Call {
            caller: Some("called-claude".into()),
            incarnation: Some(2),
            method: "report".into(),
            params: vec![json!("done"), json!("s")],
            reply: tx,
        },
        &w.scene,
        &w.named,
        w.limits,
    );
    assert!(rx.try_recv().unwrap().unwrap_err().contains("earlier run"));
}

#[test]
fn a_report_must_say_how_it_went() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    let e = w.err("claude", "report", vec![json!("it went ok"), json!("s")]);
    assert!(e.contains("done or failed") && e.contains("shikisha report"), "{e}");
}

#[test]
fn only_named_or_opened_tabs_are_given_work() {
    let mut w = World::new();
    w.named.clear();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    let e = w.err("lead", "assign", vec![json!("t1"), json!("claude")]);
    assert!(e.contains("has not named") && e.contains("open_ai_tab"), "{e}");
    // A tab this job opened is its own
    w.o.opened(Some("called-lead"), &w.scene, "claude", &uid("claude"));
    w.assign("t1", "claude");
}

/// The person named a tab that was then closed, and another opened under its
/// id: the new one is not the one named
#[test]
fn a_tab_given_a_named_tabs_id_is_not_named() {
    let mut w = World::new();
    w.named.clear();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    // The tab now called `claude` is somebody new
    w.scene.tabs.iter_mut().find(|t| t.id == "claude").unwrap().uid = "uid-claude-new".into();
    let (tx, rx) = channel();
    w.o.call(
        Call {
            caller: Some("called-lead".into()),
            incarnation: Some(1),
            method: "assign".into(),
            params: vec![json!("t1"), json!("claude")],
            reply: tx,
        },
        &w.scene,
        &[uid("claude")].into_iter().collect(),
        w.limits,
    );
    let e = rx.try_recv().unwrap().unwrap_err();
    assert!(e.contains("has not named"), "{e}");
}

#[test]
fn a_lead_cannot_assign_to_itself_or_to_a_terminal() {
    let mut w = World::new();
    w.named.insert("lead".into());
    w.named.insert("shell".into());
    let mut sh = tab("shell", TabState::Done);
    sh.ai = false;
    w.scene.tabs.push(sh);
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    assert!(w.err("lead", "assign", vec![json!("t1"), json!("lead")]).contains("itself"));
    assert!(w.err("lead", "assign", vec![json!("t1"), json!("<@shell>")]).contains("tab_run"));
}

#[test]
fn a_busy_tab_gets_the_brief_when_it_is_free_and_a_waiting_one_never() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.ok("lead", "task_add", vec![json!("b")]);
    w.set("claude", TabState::Busy);
    let (rx, _) = w.send("lead", "assign", vec![json!("t1"), json!("claude")]);
    assert!(w.o.tick(&w.scene).is_empty(), "nothing typed into a busy tab");
    w.set("claude", TabState::Done);
    let fx = w.o.tick(&w.scene);
    // Claude is told in typed words first, then handed the paste
    assert!(matches!(&fx[0], Effect::Type { lead: Some(l), .. } if l == text::LEAD_LINE));
    w.set("claude", TabState::Busy);
    w.o.tick(&w.scene);
    assert!(rx.try_recv().unwrap().is_ok());

    w.set("codex", TabState::Question);
    let (rx, _) = w.send("lead", "assign", vec![json!("t2"), json!("codex")]);
    w.o.tick(&w.scene);
    let e = rx.try_recv().unwrap().unwrap_err();
    assert!(e.contains("approve"), "{e}");
    // Nothing was tried, so nothing failed: the task is open again
    assert_eq!(w.o.store().unwrap().task(2).unwrap().unwrap().state, "open");
}

#[test]
fn codex_is_handed_the_paste_alone() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    let (_rx, _) = w.send("lead", "assign", vec![json!("t1"), json!("codex")]);
    let fx = w.o.tick(&w.scene);
    assert!(matches!(&fx[0], Effect::Type { lead: None, .. }), "{fx:?}");
}

#[test]
fn a_question_reaches_the_lead_and_its_answer_the_worker() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    let (ask, _) = w.send("claude", "ask_lead", vec![json!("Which branch?"), json!({"choices": ["main", "dev"]})]);
    assert!(ask.try_recv().is_err(), "held until answered");
    let inbox = w.ok("lead", "inbox", vec![]);
    assert_eq!(inbox["mail"][0]["kind"], "question");
    let q = inbox["mail"][0]["question"].as_str().unwrap().to_string();
    assert!(inbox["next"][0].as_str().unwrap().contains(&format!("answer {q}")));
    w.ok("lead", "answer", vec![json!(q), json!("dev")]);
    w.o.tick(&w.scene);
    assert_eq!(ask.try_recv().unwrap().unwrap()["answer"], "dev");
}

#[test]
fn a_question_outlives_its_wait_and_is_resumed() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    let (ask, _) = w.send("claude", "ask_lead", vec![json!("Which?"), json!({"wait_ms": 0})]);
    w.o.tick(&w.scene);
    let v = ask.try_recv().unwrap().unwrap();
    assert_eq!(v["state"], "NOTHING YET");
    assert!(v["next"][0].as_str().unwrap().contains("resume"));
    w.ok("lead", "answer", vec![json!("q1"), json!("this one")]);
    let v = w.ok("claude", "ask_lead", vec![json!({"resume": "q1"})]);
    assert_eq!(v["answer"], "this one");
}

#[test]
fn a_tab_with_mail_and_nothing_to_do_is_told_once() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    w.ok("claude", "report", vec![json!("failed"), json!("Could not build."), json!("Linker error."), json!("A fix for the linker.")]);
    // The lead is busy: nothing typed
    assert!(w.o.tick(&w.scene).is_empty());
    w.set("lead", TabState::Done);
    let fx = w.o.tick(&w.scene);
    assert_eq!(
        fx,
        vec![Effect::Type { tab: uid("lead"), lead: None, text: text::mail_line(1) }]
    );
    assert!(w.o.tick(&w.scene).is_empty(), "told once");
}

#[test]
fn a_lead_waiting_on_its_inbox_is_not_told_through_the_screen() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    w.set("lead", TabState::Done);
    let (rx, _) = w.send("lead", "inbox", vec![json!("wait")]);
    w.ok("claude", "report", vec![json!("done"), json!("s")]);
    let fx = w.o.tick(&w.scene);
    assert!(fx.is_empty(), "{fx:?}");
    assert!(rx.try_recv().unwrap().is_ok());
}

#[test]
fn a_worker_that_dies_is_reported_and_its_task_is_open_again() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    w.set("claude", TabState::Exited);
    w.o.last_watch = None;
    w.o.tick(&w.scene);
    let inbox = w.ok("lead", "inbox", vec![]);
    assert_eq!(inbox["mail"][0]["kind"], "alert");
    assert!(inbox["mail"][0]["text"].as_str().unwrap().contains("assign t1"));
    assert_eq!(w.o.store().unwrap().task(1).unwrap().unwrap().state, "open");
}

#[test]
fn a_worker_waiting_for_the_person_tells_the_lead_and_the_person_once() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    w.set("claude", TabState::Question);
    w.o.last_watch = None;
    let fx = w.o.tick(&w.scene);
    assert!(fx.iter().any(|e| matches!(e, Effect::Person { .. })));
    w.o.last_watch = None;
    let fx = w.o.tick(&w.scene);
    assert!(!fx.iter().any(|e| matches!(e, Effect::Person { .. })), "once");
}

#[test]
fn nesting_stops_at_the_depth_allowed() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    // The worker tries to lead a job of its own
    let e = w.err("claude", "job_open", vec![json!("sub")]);
    assert!(e.contains("yourself"), "{e}");
    w.limits.max_depth = 2;
    w.ok("claude", "job_open", vec![json!("sub")]);
}

#[test]
fn the_number_of_assignments_is_capped() {
    let mut w = World::new();
    w.limits.max_assignments = 1;
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.ok("lead", "task_add", vec![json!("b")]);
    w.assign("t1", "claude");
    let e = w.err("lead", "assign", vec![json!("t2"), json!("codex")]);
    assert!(e.contains("used 1 of its 1"), "{e}");
}

#[test]
fn a_decision_for_the_person_holds_the_task_and_the_answer_reaches_the_lead() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("merge to main")]);
    let (rx, fx) = w.send("lead", "decision_open", vec![json!("t1"), json!("Merge to main?"), json!({"choices": ["yes", "no"], "to": "person"})]);
    rx.try_recv().unwrap().unwrap();
    assert!(fx.iter().any(|e| matches!(e, Effect::Person { .. })));
    assert!(w.err("lead", "decision_make", vec![json!("d1"), json!("yes")]).contains("person's to decide"));
    assert!(w.err("lead", "assign", vec![json!("t1"), json!("claude")]).contains("held"));
    w.o.make_decision(1, "yes", "person").unwrap();
    let inbox = w.ok("lead", "inbox", vec![]);
    assert_eq!(inbox["mail"][0]["kind"], "decision");
    assert_eq!(inbox["mail"][0]["choice"], "yes");
}

#[test]
fn notes_reach_one_worker_or_all_of_them() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.ok("lead", "task_add", vec![json!("b")]);
    w.assign("t1", "claude");
    w.assign("t2", "codex");
    assert_eq!(w.ok("lead", "tell", vec![json!("workers"), json!("stop touching main.rs")])["sent"].as_array().unwrap().len(), 2);
    assert_eq!(w.ok("lead", "tell", vec![json!("workers:codex"), json!("x")])["sent"], json!(["a2"]));
    let got = w.ok("codex", "inbox", vec![]);
    assert_eq!(got["mail"].as_array().unwrap().len(), 2);
    // A worker's note goes to its lead
    w.ok("claude", "tell", vec![json!("lead"), json!("halfway")]);
    assert!(w.o.store().unwrap().unread("job:1").unwrap().iter().any(|m| m.body == "halfway"));
}

#[test]
fn stopping_a_worker_it_opened_closes_the_tab_if_it_will_not_stop() {
    let mut w = World::new();
    w.named.clear();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.o.opened(Some("called-lead"), &w.scene, "claude", &uid("claude"));
    w.assign("t1", "claude");
    let (rx, fx) = w.send("lead", "stop", vec![json!("a1")]);
    rx.try_recv().unwrap().unwrap();
    assert_eq!(fx, vec![Effect::Esc { tab: uid("claude") }]);
    assert_eq!(w.o.store().unwrap().task(1).unwrap().unwrap().state, "held");
    // Still busy after the grace: closed
    w.o.stopping[0].since -= STOP_GRACE;
    w.o.last_watch = None;
    let fx = w.o.tick(&w.scene);
    assert!(fx.contains(&Effect::Close { tab: uid("claude") }), "{fx:?}");
}

#[test]
fn a_tab_the_person_typed_into_is_not_closed_by_the_job() {
    let mut w = World::new();
    w.named.clear();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.o.opened(Some("called-lead"), &w.scene, "claude", &uid("claude"));
    w.assign("t1", "claude");
    w.ok("claude", "report", vec![json!("done"), json!("s")]);
    w.o.person_typed(&uid("claude"));
    let (rx, fx) = w.send("lead", "let_go", vec![json!("a1")]);
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["closed"], false);
    assert!(v["kept_because"].as_str().unwrap().contains("person"));
    assert!(fx.is_empty());
}

#[test]
fn every_answer_says_what_to_do_next() {
    let mut w = World::new();
    assert!(w.ok("lead", "job_open", vec![json!("x")])["next"][0].as_str().unwrap().starts_with("shikisha task_add"));
    assert!(w.ok("lead", "task_add", vec![json!("a")])["next"][0].as_str().unwrap().starts_with("shikisha assign t1"));
    let s = w.ok("lead", "job_status", vec![]);
    assert!(s["next"][0].as_str().unwrap().starts_with("shikisha assign t1"), "{s}");
    // A refusal says how to put it right
    let e = w.err("lead", "assign", vec![json!("t1")]);
    assert!(e.contains("shikisha assign t1 <tab>"), "{e}");
}

#[test]
fn a_tab_that_leads_nothing_is_told_how_to_start() {
    let mut w = World::new();
    let e = w.err("lead", "task_add", vec![json!("a")]);
    assert!(e.contains("shikisha job_open"), "{e}");
}

#[test]
fn a_far_tab_whose_machine_is_agreed_to_is_woken_and_briefed_once_its_bridge_is_up() {
    let mut w = World::new();
    let mut far = tab("remote", TabState::Wait);
    far.far = true;
    far.reachable = false;
    far.bridge = true;
    w.scene.tabs.push(far);
    w.named.insert("remote".into());
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    let (rx, _) = w.send("lead", "assign", vec![json!("t1"), json!("remote")]);
    let fx = w.o.tick(&w.scene);
    assert_eq!(fx, vec![Effect::Wake { tab: uid("remote") }], "its machine is opened, nothing typed yet");
    assert!(w.o.tick(&w.scene).is_empty(), "opened once");
    w.scene.tabs.iter_mut().find(|t| t.id == "remote").unwrap().reachable = true;
    let fx = w.o.tick(&w.scene);
    assert!(fx.iter().any(|e| matches!(e, Effect::Type { tab, .. } if *tab == uid("remote"))), "{fx:?}");
    w.set("remote", TabState::Busy);
    w.o.tick(&w.scene);
    assert!(rx.try_recv().unwrap().is_ok());
}

#[test]
fn far_tabs_without_the_relay_are_refused_with_the_way_to_install_it() {
    let mut w = World::new();
    let mut far = tab("remote", TabState::Done);
    far.far = true;
    far.reachable = false;
    w.scene.tabs.push(far);
    w.named.insert("remote".into());
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    let e = w.err("lead", "assign", vec![json!("t1"), json!("remote")]);
    assert!(e.contains("bridge") && e.contains("ask_tab"), "{e}");
}

#[test]
fn a_second_wait_on_an_inbox_takes_over_from_one_left_behind() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    let (first, _) = w.send("lead", "inbox", vec![json!("wait")]);
    let (second, _) = w.send("lead", "inbox", vec![json!("wait")]);
    assert!(first.try_recv().unwrap().unwrap_err().contains("took over"));
    assert!(second.try_recv().is_err(), "the new one is waiting");
}

#[test]
fn a_worker_that_reports_before_it_is_seen_starting_has_done_the_work() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    let (rx, _) = w.send("lead", "assign", vec![json!("t1"), json!("claude")]);
    w.o.tick(&w.scene);
    // The report arrives while the tab still reads as it did before
    assert_eq!(w.ok("claude", "report", vec![json!("done"), json!("done fast")])["state"], "reported");
    w.set("claude", TabState::Busy);
    w.o.tick(&w.scene);
    let v = rx.try_recv().unwrap().expect("the assign is answered as done, not refused");
    assert!(v["why"].as_str().unwrap().contains("already reported"), "{v}");
    assert_eq!(w.o.store().unwrap().task(1).unwrap().unwrap().state, "done");
}

#[test]
fn a_report_keeps_what_was_done_found_and_left_apart() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    // Said in whatever words an AI reaches for, positionally or by name
    w.ok("claude", "report", vec![json!("succeeded"), json!({"did": "Fixed sum()", "found": "avg() is wrong too", "left": "avg()"})]);
    let t = w.o.store().unwrap().task(1).unwrap().unwrap();
    let r = t.result.unwrap();
    assert_eq!((r["outcome"].as_str(), r["did"].as_str(), r["left"].as_str()), (Some("done"), Some("Fixed sum()"), Some("avg()")));
    let mail = w.ok("lead", "inbox", vec![]);
    let text = mail["mail"][0]["text"].as_str().unwrap();
    assert!(text.contains("Did: Fixed sum()") && text.contains("Found: avg() is wrong too") && text.contains("Left: avg()"), "{text}");
}

#[test]
fn a_report_without_what_was_done_says_how_to_write_one() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    let e = w.err("claude", "report", vec![json!("done")]);
    assert!(e.contains("shikisha report done \"<what you did>\""), "{e}");
}

#[test]
fn a_job_with_work_left_undone_does_not_close() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.ok("lead", "task_add", vec![json!("b"), json!({"waits_on": ["t1"]})]);
    let e = w.err("lead", "job_close", vec![json!("done")]);
    assert!(e.contains("t1 has not been handed") && e.contains("shikisha assign t1"), "{e}");
    assert!(e.contains("t2 waits on t1") && e.contains("task_drop t2"), "{e}");
    // Taken out on purpose, with why: then it closes
    let v = w.ok("lead", "task_drop", vec![json!("t1"), json!("the person fixed it by hand")]);
    assert!(v["note"].as_str().unwrap().contains("t2"), "{v}");
    assert!(w.err("lead", "task_drop", vec![json!("t1")]).contains("why"));
    w.ok("lead", "task_drop", vec![json!("t2"), json!("nothing to review")]);
    assert_eq!(w.ok("lead", "job_close", vec![json!("nothing was needed")])["state"], "closed");
}

#[test]
fn a_failed_or_stopped_task_is_tried_again_by_assigning_it_again() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    w.ok("claude", "report", vec![json!("failed"), json!("Could not build.")]);
    w.ok("lead", "let_go", vec![json!("a1")]);
    w.set("claude", TabState::Done);
    w.assign("t1", "claude");
    let (rx, _) = w.send("lead", "stop", vec![json!("a2")]);
    rx.try_recv().unwrap().unwrap();
    assert_eq!(w.o.store().unwrap().task(1).unwrap().unwrap().state, "held");
    w.set("claude", TabState::Done);
    w.assign("t1", "claude");
    assert_eq!(w.o.store().unwrap().task(1).unwrap().unwrap().state, "working");
}

#[test]
fn the_person_stopping_a_job_ends_it() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.ok("lead", "task_add", vec![json!("b")]);
    w.assign("t1", "claude");
    let (wait, _) = w.send("lead", "inbox", vec![json!("wait")]);
    let fx = w.o.stop_job(1, &w.scene);
    assert!(fx.contains(&Effect::Esc { tab: uid("claude") }), "{fx:?}");
    // The wait it was holding hears of it at once
    let v = wait.try_recv().unwrap().unwrap();
    assert_eq!(v["state"], "STOPPED");
    // Nothing more can be handed out in it, and its card is gone
    assert!(w.err("lead", "assign", vec![json!("t2"), json!("codex")]).contains("closed"));
    assert_eq!(w.o.board(&w.scene), json!([]));
    assert!(w.err("lead", "task_add", vec![json!("c")]).contains("job_open"));
}

#[test]
fn a_lead_not_waiting_hears_of_the_stop_in_its_own_mail() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.o.stop_job(1, &w.scene);
    let v = w.ok("lead", "inbox", vec![]);
    assert_eq!(v["mail"][0]["kind"], "alert");
    assert!(v["mail"][0]["text"].as_str().unwrap().contains("start a new job"), "{v}");
}

#[test]
fn every_command_answers_with_what_to_do_next() {
    let mut w = World::new();
    let mut said: Vec<(String, Value)> = Vec::new();
    let mut ok = |w: &mut World, from: &str, m: &str, p: Vec<Value>| {
        let v = w.ok(from, m, p);
        said.push((m.to_string(), v.clone()));
        v
    };
    ok(&mut w, "lead", "job_open", vec![json!("x")]);
    ok(&mut w, "lead", "task_add", vec![json!("a")]);
    ok(&mut w, "lead", "task_add", vec![json!("b")]);
    ok(&mut w, "lead", "task_list", vec![]);
    ok(&mut w, "lead", "job_status", vec![]);
    w.assign("t1", "claude");
    ok(&mut w, "lead", "tell", vec![json!("a1"), json!("hi")]);
    ok(&mut w, "claude", "tell", vec![json!("lead"), json!("halfway")]);
    ok(&mut w, "claude", "inbox", vec![json!("peek")]);
    ok(&mut w, "claude", "inbox", vec![]);
    ok(&mut w, "claude", "report", vec![json!("done"), json!("did it")]);
    ok(&mut w, "lead", "inbox", vec![json!("peek")]);
    let h = ok(&mut w, "lead", "inbox", vec![]);
    ok(&mut w, "lead", "inbox", vec![json!({"dealt": h["handover"]})]);
    ok(&mut w, "lead", "keep", vec![json!("a1")]);
    ok(&mut w, "lead", "decision_open", vec![json!("t2"), json!("go on?"), json!({"choices": ["yes", "no"]})]);
    ok(&mut w, "lead", "decision_make", vec![json!("d1"), json!("no")]);
    ok(&mut w, "lead", "task_drop", vec![json!("t2"), json!("not needed")]);
    let h = ok(&mut w, "lead", "inbox", vec![]);
    ok(&mut w, "lead", "inbox", vec![json!({"dealt": h["handover"]})]);
    ok(&mut w, "lead", "let_go", vec![json!("a1")]);
    ok(&mut w, "lead", "job_close", vec![json!("done")]);
    for (m, v) in &said {
        assert!(v["next"].as_array().is_some_and(|n| !n.is_empty()), "{m} answered without next: {v}");
    }
    let seen: HashSet<&str> = said.iter().map(|(m, _)| m.as_str()).collect();
    for m in METHODS {
        // Held commands are answered later, and checked where they are played
        let later = ["assign", "ask_lead", "answer", "stop"].contains(&m);
        assert!(seen.contains(m) || later, "{m} is not checked for next here");
    }
}

#[test]
fn a_stopped_tab_given_new_work_is_not_closed_for_the_old() {
    let mut w = World::new();
    w.named.clear();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.ok("lead", "task_add", vec![json!("b")]);
    w.o.opened(Some("called-lead"), &w.scene, "claude", &uid("claude"));
    w.assign("t1", "claude");
    let (rx, _) = w.send("lead", "stop", vec![json!("a1")]);
    rx.try_recv().unwrap().unwrap();
    // Within the grace, the lead hands the same tab its next task
    w.set("claude", TabState::Done);
    w.assign("t2", "claude");
    w.o.stopping[0].since -= STOP_GRACE;
    w.o.last_watch = None;
    let fx = w.o.tick(&w.scene);
    assert!(!fx.contains(&Effect::Close { tab: uid("claude") }), "{fx:?}");
}

#[test]
fn a_stopped_tab_whose_next_task_is_already_done_is_not_closed_either() {
    let mut w = World::new();
    w.named.clear();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.ok("lead", "task_add", vec![json!("b")]);
    w.o.opened(Some("called-lead"), &w.scene, "claude", &uid("claude"));
    w.assign("t1", "claude");
    let (rx, _) = w.send("lead", "stop", vec![json!("a1")]);
    rx.try_recv().unwrap().unwrap();
    w.set("claude", TabState::Done);
    w.assign("t2", "claude");
    // Quick work: reported within the grace, and the tab is still busy
    // writing its last words when the grace is up
    w.ok("claude", "report", vec![json!("done"), json!("did it")]);
    w.set("claude", TabState::Busy);
    w.o.stopping[0].since -= STOP_GRACE;
    w.o.last_watch = None;
    let fx = w.o.tick(&w.scene);
    assert!(!fx.contains(&Effect::Close { tab: uid("claude") }), "{fx:?}");
}

#[test]
fn a_decision_is_not_asked_in_a_job_the_person_stopped() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.o.stop_job(1, &w.scene);
    let (rx, fx) = w.send("lead", "decision_open", vec![json!("t1"), json!("merge?"), json!({"to": "person"})]);
    assert!(rx.try_recv().unwrap().unwrap_err().contains("closed"));
    assert!(fx.is_empty(), "nobody is notified of a question with nowhere to answer it");
}

/// A tab that draws the name of a tab closed before it is somebody else: it
/// leads none of the first one's jobs, is on none of its tasks and reads none
/// of its mail -- and what is said about the first still says its name
#[test]
fn a_tab_given_a_closed_tabs_name_is_handed_none_of_its_work() {
    let mut w = World::new();
    w.ok("lead", "job_open", vec![json!("x")]);
    w.ok("lead", "task_add", vec![json!("a")]);
    w.assign("t1", "claude");
    w.o.mail_tab(&uid("claude"), "codex", "a reply", "for the first claude");
    // Both close; two new tabs draw their names
    for t in w.scene.tabs.iter_mut().filter(|t| t.id == "lead" || t.id == "claude") {
        t.uid = format!("{}-again", t.uid);
    }
    let e = w.err("lead", "task_add", vec![json!("b")]);
    assert!(e.contains("leads no open job"), "{e}");
    let e = w.err("claude", "report", vec![json!("done"), json!("s")]);
    assert!(e.contains("no assignment"), "{e}");
    let v = w.ok("claude", "inbox", vec![]);
    assert!(!v.to_string().contains("for the first claude"), "{v}");
    // The first claude is gone: its task is lost, said with its name
    let fx = w.o.tick(&w.scene);
    assert!(fx.iter().all(|e| !matches!(e, Effect::Type { tab, .. } if *tab == uid("claude-again"))), "{fx:?}");
    let board = w.o.board(&w.scene);
    assert_eq!(board[0]["lead"], uid("lead"), "{board}");
}

/// The command that opens a tab in a folder writes a Windows folder with `/`,
/// which survives the shell of an AI on Windows; a server's folder is left
/// as it is
#[test]
fn a_tab_is_opened_in_a_folder_written_the_way_a_shell_keeps_it() {
    let back = char::from(92).to_string();
    let windows = ["C:", "work", "repo", "fix-x"].join(&back);
    assert_eq!(open_in(&windows), r#"shikisha open_ai_tab <claude|codex|gemini> '{"folder":"C:/work/repo/fix-x"}'"#);
    let share = format!("{back}{back}server{back}projects{back}x");
    assert_eq!(open_in(&share), r#"shikisha open_ai_tab <claude|codex|gemini> '{"folder":"//server/projects/x"}'"#);
    assert_eq!(open_in("/home/me/x"), r#"shikisha open_ai_tab <claude|codex|gemini> '{"folder":"/home/me/x"}'"#);
}

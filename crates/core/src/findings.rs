//! What the records say, in words, for the first page of a bug report.
//!
//! The records answer "what was wrong" only to somebody who reads every
//! line of them, and the person asking has a customer waiting. So they are
//! read here, when the report is made: each stretch of time in which
//! something was off, what it was, which programs were behind it, and what
//! that most likely means -- with what the records cannot say stated as
//! plainly as what they can, so nobody takes a silence for an all-clear.
//!
//! Nothing here looks at the machine. It is given text and gives text back.

use crate::health::{Look, Seen};

/// Looks further apart than this are two stretches of trouble, not one
const APART: u64 = 150;
/// The most stretches of trouble told one by one, the newest
const TOLD: usize = 12;
/// The most pieces of slow work listed
const SLOW_TOLD: usize = 20;

fn size(b: u64) -> String {
    crate::pressure::size(b)
}

/// A stretch of looks that each found something off
struct Stretch<'a> {
    looks: Vec<&'a Seen>,
}

/// What one heading of the named programs came to over a stretch: each
/// program, in how many looks it was named, and the most it was seen using
fn named(looks: &[&Seen], heading: &str) -> Vec<(String, usize, String)> {
    // The amount is the last word of an item, or the last two ("1.6 GB")
    let words = if heading == "most cpu" { 1 } else { 2 };
    let amount = |s: &str| -> f64 {
        let n: f64 = s.split([' ', '%']).next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
        if s.ends_with("GB") { n * 1024.0 } else { n }
    };
    let mut all: Vec<(String, usize, String)> = Vec::new();
    for seen in looks {
        for (_, items) in seen.named.iter().filter(|(h, _)| h == heading) {
            for item in items {
                let parts: Vec<&str> = item.rsplitn(words + 1, ' ').collect();
                if parts.len() != words + 1 {
                    continue;
                }
                let value = parts[..words].iter().rev().cloned().collect::<Vec<_>>().join(" ");
                // The copies are counted in the name ("chrome x23"), and the
                // count changes from look to look: the program is the same
                let name = parts[words].rsplit_once(" x").filter(|(_, n)| n.parse::<u32>().is_ok()).map_or(parts[words], |(n, _)| n).to_string();
                match all.iter_mut().find(|(n, _, _)| *n == name) {
                    Some(known) => {
                        known.1 += 1;
                        if amount(&value) > amount(&known.2) {
                            known.2 = value;
                        }
                    }
                    None => all.push((name, 1, value)),
                }
            }
        }
    }
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| amount(&b.2).partial_cmp(&amount(&a.2)).unwrap_or(std::cmp::Ordering::Equal)));
    all.truncate(5);
    all
}

/// The most the app and what it started were seen using over a stretch: of
/// the processors, in hundredths, and of the memory, in bytes
fn own(looks: &[&Seen]) -> Option<(f32, u64, bool)> {
    let mut most: Option<(f32, u64, bool)> = None;
    for seen in looks {
        for (heading, items) in seen.named.iter().filter(|(h, _)| h.starts_with("this app's own")) {
            // Counted with what programs share, the copies add up to more than they hold
            let shared = heading != "this app's own";
            let cpu = items.iter().find_map(|i| i.strip_prefix("cpu ")).and_then(|v| v.trim_end_matches('%').parse::<f32>().ok()).unwrap_or(0.0);
            let memory = items.iter().find_map(|i| i.strip_prefix("memory ")).and_then(crate::health::unsize).unwrap_or(0);
            let was = most.unwrap_or((0.0, 0, false));
            most = Some((was.0.max(cpu), was.1.max(memory), was.2 || shared));
        }
    }
    most
}

fn told(list: &[(String, usize, String)], of: usize) -> String {
    list.iter().map(|(name, times, most)| format!("{name} (in {times} of {of} looks, up to {most})")).collect::<Vec<_>>().join(", ")
}

/// One stretch, told
fn stretch(s: &Stretch) -> String {
    let looks: Vec<&Look> = s.looks.iter().filter_map(|l| l.look.as_ref()).collect();
    let n = looks.len();
    let (first, last) = (s.looks[0], s.looks[n - 1]);
    let mut out = match n {
        1 => format!("{} (one look)\n", first.when),
        // (a clock put back in between makes the last look the earlier one)
        _ => format!("{} to {} ({n} looks, {} min)\n", first.when, last.when, last.at.saturating_sub(first.at) / 60 + 1),
    };
    let mut say = |text: String| out.push_str(&format!("  {text}\n"));
    let short: Vec<&&Look> = looks.iter().filter(|l| l.short_of_memory()).collect();
    if !short.is_empty() {
        let least = short.iter().min_by_key(|l| l.free).map(|l| format!("{} free of {}", size(l.free), size(l.total))).unwrap_or_default();
        let commit = looks.iter().filter_map(|l| l.commit).min_by_key(|c| c.0).map(|(left, most)| format!("; of what the system can promise, as little as {} left of {}", size(left), size(most))).unwrap_or_default();
        say(format!("memory was short in {} of {n} looks: as little as {least}{commit}", short.len()));
    }
    let most = |of: fn(&Look) -> Option<f32>| looks.iter().filter_map(|l| of(l)).fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))));
    let paged = most(|l| l.paged);
    if looks.iter().any(|l| l.paging()) {
        say(format!("memory was being read back from the disk: up to {:.0} pages a second", paged.unwrap_or(0.0)));
    }
    if looks.iter().any(|l| l.disk_busy()) {
        say(format!("the disk was busy all the time: up to {:.0}% of it, with up to {:.1} requests waiting", most(|l| l.busy).unwrap_or(0.0), most(|l| l.queue).unwrap_or(0.0)));
    }
    let busy = looks.iter().filter(|l| l.busy_processors()).count();
    if busy > 0 {
        let kernel = most(|l| l.kernel).map(|k| format!(" (up to {k:.0}% of it in the system itself)")).unwrap_or_default();
        say(format!("the processors were nearly all in use in {busy} of {n} looks: up to {:.0}%{kernel}", looks.iter().map(|l| l.cpu).fold(0.0, f32::max)));
    }
    let late = [looks.iter().map(|l| l.late[0]).max().unwrap_or(0), looks.iter().map(|l| l.late[1]).max().unwrap_or(0)];
    let was_late = looks.iter().any(|l| l.late_app());
    if was_late {
        say(format!("the app answered late: its work part by up to {:.1} s, its window part by up to {:.1} s", late[0] as f64 / 1000.0, late[1] as f64 / 1000.0));
    }
    for (heading, label) in [("most cpu", "using the processors most"), ("most memory", "holding the most memory"), ("most committed", "promised the most memory"), ("most io", "reading and writing most (files, pipes and the network together)")] {
        let list = named(&s.looks, heading);
        if !list.is_empty() {
            say(format!("{label}: {}", told(&list, n)));
        }
    }
    let ours = own(&s.looks);
    if let Some((cpu, memory, shared)) = ours {
        let how = if shared { " (counted with what its programs share, so more than they hold)" } else { "" };
        say(format!("this app and everything it started: up to {cpu:.1}% of the processors and {} of memory{how}", size(memory)));
    }
    // What it most likely means
    let (memory, paging, disk) = (!short.is_empty(), looks.iter().any(|l| l.paging()), looks.iter().any(|l| l.disk_busy()));
    let measured = looks.iter().any(|l| l.paged.is_some());
    let mut reading: Vec<String> = Vec::new();
    // At one and the same look: a machine short of memory at one, and
    // reading pages for a program that starts at the next, did not run out
    let ran_out = looks.iter().any(|l| l.short_of_memory() && l.paging());
    let short_and_busy = looks.iter().any(|l| l.short_of_memory() && l.disk_busy());
    if ran_out {
        reading.push("the machine ran out of memory and was reading it back from its disk; every program on it is slow while that lasts, whichever one a person is looking at".into());
    } else if short_and_busy {
        reading.push("memory was short and the disk was busy at the same time, but the record does not show memory being read back from the disk: the two may have nothing to do with each other, and the programs reading and writing most are named above".into());
    } else if memory && !measured {
        reading.push("memory was short. This part of the record was written by a version that did not yet note the disk or the paging, so it cannot say whether the machine was slowed by it".into());
    } else if memory {
        reading.push("memory was short, but the machine was not working its disk for it: tight, not yet slowed".into());
    }
    if paging && !ran_out {
        reading.push("memory was being read back from the disk while the record shows memory free: a large program was starting, or coming back after being idle".into());
    }
    if disk && !ran_out && !short_and_busy {
        reading.push("the disk could not keep up with what was asked of it; the programs reading and writing most are named above".into());
    }
    if busy > 0 {
        let first = named(&s.looks, "most cpu").into_iter().next().map(|(n, _, most)| format!("; {n} used the most, up to {most} of the whole machine")).unwrap_or_default();
        reading.push(format!("the processors were the limit{first}"));
    }
    if was_late && !memory && !disk && busy == 0 && !measured {
        reading.push("the app answered late while the processors and the memory had room. This part of the record was written by a version that did not yet note the disk, so a disk that could not keep up is not ruled out".into());
    } else if was_late && !memory && !disk && busy == 0 {
        // The app writes down where a part of it was only once it has been
        // silent for a good while (`crate::watchdog`)
        let where_ = if late.iter().any(|l| *l >= 20_000) { "hang.json says which part, and names the dump that holds where it was" } else { "it was not late for long enough for the app to write down where it was" };
        reading.push(format!("the app answered late while the machine had room to spare: the fault is in the app itself; {where_}"));
    } else if was_late {
        reading.push("the app answered late while the machine was short; it may be no more than the machine".into());
    }
    if let (Some((_, memory, false)), Some(look)) = (ours, looks.iter().min_by_key(|l| l.free)) {
        let used = look.total.saturating_sub(look.free);
        if used > 0 && memory.saturating_mul(2) >= used {
            reading.push("half or more of the memory in use was this app's and what it started".into());
        }
    }
    for r in reading {
        say(format!("reading: {r}"));
    }
    out
}

/// What the week's record of the machine says (`crate::health`)
pub fn of_week(health: &str) -> String {
    let all: Vec<Seen> = health.lines().filter_map(crate::health::seen).collect();
    let looks: Vec<&Seen> = all.iter().filter(|s| s.look.is_some()).collect();
    let mut out = String::from("== The week's record of the machine (health.log) ==\n");
    let (Some(first), Some(last)) = (all.first(), all.last()) else {
        out.push_str("There is none: the app has not run since a version that keeps it was installed, or its data was cleared.\n");
        return out;
    };
    let off: Vec<&Seen> = looks.iter().filter(|s| s.look.as_ref().is_some_and(|l| l.off())).cloned().collect();
    out.push_str(&format!(
        "It runs from {} to {}: {} looks, {} of them with something off.\nNothing before {} is in it; what happened earlier cannot be told from this report.\n",
        first.when,
        last.when,
        looks.len(),
        off.len(),
        first.when
    ));
    let starts: Vec<&Seen> = all.iter().filter(|s| s.other.starts_with("started ")).collect();
    if !starts.is_empty() {
        let last: Vec<String> = starts.iter().rev().take(6).rev().map(|s| format!("{} ({})", s.when, s.other.trim_start_matches("started "))).collect();
        out.push_str(&format!("The app was started {} times in it; the last: {}.\n", starts.len(), last.join(", ")));
    }
    if let Some(tight) = looks.iter().filter_map(|s| s.look.as_ref()).filter(|l| l.total > 0).min_by_key(|l| l.free) {
        let roomy = looks.iter().filter_map(|s| s.look.as_ref()).max_by_key(|l| l.free).map_or(0, |l| l.free);
        out.push_str(&format!("Memory: {} in all; between {} and {} of it free over the record.\n", size(tight.total), size(tight.free), size(roomy)));
    }
    if let Some((left, most)) = looks.iter().filter_map(|s| s.look.as_ref()?.commit).min_by_key(|c| c.0) {
        out.push_str(&format!("Of the memory the system can promise ({}), as little as {} was left.\n", size(most), size(left)));
    }
    if let Some((free, total)) = looks.iter().rev().find_map(|s| s.look.as_ref()?.disk) {
        out.push_str(&format!("The disk the app is on: {} free of {}.\n", size(free), size(total)));
    }
    if off.is_empty() {
        out.push_str("\nNo look found the machine short of anything or the app answering late.\n");
    } else {
        let mut stretches: Vec<Stretch> = Vec::new();
        for seen in off {
            match stretches.last_mut() {
                Some(s) if seen.at.saturating_sub(s.looks[s.looks.len() - 1].at) <= APART => s.looks.push(seen),
                _ => stretches.push(Stretch { looks: vec![seen] }),
            }
        }
        out.push_str(&format!("\n{} stretches of trouble, the newest first{}:\n\n", stretches.len(), if stretches.len() > TOLD { format!(" (the newest {TOLD} told)") } else { String::new() }));
        for s in stretches.iter().rev().take(TOLD) {
            out.push_str(&stretch(s));
            out.push('\n');
        }
    }
    let slow: Vec<&Seen> = all.iter().filter(|s| s.other.starts_with("slow: ")).collect();
    if !slow.is_empty() {
        out.push_str(&format!("Work of the app's own that took long ({}; the newest {SLOW_TOLD} at most):\n", slow.len()));
        for s in slow.iter().rev().take(SLOW_TOLD) {
            out.push_str(&format!("  {} {}\n", s.when, s.other.trim_start_matches("slow: ")));
        }
    }
    out
}

/// A line of the app's log without the time it was written at
pub(crate) fn words(line: &str) -> &str {
    match line.strip_prefix('[').and_then(|l| l.split_once("] ")) {
        Some((time, rest)) if time.trim().parse::<f64>().is_ok() => rest,
        _ => line,
    }
}

/// What the app's log says that is worth a sentence: the things known to
/// stop the app working, and the lines it wrote over and over
pub fn of_log(hooks: &str) -> String {
    let mut out = String::from("== The app's log (hooks.log) ==\n");
    let lines: Vec<&str> = hooks.lines().map(words).collect();
    const NO_LINE: &str = "this PC's resident process: no line to it (";
    const STILL: &str = "this PC's resident process: still no line to it after ";
    let unreached: Vec<&&str> = lines.iter().filter(|l| l.starts_with(NO_LINE) || l.starts_with(STILL)).collect();
    if let Some(last) = unreached.last() {
        let why = last.split_once('(').map_or("", |(_, w)| w.strip_suffix(')').unwrap_or(w));
        out.push_str(&format!("The app could not reach this PC's resident process, the one that holds its terminals (the last reason: {why}).\n  No terminal on this PC opens while that lasts.\n"));
        if why.contains("keep.key is not there") {
            out.push_str("  A resident process is answering, and its folder is gone: the app's data was cleared, or the app put in again, while a resident process from before went on running.\n  Ending every copy of this program in the task manager, or restarting the PC, and then starting the app clears it.\n");
        }
    }
    let kept = lines.iter().filter(|l| l.starts_with("could not remove ")).count();
    if kept > 0 {
        out.push_str(&format!("A folder was asked to be removed and stayed, {kept} times; the lines beginning \"could not remove\" say which file held it.\n"));
    }
    // The same thing said over and over is either the trouble or its trace
    // (a log put in a report has a run of the same line said once, with a
    // line under it for how many more there were: those count for the line)
    let mut said: std::collections::HashMap<String, usize> = Default::default();
    let mut before = String::new();
    for l in &lines {
        if let Some(more) = l.trim_start().strip_prefix("(the same ").and_then(|r| r.split(' ').next()).and_then(|n| n.parse::<usize>().ok()) {
            *said.entry(before.clone()).or_default() += more;
            continue;
        }
        if l.trim().is_empty() {
            continue;
        }
        before = l.chars().take(90).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
        *said.entry(before.clone()).or_default() += 1;
    }
    let mut often: Vec<(String, usize)> = said.into_iter().filter(|(_, n)| *n >= 20).collect();
    often.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if !often.is_empty() {
        out.push_str("Said most often (a # stands for a number):\n");
        for (shape, n) in often.iter().take(8) {
            out.push_str(&format!("  {n} times: {shape}\n"));
        }
    }
    if out.lines().count() == 1 {
        out.push_str("Nothing in it is known to stop the app working, and nothing is said over and over.\n");
    }
    out
}

/// What the machine was doing at the moment the report was made
pub fn of_now(look: &Look, top: &crate::health::Top) -> String {
    let mut out = String::from("== At the moment this report was made (now.txt) ==\n");
    out.push_str(&format!("{}\n", crate::health::line(look, Some(top)).replace(" | ", "\n  ")));
    // How late the app answered is the most since the record last looked,
    // up to a minute ago: it is not of this moment, and is not read against it
    if look.late_app() {
        out.push_str(&format!(
            "A part of the app answered late at some point in the last minute (its work part by up to {:.1} s, its window part by up to {:.1} s). Whether the machine was short then is in the week's record, not here.\n",
            look.late[0] as f64 / 1000.0,
            look.late[1] as f64 / 1000.0
        ));
    }
    let look = &Look { late: [0, 0], ..look.clone() };
    if !look.off() {
        out.push_str("Nothing was off at that moment. If the trouble was not happening then, the week's record below is what says when it was.\n");
    } else {
        let now = Seen { at: 0, when: "now".into(), look: Some(look.clone()), named: crate::health::seen(&format!("[pid 0 at 0] now {}", crate::health::line(look, Some(top)))).map(|s| s.named).unwrap_or_default(), other: String::new() };
        let told = stretch(&Stretch { looks: vec![&now] });
        out.push_str(&told.lines().filter(|l| l.trim_start().starts_with("reading: ")).map(|l| format!("{}\n", l.trim_start())).collect::<String>());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A week in which the machine ran out of memory is told as that, with
    /// the programs behind it, and what the record cannot say is said
    #[test]
    fn a_week_is_told_by_its_stretches_of_trouble() {
        let record = "\
[pid 1 at 1000] 2026-11-13T13:00:00 started 0.32.0 (abc)
[pid 1 at 1600] 2026-11-13T13:10:00 cpu 8%, memory 1.2 GB free of 7.8 GB, late: work 0.0 s window 2.0 s, commit 4.0 GB left of 17.7 GB, disk 18.4 GB free of 117.8 GB, kernel 2%, disk busy 3%, disk queue 0.0, paged in 4/s
[pid 1 at 4600] 2026-11-13T14:00:00 cpu 41%, memory 212 MB free of 7.8 GB, late: work 0.4 s window 9.0 s, commit 301 MB left of 17.7 GB, disk 18.4 GB free of 117.8 GB, kernel 30%, disk busy 100%, disk queue 6.1, paged in 5210/s | most cpu: MsMpEng 14% | most memory: chrome x23 2.1 GB, node x6 1.9 GB | most committed: chrome x23 3.0 GB | most io: System 31.0 MB/s | this app's own: 14 programs, cpu 9.5%, memory 4.1 GB, io 0.4 MB/s
[pid 1 at 4660] 2026-11-13T14:01:00 cpu 38%, memory 198 MB free of 7.8 GB, late: work 0.2 s window 2.0 s, commit 280 MB left of 17.7 GB, disk 18.4 GB free of 117.8 GB, kernel 31%, disk busy 99%, disk queue 5.0, paged in 4100/s | most cpu: MsMpEng 11% | most memory: chrome x22 2.0 GB, node x6 2.0 GB | this app's own: 14 programs, cpu 8.0%, memory 4.2 GB, io 0.1 MB/s
[pid 1 at 4700] 2026-11-13T14:01:40 slow: removing a folder took 95 s (it stayed)
[pid 1 at 9000] 2026-11-13T15:13:20 cpu 96%, memory 3.0 GB free of 7.8 GB, late: work 0.0 s window 2.0 s, commit 9.0 GB left of 17.7 GB, disk 18.4 GB free of 117.8 GB, kernel 4%, disk busy 2%, disk queue 0.0, paged in 0/s | most cpu: rustc x8 88% | most memory: chrome x20 1.5 GB
";
        let told = of_week(record);
        assert!(told.contains("It runs from 2026-11-13T13:00:00 to 2026-11-13T15:13:20: 4 looks, 3 of them with something off."), "{told}");
        assert!(told.contains("Nothing before 2026-11-13T13:00:00 is in it"), "{told}");
        assert!(told.contains("2 stretches of trouble"), "{told}");
        // The newest first
        let (cpu, memory) = (told.find("2026-11-13T15:13:20 (one look)").expect("the busy look"), told.find("2026-11-13T14:00:00 to 2026-11-13T14:01:00 (2 looks, 2 min)").expect("the short stretch"));
        assert!(cpu < memory, "{told}");
        assert!(told.contains("memory was short in 2 of 2 looks: as little as 198 MB free of 7.8 GB; of what the system can promise, as little as 280 MB left of 17.7 GB"), "{told}");
        assert!(told.contains("up to 5210 pages a second") && told.contains("up to 100% of it, with up to 6.1 requests waiting"), "{told}");
        assert!(told.contains("holding the most memory: chrome (in 2 of 2 looks, up to 2.1 GB), node (in 2 of 2 looks, up to 2.0 GB)"), "{told}");
        assert!(told.contains("this app and everything it started: up to 9.5% of the processors and 4.2 GB of memory"), "{told}");
        assert!(told.contains("reading: the machine ran out of memory and was reading it back from its disk"), "{told}");
        assert!(told.contains("reading: half or more of the memory in use was this app's"), "{told}");
        assert!(told.contains("reading: the processors were the limit; rustc used the most, up to 88% of the whole machine"), "{told}");
        assert!(told.contains("2026-11-13T14:01:40 removing a folder took 95 s (it stayed)"), "{told}");
        assert!(of_week("").contains("There is none"));
    }

    /// A record from before the disk and the paging were noted says memory
    /// was short and that it cannot say more; an app late on a calm machine
    /// is the app's own fault
    #[test]
    fn what_the_record_cannot_say_is_said() {
        let old = "[pid 1 at 100] 2026-11-18T09:57:13 cpu 18%, memory 727 MB free of 7.8 GB, late: work 0.0 s window 2.0 s, commit 3.9 GB left of 17.7 GB, disk 18.3 GB free of 117.8 GB | most cpu: chrome.exe 1.4% | most memory: chrome.exe 1.6 GB\n";
        let told = of_week(old);
        assert!(told.contains("did not yet note the disk or the paging"), "{told}");
        let stuck = "[pid 1 at 100] 2026-11-18T09:57:13 cpu 6%, memory 20.0 GB free of 32.0 GB, late: work 41.0 s window 2.0 s, kernel 1%, disk busy 1%, disk queue 0.0, paged in 0/s | most cpu: chrome 1.4%\n";
        let told = of_week(stuck);
        assert!(told.contains("its work part by up to 41.0 s") && told.contains("the fault is in the app itself; hang.json says which part"), "{told}");
        // Late for a few seconds: nothing was written down about where
        assert!(of_week(&stuck.replace("work 41.0 s", "work 6.0 s")).contains("not late for long enough"), "{}", of_week(&stuck.replace("work 41.0 s", "work 6.0 s")));
        // Late in a part of the record that did not note the disk: the disk is not ruled out
        let unknown = "[pid 1 at 100] 2026-11-18T09:57:13 cpu 6%, memory 20.0 GB free of 32.0 GB, late: work 0.0 s window 6.0 s | most cpu: chrome 1.4%\n";
        assert!(of_week(unknown).contains("is not ruled out") && !of_week(unknown).contains("the fault is in the app"), "{}", of_week(unknown));
        // Little left to promise and a busy disk, with nothing read back: not called running out
        let both = "[pid 1 at 100] 2026-11-18T09:57:13 cpu 6%, memory 20.0 GB free of 32.0 GB, late: work 0.0 s window 2.0 s, commit 1.0 GB left of 40.0 GB, kernel 1%, disk busy 97%, disk queue 2.0, paged in 0/s | most io: robocopy 80.0 MB/s\n";
        assert!(of_week(both).contains("may have nothing to do with each other") && !of_week(both).contains("ran out of memory"), "{}", of_week(both));
        // A clock put back between two looks of one stretch does not break the telling
        let back = "[pid 1 at 500] 2026-11-18T10:00:00 cpu 96%, memory 20.0 GB free of 32.0 GB, late: work 0.0 s window 2.0 s\n[pid 1 at 440] 2026-11-18T09:59:00 cpu 97%, memory 20.0 GB free of 32.0 GB, late: work 0.0 s window 2.0 s\n";
        assert!(of_week(back).contains("(2 looks, 1 min)"), "{}", of_week(back));
        let calm = "[pid 1 at 100] 2026-11-18T09:57:13 cpu 6%, memory 20.0 GB free of 32.0 GB, late: work 0.0 s window 2.0 s\n";
        assert!(of_week(calm).contains("No look found the machine short of anything"));
    }

    /// The records out of a report somebody sent, read for looking at by eye:
    /// `SHIKISHA_DIAG_READ=<the unpacked report's folder> cargo test -- --ignored --nocapture read_the_records`
    #[test]
    #[ignore]
    fn read_the_records_of_a_report_for_a_check() {
        let dir = std::path::PathBuf::from(std::env::var("SHIKISHA_DIAG_READ").expect("SHIKISHA_DIAG_READ"));
        let read = |name: &str| std::fs::read(dir.join(name)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        println!("{}
{}", of_week(&read("health.log")), of_log(&read("hooks.log")));
    }

    /// The moment a report is made is read for what is true of that moment:
    /// an app that was late a while ago is said, and not blamed on a machine
    /// that is calm now
    #[test]
    fn the_moment_is_not_read_for_what_happened_before_it() {
        let look = Look { cpu: 6.0, free: 20 << 30, total: 32 << 30, late: [0, 9_000], kernel: Some(1.0), busy: Some(1.0), queue: Some(0.0), paged: Some(0.0), ..Default::default() };
        let told = of_now(&look, &crate::health::Top::default());
        assert!(told.contains("answered late at some point in the last minute") && told.contains("up to 9.0 s"), "{told}");
        assert!(told.contains("Nothing was off at that moment") && !told.contains("the fault is in the app"), "{told}");
        // Short at one look and reading pages at the next is not running out
        let apart = "[pid 1 at 100] 2026-11-18T10:00:00 cpu 6%, memory 500 MB free of 7.8 GB, late: work 0.0 s window 2.0 s, kernel 1%, disk busy 1%, disk queue 0.0, paged in 0/s\n[pid 1 at 160] 2026-11-18T10:01:00 cpu 6%, memory 4.0 GB free of 7.8 GB, late: work 0.0 s window 2.0 s, kernel 1%, disk busy 1%, disk queue 0.0, paged in 3000/s\n";
        let told = of_week(apart);
        assert!(!told.contains("ran out of memory") && told.contains("a large program was starting"), "{told}");
    }

    /// The log's known troubles are named with what clears them, and what
    /// it said over and over is counted
    #[test]
    fn the_log_is_read_for_what_stops_the_app() {
        let mut log = String::from("[   0.000] started\n");
        for i in 0..40 {
            log.push_str(&format!("[{:>8.3}] this PC's resident process: no line to it (~\\keeper\\ab12\\run\\keep.key is not there: os error 3)\n", i as f64 * 2.0));
        }
        log.push_str("[  90.000] could not remove a folder: something held it\n");
        let told = of_log(&log);
        assert!(told.contains("could not reach this PC's resident process") && told.contains("keep.key is not there"), "{told}");
        // The same, read from a log whose run of one line was said once
        let folded = "[   0.000] started\n[   2.000] this PC's resident process: no line to it (gone)\n    (the same 39 more times, the last at 80.000)\n";
        assert!(of_log(folded).contains("40 times: this PC's resident process: no line to it (gone)"), "{}", of_log(folded));
        assert!(told.contains("restarting the PC"), "{told}");
        assert!(told.contains("stayed, 1 times"), "{told}");
        assert!(told.contains("40 times: this PC's resident process: no line to it"), "{told}");
        assert!(of_log("[   0.000] started\n[   1.000] ready\n").contains("Nothing in it is known to stop the app working"));
    }
}

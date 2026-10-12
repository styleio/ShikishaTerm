//! Automation that runs by the clock.
//!
//! Every other automation runs when something happens in a tab. Some work
//! is wanted at a time instead -- a summary of the night's work at nine, the
//! tests every half hour -- and with nothing happening in any tab, nothing
//! ran it. So a hook can be named after a time the way the others are named
//! after an event, and the app's own clock runs it:
//!
//! - `at_0900` -- every day at 09:00, by this PC's clock
//! - `at_weekdays_0900`, `at_weekends_0900`, `at_mon_0900` ... `at_sun_0900`
//! - `every_30m`, `every_2h` -- every so often, from when it last ran
//!
//! **The app's clock, not the system's scheduler.** Nothing is registered
//! with Task Scheduler or cron: nothing is left on the machine, nothing is
//! left behind when the app is gone, and the same rule holds on Windows, a
//! Mac and Linux. What that costs is said plainly: while the app is not
//! running, or the machine sleeps, nothing runs. A time that went by while it
//! could not run is run once when it can, if that is within [`GRACE`] of the
//! time; later than that, it is written down as missed and not run -- an hour
//! late is still the morning's summary, a day late is not.
//!
//! When each one last ran is kept (`data/schedule.json`), so a restart does
//! not run a time twice and a time is never run for the past it was not
//! there for: one seen for the first time starts counting from now.

use std::collections::BTreeMap;

/// Which days a time holds on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Days {
    Every,
    Weekdays,
    Weekends,
    /// One day of the week, 0 for Sunday
    One(u8),
}

/// When a hook named after a time runs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Every so many seconds, from when it last ran
    Every(u64),
    /// At this time of day, on these days
    At { days: Days, hour: u8, minute: u8 },
}

/// How late a time may still be run, when it went by while it could not be
pub const GRACE: u64 = 60 * 60;

/// The shortest repeat there is. A hook asked to run every few seconds is
/// one that keeps an AI busy for nothing
pub const SHORTEST: u64 = 60;

const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// The time a hook's name says, if it says one
pub fn parse(name: &str) -> Option<When> {
    if let Some(rest) = name.strip_prefix("every_") {
        let (n, unit) = rest.split_at(rest.len().checked_sub(1)?);
        let n: u64 = n.parse().ok().filter(|n| *n > 0)?;
        let secs = match unit {
            "m" => n * 60,
            "h" => n * 60 * 60,
            _ => return None,
        };
        return (secs >= SHORTEST).then_some(When::Every(secs));
    }
    let rest = name.strip_prefix("at_")?;
    let (days, clock) = match rest.split_once('_') {
        Some((d, clock)) => (
            match d {
                "weekdays" => Days::Weekdays,
                "weekends" => Days::Weekends,
                d => Days::One(DAYS.iter().position(|x| *x == d)? as u8),
            },
            clock,
        ),
        None => (Days::Every, rest),
    };
    if clock.len() != 4 || !clock.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (hour, minute): (u8, u8) = (clock[..2].parse().ok()?, clock[2..].parse().ok()?);
    (hour < 24 && minute < 60).then_some(When::At { days, hour, minute })
}

/// A local date and time, as the clock on the wall shows it: year, month,
/// day, hour, minute
pub type Local = (u16, u16, u16, u16, u16);

/// The day of the week of a date, 0 for Sunday
fn weekday((y, m, d, _, _): Local) -> u8 {
    // Days from a fixed point (the civil calendar); 1970-01-01 was a Thursday
    let days = |y: u16, m: u16, d: u16| -> i64 {
        let (y, m) = if m <= 2 { (i64::from(y) - 1, i64::from(m) + 9) } else { (i64::from(y), i64::from(m) - 3) };
        365 * y + y / 4 - y / 100 + y / 400 + (153 * m + 2) / 5 + i64::from(d)
    };
    (days(y, m, d) - days(1970, 1, 1) + 4).rem_euclid(7) as u8
}

fn holds(days: Days, wday: u8) -> bool {
    match days {
        Days::Every => true,
        Days::Weekdays => (1..=5).contains(&wday),
        Days::Weekends => wday == 0 || wday == 6,
        Days::One(d) => d == wday,
    }
}

/// The last moment at or before `now` a time of day falls on, by the clock
/// `local` reads (seconds since 1970 in, the wall clock out)
pub fn last_time(when: When, now: u64, local: &dyn Fn(u64) -> Local) -> Option<u64> {
    let When::At { days, hour, minute } = when else { return None };
    let today = local(now);
    let into_day = u64::from(today.3) * 3600 + u64::from(today.4) * 60 + now % 60;
    let want = u64::from(hour) * 3600 + u64::from(minute) * 60;
    // Today's, else yesterday's, and back a day at a time to a day it holds
    // on -- at most a week back
    let mut at = (now - into_day + want) as i64;
    if at > now as i64 {
        at -= 86_400;
    }
    for _ in 0..8 {
        if at >= 0 && holds(days, weekday(local(at as u64))) {
            return Some(at as u64);
        }
        at -= 86_400;
    }
    None
}

/// What to do about one hook now
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Run it; it was due at this moment
    Run(u64),
    /// It was due at this moment and is too late to run now
    Missed(u64),
    /// Nothing yet
    Not,
}

/// Whether a hook is due, given when it last ran (or was first seen), and
/// what to keep as "last" afterwards
pub fn due(when: When, last: Option<u64>, now: u64, local: &dyn Fn(u64) -> Local) -> (Due, u64) {
    // Seen for the first time: it counts from now, never for the past
    let Some(last) = last else { return (Due::Not, now) };
    match when {
        When::Every(secs) => match now >= last + secs {
            true => (Due::Run(last + secs), now),
            false => (Due::Not, last),
        },
        When::At { .. } => match last_time(when, now, local) {
            Some(at) if at > last && now - at <= GRACE => (Due::Run(at), now),
            Some(at) if at > last => (Due::Missed(at), at),
            _ => (Due::Not, last),
        },
    }
}

/// The next moment a hook will run, given when it last ran (or was first
/// seen): what a person is shown beside the tab it runs for
pub fn next(when: When, last: Option<u64>, now: u64, local: &dyn Fn(u64) -> Local) -> u64 {
    match when {
        When::Every(secs) => last.unwrap_or(now) + secs,
        When::At { .. } => {
            // The next time it holds, a day at a time from today's
            let mut at = last_time(when, now, local).unwrap_or(now);
            for _ in 0..8 {
                at += 86_400;
                if let Some(back) = last_time(when, at, local)
                    && back > now
                {
                    return back;
                }
            }
            at
        }
    }
}

/// When each hook named after a time last ran, kept from one run of the app
/// to the next. Keyed by the script, the hook and the tab it runs for
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Kept {
    #[serde(default)]
    pub last: BTreeMap<String, u64>,
}

fn kept_path() -> std::path::PathBuf {
    crate::config::state_path("schedule.json")
}

impl Kept {
    pub fn load() -> Kept {
        std::fs::read_to_string(kept_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = crate::crypto::write_atomic(&kept_path(), &text);
        }
    }
}

/// The key a hook is kept under
pub fn key(script: &str, hook: &str, tab: Option<&str>) -> String {
    format!("{script}#{hook}#{}", tab.unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock in UTC, so the tests read the same everywhere
    fn utc(epoch: u64) -> Local {
        let days = (epoch / 86_400) as i64;
        let secs = epoch % 86_400;
        // Civil from days (Howard Hinnant's algorithm)
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        (y as u16, m as u16, d as u16, (secs / 3600) as u16, ((secs % 3600) / 60) as u16)
    }

    // 2026-10-12 is a Monday; 10:00 UTC
    const MON_10: u64 = 1_791_799_200;

    #[test]
    fn a_name_says_a_time_or_nothing() {
        assert_eq!(parse("at_0900"), Some(When::At { days: Days::Every, hour: 9, minute: 0 }));
        assert_eq!(parse("at_weekdays_1830"), Some(When::At { days: Days::Weekdays, hour: 18, minute: 30 }));
        assert_eq!(parse("at_sun_0700"), Some(When::At { days: Days::One(0), hour: 7, minute: 0 }));
        assert_eq!(parse("every_30m"), Some(When::Every(1800)));
        assert_eq!(parse("every_2h"), Some(When::Every(7200)));
        for not in ["at_2460", "at_900", "at_funday_0900", "every_0m", "every_30s", "every_m", "on_done", "every_30"] {
            assert_eq!(parse(not), None, "{not} was taken for a time");
        }
    }

    #[test]
    fn the_calendar_is_read_right() {
        assert_eq!(weekday(utc(MON_10)), 1, "2026-10-12 is a Monday");
        assert_eq!(utc(MON_10), (2026, 10, 12, 10, 0));
    }

    #[test]
    fn a_time_of_day_runs_once_when_it_comes() {
        let nine = parse("at_0900").unwrap();
        // Seen for the first time after nine: it does not run for this morning
        assert_eq!(due(nine, None, MON_10, &utc), (Due::Not, MON_10));
        // Kept from yesterday evening, now 10:00: nine came an hour ago -- run
        let yesterday = MON_10 - 15 * 3600;
        let (d, last) = due(nine, Some(yesterday), MON_10 - 3540, &utc);
        assert_eq!(d, Due::Run(MON_10 - 3600), "nine o'clock did not run at 09:01");
        // ...and not again until tomorrow
        assert_eq!(due(nine, Some(last), MON_10, &utc).0, Due::Not);
        assert_eq!(due(nine, Some(last), MON_10 + 23 * 3600, &utc).0, Due::Run(MON_10 + 23 * 3600));
    }

    #[test]
    fn a_time_gone_by_is_run_late_within_an_hour_and_missed_after() {
        let nine = parse("at_0900").unwrap();
        let yesterday = MON_10 - 15 * 3600;
        // Woken at 09:50: run, late
        assert_eq!(due(nine, Some(yesterday), MON_10 - 600, &utc).0, Due::Run(MON_10 - 3600));
        // Woken at 11:30: missed, and kept as missed so it is not run later
        let (d, last) = due(nine, Some(yesterday), MON_10 + 5400, &utc);
        assert_eq!(d, Due::Missed(MON_10 - 3600));
        assert_eq!(due(nine, Some(last), MON_10 + 5460, &utc).0, Due::Not, "a missed time ran after all");
    }

    #[test]
    fn days_of_the_week_are_kept() {
        let weekdays = parse("at_weekdays_0900").unwrap();
        let sunday_noon = MON_10 - 22 * 3600;
        // Sunday: the last weekday nine is Friday's
        assert_eq!(last_time(weekdays, sunday_noon, &utc), Some(MON_10 - 3 * 86_400 - 3600));
        let sunday = parse("at_sun_0700").unwrap();
        assert_eq!(last_time(sunday, MON_10, &utc), Some(MON_10 - 86_400 - 3 * 3600));
    }

    #[test]
    fn the_next_time_is_the_one_shown() {
        // Monday 10:00: nine is tomorrow; a weekday nine on Friday 10:00 is Monday's
        assert_eq!(next(parse("at_0900").unwrap(), None, MON_10, &utc), MON_10 + 23 * 3600);
        let friday_10 = MON_10 + 4 * 86_400;
        assert_eq!(next(parse("at_weekdays_0900").unwrap(), None, friday_10, &utc), friday_10 + 3 * 86_400 - 3600);
        // ...and eleven today is today
        assert_eq!(next(parse("at_1100").unwrap(), None, MON_10, &utc), MON_10 + 3600);
        assert_eq!(next(parse("every_30m").unwrap(), Some(MON_10), MON_10 + 60, &utc), MON_10 + 1800);
    }

    #[test]
    fn a_repeat_runs_from_when_it_last_ran() {
        let half = parse("every_30m").unwrap();
        assert_eq!(due(half, Some(MON_10), MON_10 + 1700, &utc).0, Due::Not);
        assert_eq!(due(half, Some(MON_10), MON_10 + 1800, &utc), (Due::Run(MON_10 + 1800), MON_10 + 1800));
        // Back after a night asleep: once, not once for every half hour gone by
        let (d, last) = due(half, Some(MON_10), MON_10 + 10 * 3600, &utc);
        assert!(matches!(d, Due::Run(_)));
        assert_eq!(due(half, Some(last), MON_10 + 10 * 3600 + 60, &utc).0, Due::Not);
    }
}

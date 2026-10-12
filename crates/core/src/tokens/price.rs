//! What a model's tokens would cost through its maker's API, by the price
//! lists the makers publish.
//!
//! A subscription is not billed this way: these numbers say what the same
//! work would have cost bought by the token, which is what lets a month of
//! work be weighed against the plan that paid for it. Read from
//! <https://platform.claude.com/docs/en/about-claude/pricing> and
//! <https://developers.openai.com/api/docs/pricing> on 2026-10-12. A model
//! not listed here has no price: it is counted, and its cost said to be
//! unknown, rather than guessed from a neighbour whose price may differ.

/// US dollars per million tokens, by kind of token
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub input: f64,
    pub cache_read: f64,
    /// Written to the cache for five minutes
    pub cache_write: f64,
    /// Written to the cache for an hour
    pub cache_write_hour: f64,
    pub out: f64,
}

/// A model's price: its rate, and what changes it
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub rate: Rate,
    /// A request whose prompt is longer than this many tokens pays the
    /// second rate for all of it (Claude Haiku 5.5 over 100,000; OpenAI's
    /// over 272,000)
    pub long: Option<(u64, Rate)>,
    /// What Claude's fast mode multiplies the whole price by, for a model
    /// that has it
    pub fast: Option<f64>,
}

/// What keeping a Claude request to the United States multiplies its price
/// by (`inference_geo: "us"`, Claude 4.6 and later)
pub const US_ONLY: f64 = 1.1;

/// The prompt lengths a price can change at, smallest first: the
/// boundaries a call's prompt is sorted by before it is priced
pub const BANDS: [u64; 2] = [100_000, 272_000];

const fn claude(input: f64, cache_write: f64, cache_write_hour: f64, cache_read: f64, out: f64) -> Rate {
    Rate { input, cache_read, cache_write, cache_write_hour, out }
}

/// OpenAI writes no separate price for a cache write; cached input is read
/// at its own price, or at the input price where none is listed (`-`)
const fn openai(input: f64, cached: f64, out: f64) -> Rate {
    Rate { input, cache_read: cached, cache_write: input, cache_write_hour: input, out }
}

const fn plain(rate: Rate) -> Price {
    Price { rate, long: None, fast: None }
}

/// The models, by the name their CLI writes down once tidied ([`tidy`])
fn listed(model: &str) -> Option<Price> {
    // Fast mode is twice the price, and the cache's multipliers apply on top
    // of that, so the whole rate doubles
    let fast = |rate: Rate| Price { rate, long: None, fast: Some(2.0) };
    let long_openai = |rate: Rate, over: Rate| Price { rate, long: Some((BANDS[1], over)), fast: None };
    Some(match model {
        "claude-fable-5-1" | "claude-mythos-5-1" => plain(claude(10.0, 12.5, 20.0, 0.25, 50.0)),
        "claude-fable-5" | "claude-mythos-5" => plain(claude(10.0, 12.5, 20.0, 1.0, 50.0)),
        "claude-opus-5-5" => fast(claude(4.0, 5.0, 8.0, 0.2, 20.0)),
        "claude-opus-5" | "claude-opus-4-8" => fast(claude(5.0, 6.25, 10.0, 0.5, 25.0)),
        "claude-opus-4-7" | "claude-opus-4-6" | "claude-opus-4-5" => plain(claude(5.0, 6.25, 10.0, 0.5, 25.0)),
        "claude-opus-4-1" | "claude-opus-4" => plain(claude(15.0, 18.75, 30.0, 1.5, 75.0)),
        "claude-sonnet-5-5" => plain(claude(2.0, 2.5, 4.0, 0.1, 10.0)),
        "claude-sonnet-5" => plain(claude(2.0, 2.5, 4.0, 0.2, 10.0)),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" | "claude-sonnet-4" => plain(claude(3.0, 3.75, 6.0, 0.3, 15.0)),
        "claude-haiku-5-5" => Price {
            rate: claude(0.1, 0.125, 0.2, 0.01, 0.5),
            long: Some((BANDS[0], claude(0.5, 0.625, 1.0, 0.05, 2.5))),
            fast: None,
        },
        "claude-haiku-4-5" => plain(claude(1.0, 1.25, 2.0, 0.1, 5.0)),
        "claude-haiku-3-5" => plain(claude(0.8, 1.0, 1.6, 0.08, 4.0)),
        "gpt-6-astra" => long_openai(openai(10.0, 1.0, 50.0), openai(20.0, 2.0, 75.0)),
        "gpt-6.1-sol" => long_openai(openai(2.0, 0.1, 10.0), openai(4.0, 0.2, 15.0)),
        "gpt-6-sol" => long_openai(openai(2.0, 0.2, 10.0), openai(4.0, 0.4, 15.0)),
        "gpt-6-luna" => long_openai(openai(0.1, 0.01, 0.5), openai(0.2, 0.02, 0.75)),
        "gpt-5.6-sol" => long_openai(openai(4.0, 0.4, 20.0), openai(8.0, 0.8, 30.0)),
        "gpt-5.6-terra" => long_openai(openai(2.0, 0.2, 12.0), openai(4.0, 0.4, 18.0)),
        "gpt-5.6-luna" => long_openai(openai(0.2, 0.02, 1.2), openai(0.4, 0.04, 1.8)),
        "gpt-5.5" => long_openai(openai(5.0, 0.5, 30.0), openai(10.0, 1.0, 45.0)),
        "gpt-5.5-pro" => long_openai(openai(30.0, 30.0, 180.0), openai(60.0, 60.0, 270.0)),
        "gpt-5.4" => long_openai(openai(2.5, 0.25, 15.0), openai(5.0, 0.5, 22.5)),
        "gpt-5.4-pro" => long_openai(openai(30.0, 30.0, 180.0), openai(60.0, 60.0, 270.0)),
        "gpt-5.4-mini" => plain(openai(0.75, 0.075, 4.5)),
        "gpt-5.4-nano" => plain(openai(0.2, 0.02, 1.25)),
        "gpt-5.3-codex" | "gpt-5.2" => plain(openai(1.75, 0.175, 14.0)),
        "gpt-5.2-pro" => plain(openai(21.0, 21.0, 168.0)),
        "gpt-5.1" | "gpt-5" => plain(openai(1.25, 0.125, 10.0)),
        "gpt-5-mini" => plain(openai(0.25, 0.025, 2.0)),
        "gpt-5-nano" => plain(openai(0.05, 0.005, 0.4)),
        "gpt-5-pro" => plain(openai(15.0, 15.0, 120.0)),
        _ => return None,
    })
}

/// A model's name the way the list above writes it: lower case, without a
/// maker's prefix, a date, a `[1m]`-style context mark or a "-thinking", and
/// with a Claude version's dot as a dash (`claude-opus-4.6`)
pub fn tidy(model: &str) -> String {
    let mut m = model.trim().to_ascii_lowercase();
    for prefix in ["anthropic/", "anthropic:", "openai/", "openai:"] {
        if let Some(rest) = m.strip_prefix(prefix) {
            m = rest.to_string();
        }
    }
    if let Some(at) = m.find(['[', '@']) {
        m.truncate(at);
    }
    if let Some(rest) = m.strip_suffix("-thinking") {
        m = rest.to_string();
    }
    // A release date on the end: -20250929
    if let Some((head, tail)) = m.rsplit_once('-')
        && tail.len() == 8
        && tail.bytes().all(|b| b.is_ascii_digit())
    {
        m = head.to_string();
    }
    if m.starts_with("claude-") {
        m = m.replace('.', "-");
    }
    m
}

/// The price of a model by the name its CLI wrote down, if it is listed. A
/// Codex variant without a price of its own (`gpt-5.1-codex`) is the model
/// it is a variant of
pub fn of(model: &str) -> Option<Price> {
    let m = tidy(model);
    listed(&m).or_else(|| {
        let base = m.split("-codex").next().filter(|b| *b != m)?;
        listed(base)
    })
}

/// What a group of calls of one model cost, in US dollars. `band` is how
/// many of [`BANDS`] their prompts were longer than
#[allow(clippy::too_many_arguments)]
pub fn cost(price: &Price, band: usize, fast: bool, us: bool, fresh_in: u64, cache_read: u64, cache_write: u64, cache_write_hour: u64, out: u64) -> f64 {
    let rate = match price.long {
        Some((over, long)) if BANDS.iter().take(band).any(|b| *b >= over) => long,
        _ => price.rate,
    };
    let dollars = (fresh_in as f64 * rate.input
        + cache_read as f64 * rate.cache_read
        + cache_write as f64 * rate.cache_write
        + cache_write_hour as f64 * rate.cache_write_hour
        + out as f64 * rate.out)
        / 1_000_000.0;
    let fast = if fast { price.fast.unwrap_or(1.0) } else { 1.0 };
    let us = if us { US_ONLY } else { 1.0 };
    dollars * fast * us
}

/// How many of [`BANDS`] a prompt of `tokens` is longer than
pub fn band_of(tokens: u64) -> usize {
    BANDS.iter().filter(|b| tokens > **b).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_the_way_the_list_writes_them() {
        assert_eq!(tidy("claude-opus-4-6-20260101"), "claude-opus-4-6");
        assert_eq!(tidy("claude-opus-4.6-thinking"), "claude-opus-4-6");
        assert_eq!(tidy("Claude-Sonnet-5-5[1m]"), "claude-sonnet-5-5");
        assert_eq!(tidy("anthropic/claude-haiku-4-5@20251001"), "claude-haiku-4-5");
        assert_eq!(tidy("gpt-5.4"), "gpt-5.4", "OpenAI's dots are part of the name");
        assert!(of("gpt-5.1-codex-max").is_some(), "a Codex variant is its model's price");
        assert!(of("claude-opus-4-9").is_none(), "an unlisted model has no price, not a neighbour's");
        assert!(of("<synthetic>").is_none());
    }

    #[test]
    fn a_call_is_priced_by_its_kinds_of_token() {
        let p = of("claude-opus-5-5").unwrap();
        // 1M of each: 4 + 0.2 + 5 + 8 + 20
        let c = cost(&p, 0, false, false, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000);
        assert!((c - 37.2).abs() < 1e-9, "{c}");
        let fast = cost(&p, 0, true, false, 1_000_000, 0, 0, 0, 0);
        assert!((fast - 8.0).abs() < 1e-9, "fast mode doubles: {fast}");
        let us = cost(&p, 0, false, true, 1_000_000, 0, 0, 0, 0);
        assert!((us - 4.4).abs() < 1e-9, "kept to the US is 1.1 times: {us}");
        let slow = cost(&of("claude-opus-4-7").unwrap(), 0, true, false, 1_000_000, 0, 0, 0, 0);
        assert!((slow - 5.0).abs() < 1e-9, "a model with no fast mode is not doubled: {slow}");
    }

    #[test]
    fn a_long_prompt_pays_the_long_rate_where_there_is_one() {
        let haiku = of("claude-haiku-5-5").unwrap();
        assert_eq!(band_of(100_000), 0);
        assert_eq!(band_of(100_001), 1);
        assert_eq!(band_of(300_000), 2);
        let short = cost(&haiku, 0, false, false, 1_000_000, 0, 0, 0, 0);
        let long = cost(&haiku, 1, false, false, 1_000_000, 0, 0, 0, 0);
        assert!((short - 0.1).abs() < 1e-9 && (long - 0.5).abs() < 1e-9, "{short} {long}");
        let gpt = of("gpt-5.5").unwrap();
        let mid = cost(&gpt, 1, false, false, 1_000_000, 0, 0, 0, 0);
        let over = cost(&gpt, 2, false, false, 1_000_000, 0, 0, 0, 0);
        assert!((mid - 5.0).abs() < 1e-9, "100k is not GPT's line: {mid}");
        assert!((over - 10.0).abs() < 1e-9, "{over}");
    }
}

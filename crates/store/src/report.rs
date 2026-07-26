//! The weekly drift report — generated locally, from local data, offline.
//!
//! # Why this exists
//!
//! Drifterr had no reason to be reopened. It watched quietly, said nothing most of
//! the time (by design), and offered no accumulated view of what it had caught. An
//! outcome-free monitoring tool is indistinguishable from a broken one, and it gets
//! uninstalled — so this is the retention loop the product was missing, and the only
//! honest upgrade trigger it has: someone who sees "`no new deps` broke 9 times this
//! week" has a reason to want history beyond 7 days.
//!
//! # What it will and won't say
//!
//! Flags are grouped by **cause**, not totalled. "23 flags" is a number; "`no new
//! deps` broke 9 times" is something you can act on — restate the rule, or admit
//! you've changed your mind and retire it.
//!
//! Re-anchor outcomes report the undecided count separately rather than folding it
//! into the success rate, and the rate is omitted entirely below a handful of decided
//! cases. Two-out-of-two is not 100%.
//!
//! Nothing here touches the network. The report is built from the local SQLite store
//! and returned as markdown for the caller to save or show.

use crate::{Result, Store};

/// One week in milliseconds — the default window.
pub const WEEK_MS: i64 = 7 * 86_400_000;

/// Below this many *decided* re-anchors, a success rate is noise dressed as a metric.
const MIN_DECIDED_FOR_RATE: usize = 5;

/// A rendered report plus the couple of figures a caller might want to show
/// separately (a notification title, say).
#[derive(Debug, Clone)]
pub struct Report {
    /// The full report, as markdown.
    pub markdown: String,
    /// Non-green flags in the window.
    pub flags: usize,
    /// Sessions with activity in the window.
    pub sessions: usize,
    /// Re-anchors in the window.
    pub reanchors: usize,
    /// True when there was essentially nothing to report — the caller should stay
    /// quiet rather than send a notification about silence.
    pub quiet_week: bool,
}

/// Build the report for the `window_ms` ending at `now_ms`.
///
/// Time is passed in rather than read from the clock so the output is deterministic
/// and testable.
pub fn weekly(store: &Store, now_ms: i64, window_ms: i64) -> Result<Report> {
    let since = now_ms - window_ms;
    let days = (window_ms / 86_400_000).max(1);

    let (sessions, red_sessions) = store.session_activity_since(since)?;
    let counts = store.flag_counts_since(since)?;
    let (reanchors, held, broke) = store.reanchor_stats(since)?;
    let flags: usize = counts.iter().map(|(_, _, n)| n).sum();

    let mut md = String::new();
    md.push_str(&format!("# Drifterr — last {days} days\n\n"));

    let quiet_week = sessions == 0 && flags == 0;
    if quiet_week {
        md.push_str(
            "Nothing to report: no sessions were tracked in this window.\n\n\
             If you expected activity here, the channel may not be connected — check \
             Settings → Config in the panel.\n",
        );
        return Ok(Report {
            markdown: md,
            flags,
            sessions,
            reanchors,
            quiet_week,
        });
    }

    md.push_str("## At a glance\n\n");
    md.push_str(&format!(
        "- **{sessions}** session{} tracked",
        plural(sessions)
    ));
    if red_sessions > 0 {
        md.push_str(&format!(", **{red_sessions}** of which went red"));
    }
    md.push('\n');
    md.push_str(&format!("- **{flags}** flag{} raised\n", plural(flags)));
    md.push_str(&format!(
        "- **{reanchors}** re-anchor{}\n",
        plural(reanchors)
    ));

    if flags == 0 {
        md.push_str(
            "\nNo drift was flagged. That is the expected outcome most weeks — only \
             hard signals can raise a red, and ambiguous cases deliberately stay quiet.\n",
        );
    } else {
        md.push_str("\n## What drifted, by cause\n\n");
        md.push_str(
            "Grouped by cause rather than totalled: a rule that breaks repeatedly is \
             either worth restating more explicitly, or worth retiring because you have \
             changed your mind about it.\n\n",
        );
        md.push_str("| times | signal | constraint |\n|---:|---|---|\n");
        for (signal, cid, n) in counts.iter().take(15) {
            md.push_str(&format!(
                "| {n} | {signal} | {} |\n",
                cid.as_deref().unwrap_or("—")
            ));
        }
        if counts.len() > 15 {
            md.push_str(&format!("\n…and {} more cause(s).\n", counts.len() - 15));
        }
    }

    if reanchors > 0 {
        md.push_str("\n## Did re-anchoring help?\n\n");
        let decided = held + broke;
        let undecided = reanchors.saturating_sub(decided);
        md.push_str(&format!(
            "- **{held}** held (the same cause stayed quiet afterwards)\n\
             - **{broke}** did not (the same cause came back)\n"
        ));
        if undecided > 0 {
            md.push_str(&format!(
                "- **{undecided}** still undecided — too few turns since to tell\n"
            ));
        }
        // A rate on two or three cases would be theatre. State the counts and say why
        // there is no percentage rather than printing a meaningless one.
        if decided >= MIN_DECIDED_FOR_RATE {
            let pct = (held as f64 / decided as f64 * 100.0).round();
            md.push_str(&format!(
                "\n**{pct:.0}%** of decided re-anchors held ({held}/{decided}).\n"
            ));
        } else if decided > 0 {
            md.push_str(&format!(
                "\nNo percentage yet — {decided} decided re-anchor{} is too few to rate \
                 (needs {MIN_DECIDED_FOR_RATE}).\n",
                plural(decided)
            ));
        }
    }

    md.push_str(
        "\n---\n\n*Generated on your machine from your local database. \
         No network, no account, nothing sent anywhere.*\n",
    );

    Ok(Report {
        markdown: md,
        flags,
        sessions,
        reanchors,
        quiet_week,
    })
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use drifterr_engine::conversation::{ContextState, Conversation, Role, Source, Turn};
    use drifterr_engine::signals::{Evidence, SignalEvent, SignalKind, State};

    const NOW: i64 = 1_800_000_000_000;

    fn conv(id: &str, ts: i64) -> Conversation {
        Conversation {
            session_id: id.into(),
            model: "m".into(),
            turns: vec![
                Turn {
                    index: 0,
                    role: Role::User,
                    content: "do the thing".into(),
                    tokens: 4,
                    timestamp: ts,
                },
                Turn {
                    index: 1,
                    role: Role::Assistant,
                    content: "done".into(),
                    tokens: 2,
                    timestamp: ts,
                },
            ],
            context: ContextState {
                window_size: 1000,
                used_tokens: 6,
                exact: true,
                occupancy_known: true,
                tool_call_count: 0,
            },
            source: Source::Proxy,
        }
    }

    fn flag(cid: &str) -> SignalEvent {
        SignalEvent::new(
            SignalKind::Constraint,
            State::Red,
            Evidence {
                detail: format!("constraint {cid} violated"),
                turn_index: Some(1),
                constraint_id: Some(cid.to_string()),
                span: Some(".js".into()),
            },
        )
    }

    #[test]
    fn quiet_week_says_so_and_asks_for_no_attention() {
        let s = Store::open_in_memory().unwrap();
        let r = weekly(&s, NOW, WEEK_MS).unwrap();
        assert!(r.quiet_week);
        assert_eq!(r.flags, 0);
        assert!(r.markdown.contains("Nothing to report"));
        // A quiet week should hint at the likely cause rather than look like success.
        assert!(r.markdown.contains("channel may not be connected"));
    }

    #[test]
    fn groups_flags_by_cause_most_frequent_first() {
        let mut s = Store::open_in_memory().unwrap();
        s.save_conversation(&conv("s1", NOW - 3_600_000)).unwrap();
        s.set_status("s1", State::Red).unwrap();
        // c1 three times, c2 once.
        s.record_events_at(
            "s1",
            &[flag("c1"), flag("c1"), flag("c1"), flag("c2")],
            NOW - 3_600_000,
        )
        .unwrap();

        let r = weekly(&s, NOW, WEEK_MS).unwrap();
        assert_eq!(r.flags, 4);
        assert_eq!(r.sessions, 1);
        let md = &r.markdown;
        assert!(md.contains("| 3 | constraint | c1 |"), "{md}");
        assert!(md.contains("| 1 | constraint | c2 |"), "{md}");
        // Most frequent cause must come first — that's the actionable ordering.
        let (i1, i2) = (md.find("| c1 |"), md.find("| c2 |"));
        assert!(i1 < i2, "c1 should be listed before c2");
        assert!(
            md.contains("**1** session tracked"),
            "singular session: {md}"
        );
        assert!(md.contains("went red"));
        assert!(md.contains("**4** flags raised"));
        // Zero re-anchors ⇒ no "did it help" section to speculate in.
        assert!(!md.contains("Did re-anchoring help"));
    }

    #[test]
    fn ignores_activity_outside_the_window() {
        let mut s = Store::open_in_memory().unwrap();
        // Two weeks ago — outside a 7-day window.
        s.save_conversation(&conv("old", NOW - 14 * 86_400_000))
            .unwrap();
        s.record_events_at("old", &[flag("c1")], NOW - 14 * 86_400_000)
            .unwrap();
        let r = weekly(&s, NOW, WEEK_MS).unwrap();
        assert_eq!(r.sessions, 0, "old session must not count");
        assert!(r.quiet_week);
    }

    #[test]
    fn reanchor_rate_is_withheld_until_it_would_mean_something() {
        let mut s = Store::open_in_memory().unwrap();
        s.save_conversation(&conv("s1", NOW - 3_600_000)).unwrap();
        s.record_events_at("s1", &[flag("c1")], NOW - 3_600_000)
            .unwrap();

        // Two decided re-anchors: below the threshold, so no percentage.
        s.record_reanchor("s1", "constraint", Some("c1"), NOW - 7_200_000)
            .unwrap();
        s.set_reanchor_outcome("s1", true).unwrap();
        s.record_reanchor("s1", "constraint", Some("c1"), NOW - 3_600_000)
            .unwrap();
        s.set_reanchor_outcome("s1", false).unwrap();

        let r = weekly(&s, NOW, WEEK_MS).unwrap();
        assert_eq!(r.reanchors, 2);
        assert!(r.markdown.contains("**1** held"));
        assert!(r.markdown.contains("**1** did not"));
        assert!(
            r.markdown.contains("No percentage yet"),
            "2 decided cases must not produce a rate: {}",
            r.markdown
        );
        assert!(!r.markdown.contains("50%"));
    }

    #[test]
    fn reanchor_rate_appears_once_there_is_enough_data() {
        let mut s = Store::open_in_memory().unwrap();
        s.save_conversation(&conv("s1", NOW - 3_600_000)).unwrap();
        for i in 0..6 {
            s.record_reanchor("s1", "constraint", Some("c1"), NOW - 100_000 * (6 - i))
                .unwrap();
            // 4 held, 2 broke ⇒ 67%.
            s.set_reanchor_outcome("s1", i < 4).unwrap();
        }
        let r = weekly(&s, NOW, WEEK_MS).unwrap();
        assert!(r.markdown.contains("67%"), "{}", r.markdown);
        assert!(r.markdown.contains("(4/6)"));
    }

    #[test]
    fn undecided_reanchors_are_reported_separately_not_as_wins() {
        let mut s = Store::open_in_memory().unwrap();
        s.save_conversation(&conv("s1", NOW - 3_600_000)).unwrap();
        s.record_reanchor("s1", "constraint", Some("c1"), NOW - 60_000)
            .unwrap();
        // Outcome never set — still unknown.
        let r = weekly(&s, NOW, WEEK_MS).unwrap();
        assert!(
            r.markdown.contains("**1** still undecided"),
            "{}",
            r.markdown
        );
        assert!(!r.markdown.contains('%'), "no rate from zero decided cases");
    }

    #[test]
    fn report_never_mentions_a_network() {
        let mut s = Store::open_in_memory().unwrap();
        s.save_conversation(&conv("s1", NOW - 3_600_000)).unwrap();
        s.record_events_at("s1", &[flag("c1")], NOW - 3_600_000)
            .unwrap();
        let md = weekly(&s, NOW, WEEK_MS).unwrap().markdown;
        assert!(md.contains("No network"));
    }
}

//! Plan entitlements — what each subscription tier unlocks.
//!
//! Detection runs entirely locally, so gating is enforced here in the proxy
//! from the plan the desktop app reports (identity only — no chat content is
//! ever involved). The plan → capability mapping lives here as the single source
//! of truth and is derived from the plan, never trusted from client-sent flags.
//! The `plans.features` rows in Supabase mirror it for display on the site.
//!
//! # What Free gets
//!
//! Free is **not** a crippled demo. It gates *depth*, never the core loop:
//! unlimited concurrent sessions, every signal, every constraint check, and
//! manual re-anchor. What Pro adds is retention (full history), the drift map,
//! and automatic re-anchor injection.
//!
//! Capping Free at a single session — which is what this used to do — put a wall
//! in front of the user *before* they had seen a single detection. The first
//! thing a new user met was a limit rather than a result. Depth limits are the
//! right shape: they only bite once the product has already proven itself.
//!
//! # The trial
//!
//! A new install gets [`TRIAL_DAYS`] days of full Pro, tracked **locally** (see
//! `app_meta.trial_started_at` in the store) so it needs no account and no
//! network. That is deliberately easy to reset by wiping local state; the
//! alternative — requiring a server account to unlock a trial — would reintroduce
//! exactly the signup wall we removed, and a trial is not a security boundary.

use serde::{Deserialize, Serialize};

/// Length of the local first-run Pro trial, in days.
pub const TRIAL_DAYS: i64 = 14;

/// How much session history Free retains, in days.
const FREE_HISTORY_DAYS: u32 = 7;

/// Subscription tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    #[default]
    Free,
    /// The local first-run trial. Carries Pro capabilities but stays a distinct
    /// plan so the UI can say "Pro trial — 6 days left" rather than "Pro".
    Trial,
    Pro,
    Team,
}

impl Plan {
    /// Parse a plan id (e.g. from `/me`). Unknown / empty ⇒ Free.
    pub fn from_id(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "trial" => Plan::Trial,
            "pro" => Plan::Pro,
            "team" => Plan::Team,
            _ => Plan::Free,
        }
    }

    /// Does this plan already grant Pro-or-better capabilities? Used to decide
    /// whether the local trial is worth applying at all.
    fn is_paid(self) -> bool {
        matches!(self, Plan::Pro | Plan::Team)
    }
}

/// Days remaining on a trial started at `started_ms`, clamped at 0. A trial that
/// has not started yields `None`.
pub fn trial_days_left(started_ms: Option<i64>, now_ms: i64) -> Option<i64> {
    let started = started_ms?;
    let elapsed_days = (now_ms - started).max(0) / 86_400_000;
    Some((TRIAL_DAYS - elapsed_days).max(0))
}

/// Resolve the plan actually in force: a paid plan always wins, otherwise an
/// unexpired local trial upgrades Free to [`Plan::Trial`].
pub fn resolve_plan(account_plan: Plan, trial_started_ms: Option<i64>, now_ms: i64) -> Plan {
    if account_plan.is_paid() {
        return account_plan;
    }
    match trial_days_left(trial_started_ms, now_ms) {
        Some(days) if days > 0 => Plan::Trial,
        _ => Plan::Free,
    }
}

/// The capabilities unlocked for a plan. Derived from `Plan` so there is exactly
/// one source of truth.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Entitlement {
    pub plan: Plan,
    /// Max sessions surfaced/tracked at once (`None` = unlimited). Unlimited on
    /// every plan today — kept in the shape because the status handler enforces
    /// it generically, so a future tier could cap it without new plumbing.
    #[serde(rename = "maxSessions")]
    pub max_sessions: Option<usize>,
    /// How many days of session history are readable (`None` = unlimited).
    #[serde(rename = "historyDays")]
    pub history_days: Option<u32>,
    /// Session drift map (the rolling drift-score history sparkline).
    #[serde(rename = "driftMap")]
    pub drift_map: bool,
    /// Opt-in proxy auto-re-anchor (inject the preamble on RED).
    #[serde(rename = "autoReanchor")]
    pub auto_reanchor: bool,
    /// Shared standing orders across a team.
    #[serde(rename = "teamSharing")]
    pub team_sharing: bool,
    /// Days left on the local trial, when one is running. Display only.
    #[serde(rename = "trialDaysLeft", skip_serializing_if = "Option::is_none")]
    pub trial_days_left: Option<i64>,
}

impl Default for Entitlement {
    fn default() -> Self {
        Entitlement::for_plan(Plan::Free)
    }
}

impl Entitlement {
    pub fn for_plan(plan: Plan) -> Self {
        match plan {
            Plan::Free => Entitlement {
                plan,
                max_sessions: None,
                history_days: Some(FREE_HISTORY_DAYS),
                drift_map: false,
                auto_reanchor: false,
                team_sharing: false,
                trial_days_left: None,
            },
            // The trial is Pro, with a countdown attached by `with_trial_days_left`.
            Plan::Trial | Plan::Pro => Entitlement {
                plan,
                max_sessions: None,
                history_days: None,
                drift_map: true,
                auto_reanchor: true,
                team_sharing: false,
                trial_days_left: None,
            },
            Plan::Team => Entitlement {
                plan,
                max_sessions: None,
                history_days: None,
                drift_map: true,
                auto_reanchor: true,
                team_sharing: true,
                trial_days_left: None,
            },
        }
    }

    /// Attach the trial countdown for display. Only meaningful on [`Plan::Trial`].
    pub fn with_trial_days_left(mut self, days: Option<i64>) -> Self {
        if self.plan == Plan::Trial {
            self.trial_days_left = days;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    #[test]
    fn free_is_the_default_and_keeps_the_core_loop() {
        let e = Entitlement::default();
        assert_eq!(e.plan, Plan::Free);
        // The point of the rework: Free is no longer capped at one session, so a
        // new user cannot hit a wall before seeing a detection.
        assert_eq!(e.max_sessions, None);
        // It gates depth instead.
        assert_eq!(e.history_days, Some(FREE_HISTORY_DAYS));
        assert!(!e.drift_map && !e.auto_reanchor && !e.team_sharing);
    }

    #[test]
    fn pro_unlocks_depth_but_not_team() {
        let e = Entitlement::for_plan(Plan::Pro);
        assert_eq!(e.max_sessions, None);
        assert_eq!(e.history_days, None);
        assert!(e.drift_map && e.auto_reanchor);
        assert!(!e.team_sharing);
    }

    #[test]
    fn team_unlocks_everything() {
        let e = Entitlement::for_plan(Plan::Team);
        assert!(e.team_sharing && e.drift_map && e.auto_reanchor);
        assert_eq!(e.history_days, None);
    }

    #[test]
    fn trial_has_pro_capabilities_but_its_own_identity() {
        let e = Entitlement::for_plan(Plan::Trial);
        assert_eq!(e.plan, Plan::Trial);
        assert_eq!(e.history_days, None);
        assert!(e.drift_map && e.auto_reanchor);
    }

    #[test]
    fn plan_parsing_is_lenient() {
        assert_eq!(Plan::from_id("trial"), Plan::Trial);
        assert_eq!(Plan::from_id("PRO"), Plan::Pro);
        assert_eq!(Plan::from_id(" team "), Plan::Team);
        assert_eq!(Plan::from_id("nonsense"), Plan::Free);
        assert_eq!(Plan::from_id(""), Plan::Free);
    }

    #[test]
    fn trial_countdown_counts_down_and_floors_at_zero() {
        let start = 1_000 * DAY;
        assert_eq!(trial_days_left(Some(start), start), Some(TRIAL_DAYS));
        assert_eq!(trial_days_left(Some(start), start + DAY), Some(13));
        assert_eq!(trial_days_left(Some(start), start + 14 * DAY), Some(0));
        assert_eq!(trial_days_left(Some(start), start + 99 * DAY), Some(0));
        // A clock that went backwards must not extend the trial.
        assert_eq!(trial_days_left(Some(start), start - DAY), Some(TRIAL_DAYS));
        assert_eq!(trial_days_left(None, start), None);
    }

    #[test]
    fn resolve_prefers_paid_then_trial_then_free() {
        let start = 1_000 * DAY;
        // No trial recorded, no account ⇒ Free.
        assert_eq!(resolve_plan(Plan::Free, None, start), Plan::Free);
        // Fresh install ⇒ Trial, no account needed.
        assert_eq!(resolve_plan(Plan::Free, Some(start), start), Plan::Trial);
        // Expired ⇒ back to Free.
        assert_eq!(
            resolve_plan(Plan::Free, Some(start), start + 14 * DAY),
            Plan::Free
        );
        // A paid plan is never downgraded by trial state, expired or not.
        assert_eq!(
            resolve_plan(Plan::Pro, Some(start), start + 99 * DAY),
            Plan::Pro
        );
        assert_eq!(resolve_plan(Plan::Team, None, start), Plan::Team);
    }

    #[test]
    fn trial_countdown_is_only_attached_to_the_trial_plan() {
        let e = Entitlement::for_plan(Plan::Trial).with_trial_days_left(Some(6));
        assert_eq!(e.trial_days_left, Some(6));
        // Pro must never render a countdown.
        let p = Entitlement::for_plan(Plan::Pro).with_trial_days_left(Some(6));
        assert_eq!(p.trial_days_left, None);
    }
}

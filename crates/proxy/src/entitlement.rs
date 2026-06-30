//! Plan entitlements — what each subscription tier unlocks.
//!
//! Detection runs entirely locally, so gating is enforced here in the proxy
//! from the plan the desktop app reports after it authenticates (identity only —
//! no chat content is ever involved). The plan → capability mapping lives here
//! as the single source of truth and is derived from the plan, never trusted
//! from client-sent flags. The `plans.features` rows in Supabase mirror it for
//! display on the site.

use serde::{Deserialize, Serialize};

/// Subscription tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    #[default]
    Free,
    Pro,
    Team,
}

impl Plan {
    /// Parse a plan id (e.g. from `/me`). Unknown / empty ⇒ Free.
    pub fn from_id(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "pro" => Plan::Pro,
            "team" => Plan::Team,
            _ => Plan::Free,
        }
    }
}

/// The capabilities unlocked for a plan. Derived from `Plan` so there is exactly
/// one source of truth.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Entitlement {
    pub plan: Plan,
    /// Max sessions surfaced/tracked at once (`None` = unlimited).
    #[serde(rename = "maxSessions")]
    pub max_sessions: Option<usize>,
    /// Session drift map (the rolling drift-score history sparkline).
    #[serde(rename = "driftMap")]
    pub drift_map: bool,
    /// Opt-in proxy auto-re-anchor (inject the preamble on RED).
    #[serde(rename = "autoReanchor")]
    pub auto_reanchor: bool,
    /// Shared standing orders across a team.
    #[serde(rename = "teamSharing")]
    pub team_sharing: bool,
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
                max_sessions: Some(1),
                drift_map: false,
                auto_reanchor: false,
                team_sharing: false,
            },
            Plan::Pro => Entitlement {
                plan,
                max_sessions: None,
                drift_map: true,
                auto_reanchor: true,
                team_sharing: false,
            },
            Plan::Team => Entitlement {
                plan,
                max_sessions: None,
                drift_map: true,
                auto_reanchor: true,
                team_sharing: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_is_the_locked_default() {
        let e = Entitlement::default();
        assert_eq!(e.plan, Plan::Free);
        assert_eq!(e.max_sessions, Some(1));
        assert!(!e.drift_map && !e.auto_reanchor && !e.team_sharing);
    }

    #[test]
    fn pro_unlocks_core_paid_features_but_not_team() {
        let e = Entitlement::for_plan(Plan::Pro);
        assert_eq!(e.max_sessions, None);
        assert!(e.drift_map && e.auto_reanchor);
        assert!(!e.team_sharing);
    }

    #[test]
    fn team_unlocks_everything() {
        let e = Entitlement::for_plan(Plan::Team);
        assert!(e.team_sharing && e.drift_map && e.auto_reanchor);
        assert_eq!(e.max_sessions, None);
    }

    #[test]
    fn plan_parsing_is_lenient() {
        assert_eq!(Plan::from_id("PRO"), Plan::Pro);
        assert_eq!(Plan::from_id(" team "), Plan::Team);
        assert_eq!(Plan::from_id("nonsense"), Plan::Free);
        assert_eq!(Plan::from_id(""), Plan::Free);
    }
}

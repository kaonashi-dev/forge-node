use std::collections::{BTreeMap, BTreeSet};

use domain::{Finding, FindingReaction, ReviewSuppression, Severity, SuppressionKind};

const WINDOW_SECS: i64 = 30 * 86_400;
const RULE_DISMISSALS: usize = 3;
const STYLE_DISMISSALS: usize = 5;
const STYLE: &str = "style";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feedback {
    pub fingerprint: String,
    pub rule_id: Option<String>,
    pub category: String,
    pub severity: Severity,
    pub reaction: FindingReaction,
    /// Unix seconds.
    pub created_at: i64,
}

/// A rule with 3 dismissals in the last 30 days and no 👍 in them, `style`
/// with 5, and every dismissed fingerprint at the severity it was dismissed at.
#[must_use]
pub fn suppressions(feedback: &[Feedback], now_unix: i64) -> Vec<ReviewSuppression> {
    let recent = |f: &&Feedback| now_unix - f.created_at <= WINDOW_SECS;
    let mut rule_dismissals: BTreeMap<&str, usize> = BTreeMap::new();
    let mut liked_rules: BTreeSet<&str> = BTreeSet::new();
    let mut style_dismissals = 0;
    let mut style_liked = false;
    for f in feedback.iter().filter(recent) {
        match f.reaction {
            FindingReaction::Dismiss => {
                if let Some(rule) = &f.rule_id {
                    *rule_dismissals.entry(rule).or_default() += 1;
                }
                if f.category == STYLE {
                    style_dismissals += 1;
                }
            }
            FindingReaction::Up => {
                if let Some(rule) = &f.rule_id {
                    liked_rules.insert(rule);
                }
                style_liked |= f.category == STYLE;
            }
            FindingReaction::Down => {}
        }
    }
    let mut list: Vec<ReviewSuppression> = rule_dismissals
        .into_iter()
        .filter(|(rule, count)| *count >= RULE_DISMISSALS && !liked_rules.contains(rule))
        .map(|(rule, count)| ReviewSuppression {
            kind: SuppressionKind::Rule,
            value: rule.to_owned(),
            severity: None,
            reason: format!("{count} dismissals in 30 days"),
        })
        .collect();
    if style_dismissals >= STYLE_DISMISSALS && !style_liked {
        list.push(ReviewSuppression {
            kind: SuppressionKind::Category,
            value: STYLE.to_owned(),
            severity: None,
            reason: format!("{style_dismissals} dismissals in 30 days"),
        });
    }
    let dismissed: BTreeSet<(&str, Severity)> = feedback
        .iter()
        .filter(|f| f.reaction == FindingReaction::Dismiss)
        .map(|f| (f.fingerprint.as_str(), f.severity))
        .collect();
    list.extend(
        dismissed
            .into_iter()
            .map(|(fingerprint, severity)| ReviewSuppression {
                kind: SuppressionKind::Fingerprint,
                value: fingerprint.to_owned(),
                severity: Some(severity),
                reason: "dismissed before".into(),
            }),
    );
    list
}

/// P0 and `security` findings are never suppressed, whatever the list says.
pub(crate) fn is_suppressed(finding: &Finding, suppressions: &[ReviewSuppression]) -> bool {
    if finding.severity == Severity::P0 || finding.category == "security" {
        return false;
    }
    suppressions.iter().any(|s| match s.kind {
        SuppressionKind::Rule => finding.rule_id.as_deref() == Some(s.value.as_str()),
        SuppressionKind::Category => finding.category == s.value,
        SuppressionKind::Fingerprint => {
            finding.fingerprint == s.value && s.severity == Some(finding.severity)
        }
    })
}

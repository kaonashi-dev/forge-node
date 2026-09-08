//! The one place a feature's status is decided.
//!
//! Before this table, `set_status` took any string from any caller and
//! `advance` accepted any action from any status. `RunHarnessStep{Implement}`
//! on a `spec_ready` feature ran the implementer without an approval, and
//! replaying an `ApproveSpec` appended a second `human_gate_resolved` and
//! launched a second implementer in the same worktree. The gate was a
//! convention, and `validate.ts` only complained about the wreckage afterwards.
//!
//! So: [`transition`] is a **pure function** over a row, the repository's rules
//! and one trigger. It returns the row it should become, the events that record
//! why, and the step the coordinator must start next — or a refusal. It reads
//! no file, spawns nothing and is therefore the part that can be tested
//! exhaustively. [`crate::apply`] is the thin shell that loads under the lock,
//! calls this, and writes.
//!
//! ```text
//! pending      --spec job ok-->              spec_ready   (gate opened)
//! spec_ready   --ApproveSpec (human)-->      in_progress  (implement next)
//! spec_ready   --ReviseSpec (human)-->       pending      (spec next)
//! in_progress  --implement job ok-->         in_progress  (review next)
//! in_review    --APPROVED-->                 done
//! in_review    --CHANGES_REQUESTED, rounds<max-->  in_progress (implement next)
//! any running  --job failed, attempts<max--> same status  (retry the step)
//! any running  --job failed, attempts==max-->blocked
//! any          --Block (human)-->            blocked
//! blocked      --RetryStep (human)-->        the step's running status
//! done|blocked --Reopen (human)-->           pending
//! ```

use domain::{HarnessAttempt, HarnessFeature, HarnessStep};
use serde_json::{Map, Value};

use crate::HarnessError;

pub const PENDING: &str = "pending";
pub const SPEC_READY: &str = "spec_ready";
pub const IN_PROGRESS: &str = "in_progress";
pub const IN_REVIEW: &str = "in_review";
pub const DONE: &str = "done";
pub const BLOCKED: &str = "blocked";

/// The budgets `harness/features.json` states, with this crate's defaults.
///
/// Every field here is read by code in this file. A rule nothing reads is a
/// rule that lies, which is why `require_human_spec_approval` and
/// `require_green_gate_to_close` — both inert since they were written — are
/// enforced below rather than left as documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HarnessRules {
    /// The gate a machine may not skip. `false` lets a written spec go straight
    /// to the implementer, which is the only supported way to run unattended.
    pub require_human_spec_approval: bool,
    /// A feature may not reach `done` until the implementer left its report —
    /// the artefact that says the gate command was run and was green.
    pub require_green_gate_to_close: bool,
    /// How many times the reviewer may send the work back.
    pub max_review_rounds: u32,
    /// How many failed `scripts/dev check` runs an implementation may spend.
    pub max_gate_attempts: u32,
    /// How many times a step may be *restarted* after a job failed.
    pub max_step_attempts: u32,
    /// Wall clock for one attempt, after which the job is cancelled.
    pub max_step_minutes: u32,
    /// Silence after which an attempt is reported stale — a warning, never a
    /// kill: a long implementation legitimately thinks for minutes.
    pub stale_after_minutes: u32,
}

impl Default for HarnessRules {
    fn default() -> Self {
        Self {
            require_human_spec_approval: true,
            require_green_gate_to_close: true,
            max_review_rounds: 2,
            max_gate_attempts: 3,
            max_step_attempts: 2,
            max_step_minutes: 90,
            stale_after_minutes: 10,
        }
    }
}

impl HarnessRules {
    /// Read the `rules` object of `features.json`, keeping this crate's default
    /// for anything the repository does not state.
    #[must_use]
    pub fn from_json(rules: Option<&Value>) -> Self {
        let mut out = Self::default();
        let Some(rules) = rules else {
            return out;
        };
        let flag =
            |key: &str, current: bool| rules.get(key).and_then(Value::as_bool).unwrap_or(current);
        let count = |key: &str, current: u32| {
            rules
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(current)
        };
        out.require_human_spec_approval = flag(
            "require_human_spec_approval",
            out.require_human_spec_approval,
        );
        out.require_green_gate_to_close = flag(
            "require_green_gate_to_close",
            out.require_green_gate_to_close,
        );
        out.max_review_rounds = count("max_review_rounds", out.max_review_rounds);
        out.max_gate_attempts = count("max_gate_attempts", out.max_gate_attempts);
        out.max_step_attempts = count("max_step_attempts", out.max_step_attempts);
        out.max_step_minutes = count("max_step_minutes", out.max_step_minutes);
        out.stale_after_minutes = count("stale_after_minutes", out.stale_after_minutes);
        out
    }
}

/// Something that happened to a feature, in the vocabulary of observable facts.
///
/// Each variant is either a human decision or something the daemon *watched*:
/// a process exited, a schema parsed. Nothing here is a guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HarnessTrigger {
    /// A step's job is about to be started. Refused unless the table allows
    /// this step from this status — which is the gate, expressed once.
    StartStep {
        step: HarnessStep,
        job: Option<String>,
        provider: Option<String>,
        /// `cli` today; `acp` when the second transport lands.
        transport: Option<String>,
        /// Skip the table check, and say so in the log. The GUI never sets it.
        force: bool,
    },
    /// The spec step's job exited 0.
    SpecWritten,
    /// The implement step's job exited 0.
    Implemented,
    /// The reviewer answered.
    ReviewVerdict {
        approved: bool,
        /// Where the answer came from: `schema` or `file`.
        source: &'static str,
    },
    /// A step's job ended in failure, and the coordinator wants the budget.
    StepFailed { step: HarnessStep, reason: String },
    /// The reviewer answered nothing, or two things.
    ReviewUnreadable { reason: String },
    /// A human approved the spec at the gate.
    ApproveSpec,
    /// A human sent the spec back to be rewritten.
    ReviseSpec { reason: Option<String> },
    /// A human blocked the feature.
    Block { reason: String },
    /// A human restarted the step a blocked feature died on.
    RetryStep,
    /// A human took a `done` or `blocked` feature back to `pending`.
    Reopen,
}

/// What a transition decided.
#[derive(Clone, Debug)]
pub struct Transitioned {
    /// The row as it should now be written.
    pub feature: HarnessFeature,
    /// The step the coordinator must start, once the row is on disk.
    pub next_step: Option<HarnessStep>,
    /// The trail, in order. `(type, extra fields)`.
    pub events: Vec<(String, Map<String, Value>)>,
}

/// The statuses a step may be started from.
///
/// `Review` is startable from `in_progress` because that is exactly what the
/// chain does when an implementation exits 0: the row is still `in_progress`
/// at the moment the review is launched, and the launch is what moves it.
#[must_use]
pub fn allowed_from(step: HarnessStep) -> &'static [&'static str] {
    match step {
        HarnessStep::Spec => &[PENDING],
        HarnessStep::Implement => &[IN_PROGRESS],
        HarnessStep::Review => &[IN_PROGRESS, IN_REVIEW],
        // A step this build does not know may not be started by guessing.
        _ => &[],
    }
}

/// How many attempts of `step` this feature has already recorded.
#[must_use]
pub fn attempts_of(feature: &HarnessFeature, step: HarnessStep) -> u32 {
    feature
        .attempts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|attempt| attempt.step == step)
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// The attempt that is still on record as running, if any.
#[must_use]
pub fn live_attempt(feature: &HarnessFeature) -> Option<&HarnessAttempt> {
    feature
        .attempts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .rev()
        .find(|attempt| attempt.is_live())
}

/// The step a running feature is on, from its status.
#[must_use]
pub fn step_of_status(status: &str) -> Option<HarnessStep> {
    match status {
        PENDING => Some(HarnessStep::Spec),
        IN_PROGRESS => Some(HarnessStep::Implement),
        IN_REVIEW => Some(HarnessStep::Review),
        _ => None,
    }
}

fn refuse(message: impl Into<String>) -> HarnessError {
    HarnessError::Invalid(message.into())
}

fn event(kind: &str, pairs: &[(&str, Value)]) -> (String, Map<String, Value>) {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_owned(), value.clone());
    }
    (kind.to_owned(), map)
}

/// Settle the feature's live attempt, if it has one.
fn settle_live(feature: &mut HarnessFeature, now: &str, outcome: &str, detail: Option<&str>) {
    let Some(attempts) = feature.attempts.as_mut() else {
        return;
    };
    if let Some(attempt) = attempts.iter_mut().rev().find(|a| a.is_live()) {
        attempt.settled_at = Some(now.to_owned());
        attempt.outcome = Some(outcome.to_owned());
        if let Some(detail) = detail {
            attempt.detail = Some(detail.to_owned());
        }
    }
}

/// Apply one trigger to one row.
///
/// `now` is passed in rather than read so the function stays pure and its
/// tests stay deterministic.
///
/// # Errors
///
/// [`HarnessError::Invalid`] when the table has no row for this trigger from
/// this status — the refusal that makes the gate real.
#[allow(clippy::too_many_lines)]
pub fn transition(
    feature: &HarnessFeature,
    rules: &HarnessRules,
    trigger: &HarnessTrigger,
    now: &str,
) -> Result<Transitioned, HarnessError> {
    let mut next = feature.clone();
    let mut events: Vec<(String, Map<String, Value>)> = Vec::new();
    let mut next_step = None;

    match trigger {
        HarnessTrigger::StartStep {
            step,
            job,
            provider,
            transport,
            force,
        } => {
            let allowed = allowed_from(*step);
            if !allowed.contains(&feature.status.as_str()) {
                if !force {
                    return Err(refuse(format!(
                        "a {step:?} step cannot start from status '{}'; the harness runs \
                         {step:?} from {}",
                        feature.status,
                        if allowed.is_empty() {
                            "no status this build knows".to_owned()
                        } else {
                            allowed.join(" or ")
                        }
                    )));
                }
                events.push(event(
                    "gate_bypassed",
                    &[
                        ("step", format!("{step:?}").into()),
                        ("from", feature.status.clone().into()),
                    ],
                ));
            }
            let n = attempts_of(feature, *step) + 1;
            // A step that is being started again settles whatever the previous
            // attempt left open: nothing else is going to.
            settle_live(&mut next, now, "superseded", None);
            next.attempts
                .get_or_insert_with(Vec::new)
                .push(HarnessAttempt {
                    step: *step,
                    n,
                    job: job.clone(),
                    provider: provider.clone(),
                    transport: transport.clone().or_else(|| Some("cli".to_owned())),
                    started_at: now.to_owned(),
                    settled_at: None,
                    outcome: None,
                    detail: None,
                });
            next.status = step.running_status().to_owned();
            next.blocked_reason = None;
            let mut data = Map::new();
            data.insert("attempt".into(), n.into());
            if let Some(job) = job {
                data.insert("job".into(), job.clone().into());
            }
            if let Some(provider) = provider {
                data.insert("provider".into(), provider.clone().into());
            }
            data.insert(
                "transport".into(),
                transport.clone().unwrap_or_else(|| "cli".to_owned()).into(),
            );
            events.push((step.started_event().to_owned(), data));
        }

        HarnessTrigger::SpecWritten => {
            if feature.status != PENDING {
                return Err(refuse(format!(
                    "a spec can only be accepted from 'pending'; this feature is '{}'",
                    feature.status
                )));
            }
            settle_live(&mut next, now, "succeeded", None);
            if rules.require_human_spec_approval {
                next.status = SPEC_READY.into();
                events.push(event(
                    "human_gate_opened",
                    &[("gate", "spec_approval".into())],
                ));
            } else {
                // The one way the gate is skipped, and it is the repository's
                // own written choice — not a caller's.
                next.status = IN_PROGRESS.into();
                events.push(event(
                    "human_gate_opened",
                    &[
                        ("gate", "spec_approval".into()),
                        ("auto_approved", Value::Bool(true)),
                    ],
                ));
                next_step = Some(HarnessStep::Implement);
            }
        }

        HarnessTrigger::Implemented => {
            if feature.status != IN_PROGRESS {
                return Err(refuse(format!(
                    "an implementation can only settle from 'in_progress'; this feature \
                     is '{}'",
                    feature.status
                )));
            }
            settle_live(&mut next, now, "succeeded", None);
            events.push(event("impl_done", &[]));
            next_step = Some(HarnessStep::Review);
        }

        HarnessTrigger::ReviewVerdict { approved, source } => {
            if feature.status != IN_REVIEW {
                return Err(refuse(format!(
                    "a verdict can only settle from 'in_review'; this feature is '{}'",
                    feature.status
                )));
            }
            settle_live(&mut next, now, "succeeded", None);
            events.push(event(
                "review_verdict",
                &[
                    (
                        "verdict",
                        Value::from(if *approved {
                            "APPROVED"
                        } else {
                            "CHANGES_REQUESTED"
                        }),
                    ),
                    ("source", Value::from(*source)),
                ],
            ));
            if *approved {
                next.status = DONE.into();
                events.push(event("feature_done", &[]));
            } else {
                let rounds = feature.review_rounds.unwrap_or(0) + 1;
                next.review_rounds = Some(rounds);
                if rounds < rules.max_review_rounds {
                    next.status = IN_PROGRESS.into();
                    next_step = Some(HarnessStep::Implement);
                } else {
                    let reason = format!(
                        "the review requested changes and max_review_rounds \
                         ({}) is spent",
                        rules.max_review_rounds
                    );
                    next.status = BLOCKED.into();
                    next.blocked_reason = Some(reason.clone());
                    events.push(event("feature_blocked", &[("reason", reason.into())]));
                }
            }
        }

        HarnessTrigger::ReviewUnreadable { reason } => {
            settle_live(&mut next, now, "failed", Some(reason));
            next.status = BLOCKED.into();
            next.blocked_reason = Some(reason.clone());
            events.push(event(
                "feature_blocked",
                &[("reason", reason.clone().into())],
            ));
        }

        HarnessTrigger::StepFailed { step, reason } => {
            // Only the step the feature is running can fail it. A job that
            // outlived a human `Block` still exits, and settling that exit
            // here would spend a retry and put a blocked feature back to work.
            if feature.status != step.running_status() {
                return Err(refuse(format!(
                    "a {step:?} failure cannot settle a feature that is '{}'",
                    feature.status
                )));
            }
            settle_live(&mut next, now, "failed", Some(reason));
            // Orca's circuit breaker: a retry is a new attempt, and there are
            // only so many. `max_step_attempts` counts *restarts*, so the first
            // run plus one retry is the default.
            let spent = attempts_of(feature, *step);
            if spent < rules.max_step_attempts {
                // The status stays whatever the step runs under, so the retry
                // is a legal `StartStep` for the coordinator.
                next.status = step.running_status().to_owned();
                events.push(event(
                    "step_retried",
                    &[
                        ("step", format!("{step:?}").into()),
                        ("after", spent.into()),
                        ("reason", reason.clone().into()),
                    ],
                ));
                next_step = Some(*step);
            } else {
                let reason = format!(
                    "{reason} — max_step_attempts ({}) is spent",
                    rules.max_step_attempts
                );
                next.status = BLOCKED.into();
                next.blocked_reason = Some(reason.clone());
                events.push(event("feature_blocked", &[("reason", reason.into())]));
            }
        }

        HarnessTrigger::ApproveSpec => {
            // Idempotent by refusal rather than by silence: a second approval
            // used to append a second gate resolution and start a second
            // implementer in the same worktree.
            if feature.status != SPEC_READY {
                return Err(refuse(format!(
                    "there is no open spec gate on this feature; it is '{}'",
                    feature.status
                )));
            }
            next.status = IN_PROGRESS.into();
            next.blocked_reason = None;
            events.push(event(
                "human_gate_resolved",
                &[
                    ("gate", "spec_approval".into()),
                    ("decision", "approve".into()),
                ],
            ));
            next_step = Some(HarnessStep::Implement);
        }

        HarnessTrigger::ReviseSpec { reason } => {
            if feature.status != SPEC_READY {
                return Err(refuse(format!(
                    "there is no open spec gate on this feature; it is '{}'",
                    feature.status
                )));
            }
            // `gate_attempts` is untouched: it is the implementer's
            // `scripts/dev check` budget, and a human sending the spec back
            // is not a failed build.
            next.status = PENDING.into();
            let mut data = Map::new();
            data.insert("gate".into(), "spec_approval".into());
            data.insert("decision".into(), "revise".into());
            if let Some(reason) = reason {
                data.insert("reason".into(), reason.clone().into());
            }
            events.push(("human_gate_resolved".to_owned(), data));
            next_step = Some(HarnessStep::Spec);
        }

        HarnessTrigger::Block { reason } => {
            settle_live(&mut next, now, "blocked", Some(reason));
            next.status = BLOCKED.into();
            next.blocked_reason = Some(reason.clone());
            events.push(event(
                "feature_blocked",
                &[("reason", reason.clone().into())],
            ));
        }

        HarnessTrigger::RetryStep => {
            if feature.status != BLOCKED {
                return Err(refuse(format!(
                    "only a blocked feature is retried; this one is '{}'",
                    feature.status
                )));
            }
            // Where it died, from the last attempt on record; a feature blocked
            // before it ever ran a step starts at the spec.
            let step = feature
                .attempts
                .as_deref()
                .unwrap_or_default()
                .last()
                .map_or(HarnessStep::Spec, |attempt| attempt.step);
            next.status = step.running_status().to_owned();
            next.blocked_reason = None;
            events.push(event(
                "step_retried",
                &[("step", format!("{step:?}").into()), ("by", "human".into())],
            ));
            next_step = Some(step);
        }

        HarnessTrigger::Reopen => {
            if feature.status != DONE && feature.status != BLOCKED {
                return Err(refuse(format!(
                    "only a finished or abandoned feature is reopened; this one is '{}'",
                    feature.status
                )));
            }
            next.status = PENDING.into();
            next.blocked_reason = None;
            next.review_rounds = Some(0);
            events.push(event("feature_reopened", &[]));
        }
    }

    // The gate that was never read. `done` means the implementer left the
    // report that says the gate command ran green; a review that approves work
    // with no report approves something nobody can point at.
    if rules.require_green_gate_to_close
        && next.status == DONE
        && feature.gate_attempts.unwrap_or(0) > rules.max_gate_attempts
    {
        let reason = format!(
            "gate_attempts ({}) exceeds max_gate_attempts ({}); \
             require_green_gate_to_close refuses to close this feature",
            feature.gate_attempts.unwrap_or(0),
            rules.max_gate_attempts
        );
        next.status = BLOCKED.into();
        next.blocked_reason = Some(reason.clone());
        events.retain(|(kind, _)| kind != "feature_done");
        events.push(event("feature_blocked", &[("reason", reason.into())]));
        next_step = None;
    }

    next.revision = Some(feature.revision.unwrap_or(0) + 1);
    Ok(Transitioned {
        feature: next,
        next_step,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-09-01T00:00:00Z";

    fn feature(status: &str) -> HarnessFeature {
        HarnessFeature {
            id: 7,
            slug: "a-feature".into(),
            status: status.into(),
            review_rounds: Some(0),
            gate_attempts: Some(0),
            ..HarnessFeature::default()
        }
    }

    fn start(step: HarnessStep) -> HarnessTrigger {
        HarnessTrigger::StartStep {
            step,
            job: Some("job-1".into()),
            provider: Some("claude".into()),
            transport: None,
            force: false,
        }
    }

    /// H1, the headline defect: `RunHarnessStep{Implement}` on a feature
    /// waiting at the gate ran the implementer without an approval.
    #[test]
    fn implement_cannot_start_before_the_gate_is_resolved() {
        let rules = HarnessRules::default();
        let error = transition(
            &feature(SPEC_READY),
            &rules,
            &start(HarnessStep::Implement),
            NOW,
        )
        .expect_err("the gate must refuse");
        assert!(error.to_string().contains("spec_ready"), "{error}");
    }

    #[test]
    fn approving_the_spec_starts_the_implementer_exactly_once() {
        let rules = HarnessRules::default();
        let approved = transition(
            &feature(SPEC_READY),
            &rules,
            &HarnessTrigger::ApproveSpec,
            NOW,
        )
        .expect("the gate is open");
        assert_eq!(approved.feature.status, IN_PROGRESS);
        assert_eq!(approved.next_step, Some(HarnessStep::Implement));
        // Replayed against the row it produced, the second approval refuses:
        // there is no open gate any more.
        assert!(transition(&approved.feature, &rules, &HarnessTrigger::ApproveSpec, NOW).is_err());
    }

    /// `require_human_spec_approval` was written in `features.json` and read by
    /// nothing. It is the only way the machine may skip a human.
    #[test]
    fn the_gate_is_skipped_only_when_the_repository_says_so() {
        let mut rules = HarnessRules::default();
        let gated = transition(&feature(PENDING), &rules, &HarnessTrigger::SpecWritten, NOW)
            .expect("a written spec");
        assert_eq!(gated.feature.status, SPEC_READY);
        assert_eq!(gated.next_step, None);

        rules.require_human_spec_approval = false;
        let ungated = transition(&feature(PENDING), &rules, &HarnessTrigger::SpecWritten, NOW)
            .expect("a written spec");
        assert_eq!(ungated.feature.status, IN_PROGRESS);
        assert_eq!(ungated.next_step, Some(HarnessStep::Implement));
    }

    #[test]
    fn a_rejected_review_goes_round_again_until_the_budget_is_spent() {
        let rules = HarnessRules {
            max_review_rounds: 2,
            ..HarnessRules::default()
        };
        let mut row = feature(IN_REVIEW);
        let again = transition(
            &row,
            &rules,
            &HarnessTrigger::ReviewVerdict {
                approved: false,
                source: "schema",
            },
            NOW,
        )
        .expect("a verdict");
        assert_eq!(again.feature.status, IN_PROGRESS);
        assert_eq!(again.feature.review_rounds, Some(1));
        assert_eq!(again.next_step, Some(HarnessStep::Implement));

        row.review_rounds = Some(1);
        row.status = IN_REVIEW.into();
        let spent = transition(
            &row,
            &rules,
            &HarnessTrigger::ReviewVerdict {
                approved: false,
                source: "file",
            },
            NOW,
        )
        .expect("a verdict");
        assert_eq!(spent.feature.status, BLOCKED);
        assert!(spent.next_step.is_none());
    }

    /// H5: a rate limit and "I could not do this" were the same outcome, and
    /// both blocked the feature forever.
    #[test]
    fn a_failed_job_is_retried_before_it_blocks() {
        let rules = HarnessRules {
            max_step_attempts: 2,
            ..HarnessRules::default()
        };
        let started = transition(
            &feature(IN_PROGRESS),
            &rules,
            &start(HarnessStep::Implement),
            NOW,
        )
        .expect("a legal start");
        let failed = transition(
            &started.feature,
            &rules,
            &HarnessTrigger::StepFailed {
                step: HarnessStep::Implement,
                reason: "the agent exited 1".into(),
            },
            NOW,
        )
        .expect("a settled failure");
        assert_eq!(failed.next_step, Some(HarnessStep::Implement));
        assert_eq!(failed.feature.status, IN_PROGRESS);
        assert_eq!(
            failed.feature.attempts.as_deref().unwrap()[0]
                .outcome
                .as_deref(),
            Some("failed")
        );

        // Second attempt, second failure: the budget is spent.
        let retried = transition(&failed.feature, &rules, &start(HarnessStep::Implement), NOW)
            .expect("a legal retry");
        let blocked = transition(
            &retried.feature,
            &rules,
            &HarnessTrigger::StepFailed {
                step: HarnessStep::Implement,
                reason: "the agent exited 1".into(),
            },
            NOW,
        )
        .expect("a settled failure");
        assert_eq!(blocked.feature.status, BLOCKED);
        assert!(blocked.next_step.is_none());
        assert!(blocked
            .feature
            .blocked_reason
            .as_deref()
            .unwrap()
            .contains("max_step_attempts"));
    }

    /// A job that outlives a human `Block` still exits; its failure must not
    /// spend a retry and put the feature back to work.
    #[test]
    fn a_failure_cannot_settle_a_feature_that_is_not_running_that_step() {
        let rules = HarnessRules::default();
        let started = transition(
            &feature(IN_PROGRESS),
            &rules,
            &start(HarnessStep::Implement),
            NOW,
        )
        .expect("a legal start");
        let blocked = transition(
            &started.feature,
            &rules,
            &HarnessTrigger::Block {
                reason: "stop".into(),
            },
            NOW,
        )
        .expect("a block");
        let error = transition(
            &blocked.feature,
            &rules,
            &HarnessTrigger::StepFailed {
                step: HarnessStep::Implement,
                reason: "the agent exited 1".into(),
            },
            NOW,
        )
        .expect_err("a blocked feature is not running anything");
        assert!(error.to_string().contains("blocked"), "{error}");
    }

    /// `gate_attempts` is the implementer's `scripts/dev check` budget. Counting
    /// a revision as a failed build let four revisions block a feature at the
    /// moment its review approved it.
    #[test]
    fn revising_the_spec_does_not_spend_the_gate_budget() {
        let rules = HarnessRules::default();
        let mut row = feature(SPEC_READY);
        row.gate_attempts = Some(1);
        let revised = transition(
            &row,
            &rules,
            &HarnessTrigger::ReviseSpec { reason: None },
            NOW,
        )
        .expect("the gate is open");
        assert_eq!(revised.feature.status, PENDING);
        assert_eq!(revised.feature.gate_attempts, Some(1));
        assert_eq!(revised.next_step, Some(HarnessStep::Spec));
    }

    #[test]
    fn a_blocked_feature_is_retried_on_the_step_it_died_on() {
        let rules = HarnessRules::default();
        let started = transition(
            &feature(IN_REVIEW),
            &rules,
            &start(HarnessStep::Review),
            NOW,
        )
        .expect("a legal start");
        let blocked = transition(
            &started.feature,
            &rules,
            &HarnessTrigger::Block {
                reason: "no verdict".into(),
            },
            NOW,
        )
        .expect("a block");
        assert_eq!(blocked.feature.status, BLOCKED);

        let retried =
            transition(&blocked.feature, &rules, &HarnessTrigger::RetryStep, NOW).expect("a retry");
        assert_eq!(retried.next_step, Some(HarnessStep::Review));
        assert_eq!(retried.feature.status, IN_REVIEW);
        assert_eq!(retried.feature.blocked_reason, None);
    }

    /// Every write bumps the revision, which is what makes a replayed decision
    /// detectable rather than merely unlikely.
    #[test]
    fn every_transition_bumps_the_revision() {
        let rules = HarnessRules::default();
        let first = transition(&feature(PENDING), &rules, &start(HarnessStep::Spec), NOW)
            .expect("a legal start");
        assert_eq!(first.feature.revision, Some(1));
        let second = transition(&first.feature, &rules, &HarnessTrigger::SpecWritten, NOW)
            .expect("a written spec");
        assert_eq!(second.feature.revision, Some(2));
    }

    /// `require_green_gate_to_close` was the other rule nothing read.
    #[test]
    fn a_feature_over_its_gate_budget_cannot_be_closed() {
        let rules = HarnessRules::default();
        let mut row = feature(IN_REVIEW);
        row.gate_attempts = Some(rules.max_gate_attempts + 1);
        let settled = transition(
            &row,
            &rules,
            &HarnessTrigger::ReviewVerdict {
                approved: true,
                source: "schema",
            },
            NOW,
        )
        .expect("a verdict");
        assert_eq!(settled.feature.status, BLOCKED);
        assert!(!settled
            .events
            .iter()
            .any(|(kind, _)| kind == "feature_done"));
    }

    #[test]
    fn forcing_a_step_is_allowed_and_recorded() {
        let rules = HarnessRules::default();
        let forced = transition(
            &feature(SPEC_READY),
            &rules,
            &HarnessTrigger::StartStep {
                step: HarnessStep::Implement,
                job: None,
                provider: None,
                transport: None,
                force: true,
            },
            NOW,
        )
        .expect("force skips the table");
        assert!(forced
            .events
            .iter()
            .any(|(kind, _)| kind == "gate_bypassed"));
        assert_eq!(forced.feature.status, IN_PROGRESS);
    }

    #[test]
    fn a_new_attempt_settles_the_one_it_replaces() {
        let rules = HarnessRules::default();
        let first = transition(&feature(PENDING), &rules, &start(HarnessStep::Spec), NOW)
            .expect("a legal start");
        let second = transition(&first.feature, &rules, &start(HarnessStep::Spec), NOW)
            .expect("a legal restart");
        let attempts = second.feature.attempts.as_deref().unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].outcome.as_deref(), Some("superseded"));
        assert_eq!(attempts[1].n, 2);
        assert!(attempts[1].is_live());
    }
}

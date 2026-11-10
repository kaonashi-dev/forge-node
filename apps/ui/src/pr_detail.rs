//! One pull request opened as a centre view, read from the last refresh.
//!
//! The view is forbidden from asking the host anything: checks, review threads
//! and the diff are not in the cached row, so a tab for them would be a promise
//! the daemon does not keep. Everything here answers from the snapshot.

use domain::{PullRequest, PullRequestRelations, ReviewDecision, Timestamp};
use rune::{El, ScrollSpec, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Markdown};

use crate::app::AppState;
use crate::pr_filters::{Tone, decision_label};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Description,
    Details,
}

/// Open or draft — the daemon lists open pull requests, so there is no third.
#[must_use]
pub fn state_chip(pr: &PullRequest) -> (&'static str, Tone) {
    if pr.is_draft {
        ("Draft", Tone::Neutral)
    } else {
        ("Open", Tone::Success)
    }
}

/// How the viewer is involved, in the order that decides what to do next.
#[must_use]
pub fn relation_chips(relations: &PullRequestRelations) -> Vec<(&'static str, Tone)> {
    let mut chips = Vec::new();
    if relations.review_requested {
        chips.push(("Your review requested", Tone::Warning));
    }
    if relations.assigned {
        chips.push(("Assigned to you", Tone::Neutral));
    }
    if relations.authored {
        chips.push(("You opened this", Tone::Neutral));
    }
    chips
}

/// `owner/repo` → its two halves, for a trail.
#[must_use]
pub fn split_repository(repository: &str) -> (Option<&str>, &str) {
    repository.rfind('/').map_or((None, repository), |at| {
        (Some(&repository[..at]), &repository[at + 1..])
    })
}

/// Six hex digits and nothing else: the value comes off the network and lands
/// in a fill, and a malformed one must not paint anything.
#[must_use]
pub fn label_color(color: &str) -> Option<rune::Color> {
    let value = color.trim().trim_start_matches('#');
    (value.len() == 6 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .then(|| rune::color::hex(&format!("#{value}")))
        .flatten()
}

/// `36 files changed`, `1 file changed`, `No files changed`.
#[must_use]
pub fn file_summary(count: u32) -> String {
    match count {
        0 => "No files changed".to_string(),
        1 => "1 file changed".to_string(),
        count => format!("{count} files changed"),
    }
}

/// `4 comments`, `1 comment`, `No comments`.
#[must_use]
pub fn comment_summary(count: u32) -> String {
    match count {
        0 => "No comments".to_string(),
        1 => "1 comment".to_string(),
        count => format!("{count} comments"),
    }
}

/// The one letter that stands in for a face nobody downloaded.
#[must_use]
pub fn monogram(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map(|first| first.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

/// Coarse age of `at`, for a line that has room for `5m` and not a timestamp.
#[must_use]
pub fn relative_age(at: Timestamp, now: Timestamp) -> String {
    let minutes =
        ((now.as_offset().unix_timestamp() - at.as_offset().unix_timestamp()) / 60).max(0);
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}

/// The date a person reads. RFC-3339's first ten bytes are the calendar date
/// every host writes, and a malformed one falls back to the whole string.
#[must_use]
pub fn readable_date(at: Timestamp) -> String {
    let text = at.to_rfc3339();
    text.get(..10).unwrap_or(&text).to_string()
}

fn chip(k: &Kit, label: &str, tone: Tone) -> El {
    crate::ds::chip(k, &crate::theme::roles(k), label, tone)
}

fn section(k: &Kit, title: &str, body: El) -> El {
    col()
        .w_full()
        .gap(crate::ds::SPACE_1)
        .child(crate::ds::section_header(k, title, None, None))
        .child(body)
}

fn people(k: &Kit, title: &str, names: &[String], empty: &str) -> El {
    let body = if names.is_empty() {
        text(empty.to_string())
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint)
    } else {
        let mut list = col().gap(crate::ds::SPACE_2);
        for name in names {
            list = list.child(
                row()
                    .gap(crate::ds::SPACE_4)
                    .items_center()
                    .child(
                        el().size(20.0, 20.0)
                            .radius(10.0)
                            .center()
                            .fill(k.c.surface_hover)
                            .child(
                                text(monogram(name))
                                    .font(k.ty.text_xsmall)
                                    .color(k.c.text_muted),
                            ),
                    )
                    .child(text(name.clone()).font(k.ty.small).color(k.c.text_base)),
            );
        }
        list
    };
    section(k, title, body)
}

fn labels(k: &Kit, pr: &PullRequest) -> El {
    if pr.labels.is_empty() {
        return text("None".to_string())
            .font(k.ty.small)
            .color(k.c.text_faint);
    }
    let mut list = col().gap(crate::ds::SPACE_2);
    for label in &pr.labels {
        let mut line = row().gap(crate::ds::SPACE_4).items_center();
        if let Some(color) = label_color(&label.color) {
            line = line.child(el().size(7.0, 7.0).radius(3.5).fill(color));
        }
        list = list.child(
            line.child(
                text(label.name.clone())
                    .font(k.ty.small)
                    .color(k.c.text_base),
            ),
        );
    }
    list
}

fn verdict(k: &Kit, decision: Option<ReviewDecision>) -> El {
    decision_label(decision).map_or_else(
        || {
            text("No verdict yet.".to_string())
                .font(k.ty.small)
                .color(k.c.text_faint)
        },
        |(label, tone)| chip(k, label, tone),
    )
}

fn facts(k: &Kit, pr: &PullRequest) -> El {
    let roles = crate::theme::roles(k);
    let now = Timestamp::now();
    row()
        .key("pr-detail-facts")
        .w_full()
        .gap(14.0)
        .items_center()
        .flex_wrap()
        .child(
            row()
                .gap(crate::ds::SPACE_3)
                .items_center()
                .child(
                    text(pr.head_ref.clone())
                        .font(k.ty.mono)
                        .color(k.c.text_base),
                )
                .child(text("→").font(k.ty.small).color(k.c.text_faint))
                .child(
                    text(pr.base_ref.clone())
                        .font(k.ty.mono)
                        .color(k.c.text_faint),
                ),
        )
        .child(
            text(file_summary(pr.changed_files))
                .font(k.ty.text_xsmall)
                .color(k.c.text_muted),
        )
        .child(
            row()
                .gap(crate::ds::SPACE_3)
                .child(
                    text(format!("+{}", pr.additions))
                        .font(k.ty.text_xsmall)
                        .color(roles.git.added),
                )
                .child(
                    text(format!("−{}", pr.deletions))
                        .font(k.ty.text_xsmall)
                        .color(roles.git.deleted),
                ),
        )
        .child(
            text(format!(
                "{} · updated {} ago",
                pr.author,
                relative_age(pr.updated_at, now)
            ))
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint)
            .wrap(),
        )
}

fn details(k: &Kit, pr: &PullRequest) -> El {
    let mut grid = col().key("pr-detail-details").gap(14.0).w_full();
    grid = grid.child(section(
        k,
        "Verdict",
        verdict(k, pr.review_decision.clone()),
    ));
    grid = grid.child(people(
        k,
        "Reviewers",
        &pr.review_requests,
        "No reviewers requested.",
    ));
    grid = grid.child(people(k, "Assignees", &pr.assignees, "Nobody assigned."));
    grid = grid.child(section(k, "Labels", labels(k, pr)));
    grid.child(section(
        k,
        "Activity",
        col()
            .gap(crate::ds::SPACE_2)
            .child(
                text(comment_summary(pr.comment_count))
                    .font(k.ty.small)
                    .color(k.c.text_base),
            )
            .child(
                text(format!("Opened {}", readable_date(pr.created_at)))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
            ),
    ))
}

fn review_button(k: &Kit, pr: &PullRequest, variant: ButtonVariant, key: &'static str) -> El {
    let repository = pr.repository.clone();
    let number = pr.number;
    Button::new("Review with an agent")
        .variant(variant)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut AppState| s.open_review(&repository, number))
        .build(k)
        .key(key)
}

fn rail(k: &Kit, pr: &PullRequest) -> El {
    col()
        .key("pr-detail-rail")
        .w(280.0)
        .h_full()
        .min_h(0.0)
        .gap(14.0)
        .padding(14.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(section(
            k,
            "Verdict",
            verdict(k, pr.review_decision.clone()),
        ))
        .child(people(
            k,
            "Reviewers",
            &pr.review_requests,
            "None requested",
        ))
        .child(section(k, "Labels", labels(k, pr)))
        .child(review_button(
            k,
            pr,
            ButtonVariant::Outline,
            "pr-detail-review",
        ))
        .child(
            text(
                "Runs read-only in this checkout. It cannot edit, commit or post; you decide what \
                 reaches the host.",
            )
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint)
            .wrap(),
        )
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let Some(key) = s.workbench.pr_detail.as_deref() else {
        return empty(k, "Pull request not found in the last read.");
    };
    let Some(pr) = s.store.as_ref().and_then(|store| {
        store
            .pull_requests
            .pull_requests
            .iter()
            .find(|pr| crate::pr_filters::key(pr) == key)
    }) else {
        return empty(k, "Pull request not found in the last read.");
    };
    let (state, state_tone) = state_chip(pr);
    let mut trail = row()
        .key("pr-detail-trail")
        .w_full()
        .gap(crate::ds::SPACE_4)
        .items_center()
        .flex_wrap()
        .child(chip(k, state, state_tone))
        .child(
            text(format!("{} #{}", pr.repository, pr.number))
                .font(k.ty.mono)
                .color(k.c.text_muted),
        );
    for (label, tone) in relation_chips(&pr.relations) {
        trail = trail.child(chip(k, label, tone));
    }
    let url = pr.url.clone();
    let url_again = pr.url.clone();
    trail = trail
        .child(el().grow(1.0))
        .child(
            Button::new("Copy link")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press_tx(move |_s: &mut AppState, tx| tx.copy(url.clone()))
                .build(k)
                .key("pr-detail-copy"),
        )
        .child(
            Button::new("Open on the host")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press_tx(move |_s: &mut AppState, tx| {
                    let _ = tx.open_url(&url_again);
                })
                .build(k)
                .key("pr-detail-open"),
        );
    let mut tabs = row().key("pr-detail-tabs").gap(crate::ds::SPACE_2).w_full();
    for (tab, label, key) in [
        (Tab::Description, "Description", "pr-detail-tab-description"),
        (Tab::Details, "Details", "pr-detail-tab-details"),
    ] {
        let shown = s.workbench.pr_tab == tab;
        tabs = tabs.child(
            row()
                .key(key)
                .role("tab")
                .label(label.to_string())
                .checked(shown)
                .h(28.0)
                .px(10.0)
                .radius(k.radius.sm)
                .center()
                .fill(if shown {
                    k.c.surface_hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(k.c.hover)
                .on_press(move |s: &mut AppState| s.workbench.pr_tab = tab)
                .child(
                    text(label.to_string())
                        .font(if shown { k.ty.small_medium } else { k.ty.small })
                        .color(if shown { k.c.text_base } else { k.c.text_muted }),
                ),
        );
    }
    let body = if s.workbench.pr_tab == Tab::Details {
        details(k, pr)
    } else if pr.body.trim().is_empty() {
        col().gap(10.0).child(
            text("No description.".to_string())
                .font(k.ty.small)
                .color(k.c.text_faint),
        )
    } else {
        col()
            .key("pr-detail-description")
            .gap(10.0)
            .w_full()
            .child(
                Markdown::new(pr.body.clone())
                    .on_link_tx(|_s: &mut AppState, url, tx| {
                        let _ = tx.open_url(url);
                    })
                    .build(k),
            )
            .children(pr.body_truncated.then(|| {
                text(
                    "The host's description was longer than the read allows, so the end of it is \
                     not here. Open it on the host for the rest.",
                )
                .key("pr-detail-truncated")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .wrap()
            }))
    };
    let header = col()
        .key("pr-detail-header")
        .w_full()
        .gap(10.0)
        .padding(16.0)
        .child(trail)
        .child(
            text(pr.title.clone())
                .role("heading")
                .font(k.ty.body_medium)
                .color(k.c.text_strong),
        )
        .child(facts(k, pr))
        .child(
            row()
                .gap(crate::ds::SPACE_5)
                .items_center()
                .child(review_button(
                    k,
                    pr,
                    ButtonVariant::Contrast,
                    "pr-detail-review-main",
                ))
                .child(
                    text(format!(
                        "{} · opened {}",
                        comment_summary(pr.comment_count),
                        readable_date(pr.created_at)
                    ))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
                ),
        );
    row()
        .key("pr-detail")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .min_h(0.0)
                .child(header)
                .child(row().px(16.0).child(tabs))
                .child(
                    col()
                        .key("pr-detail-body")
                        .w_full()
                        .grow(1.0)
                        .min_h(0.0)
                        .padding(16.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(body),
                ),
        )
        .child(rail(k, pr))
}

fn empty(k: &Kit, message: &str) -> El {
    col()
        .key("pr-detail-empty")
        .w_full()
        .h_full()
        .min_h(0.0)
        .center()
        .child(
            crate::ds::Empty::new("pr-detail-missing", message)
                .why("Refresh pull requests to read it again.")
                .build(k),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(text: &str) -> Timestamp {
        Timestamp::parse_rfc3339(text).expect("a timestamp")
    }

    #[test]
    fn age_steps_from_minutes_to_hours_to_days() {
        let now = ts("2026-08-30T12:00:00Z");
        assert_eq!(relative_age(ts("2026-08-30T11:55:00Z"), now), "5m");
        assert_eq!(relative_age(ts("2026-08-30T09:00:00Z"), now), "3h");
        assert_eq!(relative_age(ts("2026-08-27T12:00:00Z"), now), "3d");
        assert_eq!(
            relative_age(ts("2026-08-31T12:00:00Z"), now),
            "0m",
            "clock skew reads as now, never as a negative age"
        );
    }

    #[test]
    fn summaries_count_singular_and_plural() {
        assert_eq!(file_summary(0), "No files changed");
        assert_eq!(file_summary(1), "1 file changed");
        assert_eq!(file_summary(12), "12 files changed");
        assert_eq!(comment_summary(0), "No comments");
        assert_eq!(comment_summary(1), "1 comment");
        assert_eq!(comment_summary(4), "4 comments");
    }

    #[test]
    fn only_a_real_six_digit_colour_is_painted() {
        assert!(label_color("7c3aed").is_some());
        assert!(label_color("#7C3AED").is_some());
        assert!(label_color("").is_none());
        assert!(label_color("not-a-colour").is_none());
        assert!(label_color("<script>").is_none());
    }

    #[test]
    fn relations_read_in_the_order_that_decides_what_to_do_next() {
        let chips = relation_chips(&PullRequestRelations {
            assigned: true,
            review_requested: true,
            authored: true,
        });
        assert_eq!(
            chips,
            [
                ("Your review requested", Tone::Warning),
                ("Assigned to you", Tone::Neutral),
                ("You opened this", Tone::Neutral)
            ]
        );
        assert!(
            relation_chips(&PullRequestRelations {
                assigned: false,
                review_requested: false,
                authored: false
            })
            .is_empty()
        );
    }

    #[test]
    fn a_monogram_is_one_uppercase_letter_or_a_question_mark() {
        assert_eq!(monogram("rin"), "R");
        assert_eq!(monogram("  ada "), "A");
        assert_eq!(monogram(""), "?");
    }

    #[test]
    fn a_calendar_date_is_the_rfc3339_prefix() {
        assert_eq!(readable_date(ts("2026-08-30T12:00:00Z")), "2026-08-30");
    }

    #[test]
    fn a_decision_with_no_value_has_no_chip() {
        assert_eq!(decision_label(None), None);
        assert_eq!(
            decision_label(Some(ReviewDecision::ReviewRequired)),
            Some(("Review required", Tone::Warning))
        );
    }

    #[test]
    fn a_repository_splits_at_its_last_slash() {
        assert_eq!(split_repository("owner/repo"), (Some("owner"), "repo"));
        assert_eq!(split_repository("repo"), (None, "repo"));
    }
}

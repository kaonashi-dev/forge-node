//! What the review chat's transcript shows, from the agent session's
//! conversation: the person's questions as they typed them, Forge's own
//! prompts as one-line notes, and the agent's answers.

use domain::{ConversationTurn, TurnRole};

use super::model::{CHAT_OPENING, FIRST_QUESTION, UPDATE_OPENING};

/// Where the read-only rules start in a first prompt; the question sits before them.
const RULES_HEADING: &str = "## Rules for this session";
/// Tool lines a reply lists before folding the rest into a count.
pub const SHOWN_TOOLS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatItem {
    /// Forge's opening prompt: the report the chat started with.
    Briefing,
    /// Forge told the chat that the review ran again.
    Update,
    Question(String),
    Answer {
        text: String,
        tools: Vec<String>,
    },
}

#[must_use]
pub fn items(turns: &[ConversationTurn]) -> Vec<ChatItem> {
    let mut items = Vec::with_capacity(turns.len() + 1);
    for turn in turns {
        match turn.role {
            TurnRole::Agent => items.push(ChatItem::Answer {
                text: turn.text.clone(),
                tools: turn.tools.clone(),
            }),
            TurnRole::User => {
                let text = turn.text.trim();
                if text.starts_with(CHAT_OPENING) {
                    items.push(ChatItem::Briefing);
                    if let Some(question) = first_question(text) {
                        items.push(ChatItem::Question(question.to_owned()));
                    }
                } else if text.starts_with(UPDATE_OPENING) {
                    items.push(ChatItem::Update);
                } else if !text.is_empty() {
                    items.push(ChatItem::Question(text.to_owned()));
                }
            }
        }
    }
    items
}

/// The question a first prompt carried, between its heading and the rules.
fn first_question(prompt: &str) -> Option<&str> {
    let start = prompt.find(FIRST_QUESTION)? + FIRST_QUESTION.len();
    let end = prompt[start..]
        .rfind(RULES_HEADING)
        .map_or(prompt.len(), |at| start + at);
    let question = prompt[start..end].trim();
    (!question.is_empty()).then_some(question)
}

/// The tools a reply shows, and how many more it ran.
#[must_use]
pub fn shown_tools(tools: &[String]) -> (&[String], usize) {
    let shown = tools.len().min(SHOWN_TOOLS);
    (&tools[..shown], tools.len() - shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: TurnRole, text: &str) -> ConversationTurn {
        ConversationTurn {
            role,
            text: text.into(),
            tools: Vec::new(),
        }
    }

    #[test]
    fn the_opening_report_becomes_a_note_and_its_question_a_bubble() {
        let opening = format!(
            "{CHAT_OPENING} of the changes in this checkout (head abc1234).\n\n## Findings\n\n\
             - Finding 1 [P1] a.rs:3 — A\n\n{FIRST_QUESTION}Is the P1 a blocker?\n\n\
             {RULES_HEADING}\n\nThis is a read-only review."
        );
        let shown = items(&[
            turn(TurnRole::User, &opening),
            turn(TurnRole::Agent, "Yes."),
        ]);
        assert_eq!(
            shown,
            [
                ChatItem::Briefing,
                ChatItem::Question("Is the P1 a blocker?".into()),
                ChatItem::Answer {
                    text: "Yes.".into(),
                    tools: Vec::new()
                },
            ]
        );
    }

    #[test]
    fn an_opening_without_a_question_and_an_update_are_notes() {
        let opening = format!("{CHAT_OPENING} of pull request #4.\n\n{RULES_HEADING}\n\nRules.");
        let update = format!("{UPDATE_OPENING}def5678. Verdict: Ready.");
        let shown = items(&[
            turn(TurnRole::User, &opening),
            turn(TurnRole::User, &update),
            turn(TurnRole::User, "And now?"),
        ]);
        assert_eq!(
            shown,
            [
                ChatItem::Briefing,
                ChatItem::Update,
                ChatItem::Question("And now?".into())
            ]
        );
    }

    #[test]
    fn a_question_naming_the_rules_heading_keeps_its_own_words() {
        let opening = format!(
            "{CHAT_OPENING} of x.\n\n{FIRST_QUESTION}Why does {RULES_HEADING} matter?\n\n\
             {RULES_HEADING}\n\nRules."
        );
        assert_eq!(
            first_question(&opening),
            Some(format!("Why does {RULES_HEADING} matter?").as_str())
        );
    }

    #[test]
    fn tools_past_the_first_few_fold_into_a_count() {
        let tools: Vec<String> = (0..5).map(|at| format!("Read {at}.rs")).collect();
        let (shown, more) = shown_tools(&tools);
        assert_eq!(shown.len(), SHOWN_TOOLS);
        assert_eq!(more, 2);
        assert_eq!(shown_tools(&tools[..1]), (&tools[..1], 0));
    }
}

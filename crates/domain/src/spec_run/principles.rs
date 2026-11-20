//! Fixed prompt texts every spec-run stage shares, composed verbatim so one test can hold them equal.

pub const POWER_OF_THE_SIMPLE: &str = "\
## The power of the simple

It means three steps, in this order:
1. Understand the problem. State what is wrong or missing, where it lives (file:line)
   and why the code behaves that way today. Do not change code you cannot explain.
2. Choose the attack. Pick the approach that fixes the cause, not the symptom, with
   the fewest moving parts. Name one alternative you rejected and why.
3. Make only the changes that approach needs. Touch what the criteria require;
   anything else you notice goes under \"Noticed, not touched\" in your report.

A bigger change (a refactor, a new module, a rewrite) is allowed only when the
smaller one cannot meet the criteria. Then it is a decision: say why, what it costs,
and ask before doing it. Simple is not a hack around a design that has to change.
No abstraction, option, layer or file the spec does not need; comments only where
the code cannot say why.
";

pub const PRINCIPLES: &str = "\
## Rules for every claim and question

- Evidence before a claim: no claim without the command, its exit code and an excerpt of its output. What you could not run is *not verified*, never *met*.
- Stopping is not succeeding: idle, exit or a timeout means the agent stopped, not that the work succeeded. Only a structured report settles a step.
- Artifacts, not summaries: review the diff against the criteria. Never review the coder's summary of the diff.
- Lukewarm is not approval: \"looks reasonable\" or \"whatever you think\" is not approval. Ask again with the decision stated.
- Ask with a recommendation: each question carries its context, 2–4 options, your recommendation first and what goes wrong if we choose badly. A question with a sensible default becomes a recorded assumption instead.
";

pub const QUESTIONS: &str = "\
Ask with your question tool (AskUserQuestion for Claude) in batches of at most 4, highest impact \
first: scope, then security, then experience, then technical. Write each answer into the section \
of the spec it affects, replacing the text it contradicts.";

pub const READINESS: &str = "\
Before asking for approval, rate each category Clear, Partial or Missing in a table: scope, data, \
user flow, performance and security, integrations, edge cases, terminology, done signal. Do not \
ask for approval while one is Missing.";

pub const ARTIFACT_RULES: &str = "\
## Publishing as a Claude artifact

Only when the run asks for it. Publish with your own artifact tool (Forge has no claude.ai login) \
and pass its URL as artifact_url; without such a tool, say so and keep only the file. The file is \
the source of truth and the artifact a view of the same content:
- the conclusion first, in two or three sentences;
- a diagram only where the shape matters: for a spec the current and proposed flow side by side \
and a map of the files touched; for a report the changes per crate and file with lines added and \
removed, and the criteria table coloured by state;
- tables for criteria, decisions and checks; short sentences with file:line;
- nothing invented: no number, path or box that is not in the file or the diff;
- one URL: when the content changes, republish at the same URL.
";

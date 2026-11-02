//! The planner brief of a spec run. Forge never fetches the spec: an artifact needs the reader's login.

use std::path::{Path, PathBuf};

use crate::{AgentDescriptor, EffortSpelling};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecSource {
    ClaudeArtifact(String),
    Link(String),
    File(PathBuf),
    Inline(String),
}

impl SpecSource {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            SpecSource::ClaudeArtifact(_) => "claude_artifact",
            SpecSource::Link(_) => "link",
            SpecSource::File(_) => "file",
            SpecSource::Inline(_) => "inline",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("the spec is empty")]
    Empty,
    #[error("spec file not found: {0}")]
    MissingFile(String),
}

const ARTIFACT_HOSTS: [&str; 2] = ["claude.ai", "claude.site"];
const SPEC_EXTENSIONS: [&str; 4] = ["md", "markdown", "mdx", "txt"];

/// A path-like value that is not a file is an error, never a one-line inline spec.
pub fn classify(raw: &str, is_file: impl Fn(&Path) -> bool) -> Result<SpecSource, SpecError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SpecError::Empty);
    }
    if let Some(host) = link_host(trimmed) {
        let artifact = ARTIFACT_HOSTS
            .iter()
            .any(|known| host == *known || host.ends_with(&format!(".{known}")));
        return Ok(if artifact {
            SpecSource::ClaudeArtifact(trimmed.to_owned())
        } else {
            SpecSource::Link(trimmed.to_owned())
        });
    }
    let path = Path::new(trimmed);
    if is_file(path) {
        return Ok(SpecSource::File(path.to_path_buf()));
    }
    if looks_like_path(trimmed) {
        return Err(SpecError::MissingFile(trimmed.to_owned()));
    }
    Ok(SpecSource::Inline(trimmed.to_owned()))
}

fn link_host(raw: &str) -> Option<String> {
    if raw.chars().any(char::is_whitespace) {
        return None;
    }
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn looks_like_path(raw: &str) -> bool {
    if raw.contains('\n') {
        return false;
    }
    let extension = Path::new(raw)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    ["/", "./", "../", "~/"].iter().any(|p| raw.starts_with(p))
        || extension.is_some_and(|e| SPEC_EXTENSIONS.contains(&e.as_str()))
}

/// Leaves room for the brief inside the 64 KiB launch prompt.
pub const MAX_INLINE_SPEC_BYTES: usize = 56 * 1024;
/// The planner's effort when its provider offers it.
pub const PLANNER_EFFORT: &str = "high";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// A Claude coder at medium effort.
    #[default]
    Claude,
    /// A Cursor coder, unless a coder provider is named.
    Mix,
}

impl Preset {
    #[must_use]
    pub fn coder(self) -> (&'static str, Option<&'static str>) {
        match self {
            Preset::Claude => ("claude", Some("medium")),
            Preset::Mix => ("cursor", None),
        }
    }
}

#[must_use]
pub fn planner_effort(descriptor: &AgentDescriptor) -> Option<String> {
    descriptor
        .effort
        .as_ref()
        .filter(|spec| spec.allowed.iter().any(|level| level == PLANNER_EFFORT))
        .map(|_| PLANNER_EFFORT.to_owned())
}

/// Why this provider cannot take this role's launch, checked before a planner starts rather than
/// by the daemon halfway through a run.
#[must_use]
pub fn agent_refusal(
    descriptor: &AgentDescriptor,
    effort: Option<&str>,
    model: Option<&str>,
) -> Option<String> {
    if !descriptor.capabilities.supports_initial_prompt {
        return Some("cannot take a prompt at launch".to_owned());
    }
    if model.is_some() && descriptor.model.is_none() {
        return Some("does not support model selection".to_owned());
    }
    let effort = effort?;
    match &descriptor.effort {
        None => Some("does not support effort selection".to_owned()),
        Some(spec) if !spec.allowed.iter().any(|level| level == effort) => Some(format!(
            "has no effort {effort}; it offers {}",
            spec.allowed.join(", ")
        )),
        Some(spec) if matches!(spec.spelling, EffortSpelling::ModelSuffix) && model.is_none() => {
            Some("needs a model to apply an effort".to_owned())
        }
        Some(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coder {
    pub provider: String,
    pub effort: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecRun {
    pub source: SpecSource,
    pub coder: Coder,
    /// Absolute: a session's `PATH` need not contain `forgectl`.
    pub forgectl: PathBuf,
    pub name: String,
    pub worktree: Option<String>,
    pub base: Option<String>,
    pub rounds: u32,
    pub planner_is_session: bool,
}

fn quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-:=,@+".contains(&b))
    {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn source_section(source: &SpecSource) -> String {
    match source {
        SpecSource::ClaudeArtifact(url) => format!(
            "The spec is a Claude artifact: {url}\n\nRead it with your Claude artifact tool \
             (for example `read_artifact`). A plain web fetch cannot use your claude.ai login. \
             If no such tool is available, stop and ask the person to paste the spec or save \
             it to a file."
        ),
        SpecSource::Link(url) => format!(
            "The spec is at {url}\n\nRead it with your web tools. If it cannot be read, stop \
             and ask the person."
        ),
        SpecSource::File(path) => {
            format!("The spec is the file {}\n\nRead all of it.", path.display())
        }
        SpecSource::Inline(text) => {
            format!("The spec follows, between the markers.\n\n<<<SPEC\n{text}\nSPEC>>>")
        }
    }
}

#[must_use]
pub fn compose_planner_prompt(run: &SpecRun) -> String {
    let ctl = quote(&run.forgectl.to_string_lossy());
    let mut spawn = format!(
        "{ctl} --json session spawn --provider {} --role executor",
        quote(&run.coder.provider)
    );
    if let Some(effort) = &run.coder.effort {
        spawn.push_str(&format!(" --effort {}", quote(effort)));
    }
    if let Some(model) = &run.coder.model {
        spawn.push_str(&format!(" --model {}", quote(model)));
    }
    if run.planner_is_session {
        spawn.push_str(" --parent self");
    }
    spawn.push_str(&format!(
        " --name {}",
        quote(&format!("{} coder", run.name))
    ));
    if let Some(branch) = &run.worktree {
        spawn.push_str(&format!(" --worktree {}", quote(branch)));
        if let Some(base) = &run.base {
            spawn.push_str(&format!(" --base {}", quote(base)));
        }
    }
    spawn.push_str(" --prompt-file <plan file>");
    let place = if run.worktree.is_some() {
        "The coder works in the new worktree the result reports as `workspace.path`; review it there."
    } else {
        "The coder works in your checkout: do not edit files while it runs."
    };
    let rounds = run.rounds;
    format!(
        "You are the planner of a spec run in Forge. You own the spec, the plan and the review; \
another agent session, the coder, writes the code. Do not write or edit the implementation yourself.

The power of the simple: the smallest concrete change that meets the spec wins. Take complexity out \
before adding any; no abstraction, option, layer or file the spec does not need, and comments only \
where the code cannot say why.

## The spec

{source}

## 1. Validate the definitions

Read the spec completely and list every acceptance criterion and every change it asks for. Give each one:
- context: why it exists and which code it touches (file:line);
- validation: the end state that proves it done and how to check it, preferably a test or command \
with its expected result; a manual check only when nothing can be automated.

Answer what the code can answer yourself. Collect the rest (items without context or validation, \
ambiguities, contradictions with the code) and ask the person about all of them at once with your \
question selector (for example AskUserQuestion): each question with its context, concrete options and \
your recommendation first. Launch nothing until every item has context and a validation.

## 2. Plan

Write a plan the coder can follow without this conversation, with no open question left in it:
- the goal and the current state (file:line);
- each criterion with its context and validation;
- what is out of scope;
- small ordered steps naming the files to change, each ending with the check that proves it.

Give the coder these rules in the plan:
- keep to the power of the simple;
- for each step, write the test that fails for the expected reason, then the smallest change that \
passes it; the test uses the code the way its user does; if no honest test is possible, say so and \
describe the manual check;
- when the plan and the code disagree, stop and report: expected, found, why it matters, how to proceed;
- when done, or when a decision is needed, report to the planner with \
`{ctl} --json session send parent --queue --file <report file>`: per criterion what changed and how it \
was proven (command and result);
- do not commit.

Keep the plan under 60 KiB and save it outside the checkout, for example in your temporary directory.

## 3. Launch the coder

{spawn}

Keep the returned `session_id` and use it, never the title. {place}

Send to the coder only with `--queue`: it is submitted as soon as the coder is idle, while `--now` types \
without pressing Enter and leaves the prompt sitting in its input. The coder's report reaches you the \
same way, as a prompt once you are idle; you can also follow it with \
`{ctl} --json session watch <coder id>`.

## 4. Wait

{ctl} --json session wait <coder id> --for idle,waiting,exit --timeout 15m

A timeout (exit 124) is not proof the coder is busy: some agents never report their activity. Read \
`{ctl} --json session read <coder id> --lines 80`; if the coder has stopped and awaits input, treat it \
as `idle`, otherwise wait again. `matched` says why a wait returned:
- `waiting`: read `{ctl} --json session read <coder id> --lines 80`. Answer what the spec or the plan \
answers with `{ctl} --json session send <coder id> --queue --text '...'`; take anything else to the person \
with your question selector. Never approve a permission, a command or a destructive action for it.
- `exit`: the coder ended. Tell the person; relaunch only if they ask.
- `idle`: review.

## 5. Review

Idle is not proof of completion, and the coder's report is not evidence until you repeat it. Run \
every criterion's validation yourself, and read `{ctl} --json session changes <coder id>` and the \
`git diff` from its `base_commit` (`{ctl} --json session show <coder id>`). Sort what you find into: \
done as planned, deviations, additions not in the plan, missing; write None for an empty group. Flag \
anything more complex than the spec needs. When the coder disputes a finding, check it rather than \
accept or repeat it.

## 6. Send the review

If anything must change, send numbered findings (file, problem, expected fix):

{ctl} --json session send <coder id> --queue --file <review file>

At most 8 KiB; for more, send a short prompt naming a file. Then wait again (step 4). If two rounds \
fail on the same criterion, state the premise they share in one sentence and take it to the person. \
After {rounds} review round(s), stop and hand what remains to the person.

## 7. Finish

Report each criterion as met or not with its evidence, the review rounds, and what is left open. Do \
not commit, push, merge or close the coder's session unless the person asks.
",
        source = source_section(&run.source),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &Path) -> bool {
        false
    }

    #[test]
    fn sources_are_links_files_or_text_and_a_missing_path_is_an_error() {
        assert_eq!(
            classify("https://claude.ai/public/artifacts/abc", none).unwrap(),
            SpecSource::ClaudeArtifact("https://claude.ai/public/artifacts/abc".into())
        );
        assert!(matches!(
            classify(" https://www.CLAUDE.site/x ", none).unwrap(),
            SpecSource::ClaudeArtifact(_)
        ));
        assert!(matches!(
            classify("https://claude.ai.evil.example/x", none).unwrap(),
            SpecSource::Link(_)
        ));
        assert!(matches!(
            classify("https://user@claude.ai:443/x", none).unwrap(),
            SpecSource::ClaudeArtifact(_)
        ));
        assert_eq!(
            classify("docs/spec.md", |p| p == Path::new("docs/spec.md")).unwrap(),
            SpecSource::File("docs/spec.md".into())
        );
        assert_eq!(
            classify("docs/spec.md", none),
            Err(SpecError::MissingFile("docs/spec.md".into()))
        );
        assert_eq!(
            classify("./plan", none),
            Err(SpecError::MissingFile("./plan".into()))
        );
        assert_eq!(
            classify("Add a cache to the loader.\nKeep the API.", none).unwrap(),
            SpecSource::Inline("Add a cache to the loader.\nKeep the API.".into())
        );
        assert_eq!(classify("  \n", none), Err(SpecError::Empty));
    }

    fn run(coder: Coder, worktree: Option<&str>) -> SpecRun {
        SpecRun {
            source: SpecSource::Inline("Build it".into()),
            coder,
            forgectl: "/Applications/Forge Node.app/Contents/MacOS/forgectl".into(),
            name: "cache's spec".into(),
            worktree: worktree.map(str::to_owned),
            base: worktree.map(|_| "main".to_owned()),
            rounds: 2,
            planner_is_session: true,
        }
    }

    #[test]
    fn the_brief_names_the_exact_coder_launch_and_the_review_loop() {
        let brief = compose_planner_prompt(&run(
            Coder {
                provider: "claude".into(),
                effort: Some("medium".into()),
                model: None,
            },
            None,
        ));
        assert!(brief.contains(
            "'/Applications/Forge Node.app/Contents/MacOS/forgectl' --json session spawn \
             --provider claude --role executor --effort medium --parent self \
             --name 'cache'\\''s spec coder' --prompt-file <plan file>"
        ));
        assert!(brief.contains("<<<SPEC\nBuild it\nSPEC>>>"));
        assert!(brief.contains("--for idle,waiting,exit"));
        assert!(brief.contains("session send parent --queue --file <report file>"));
        assert!(brief.contains("After 2 review round(s)"));
        assert!(brief.contains("AskUserQuestion"));
        assert!(brief.contains("Launch nothing until every item has context and a validation."));
        assert!(brief.contains("The power of the simple"));
        assert!(brief.contains("do not edit files while it runs"));

        let mixed = compose_planner_prompt(&run(
            Coder {
                provider: "cursor".into(),
                effort: Some("high".into()),
                model: Some("gpt-5".into()),
            },
            Some("spec/cache"),
        ));
        assert!(mixed.contains(
            "--provider cursor --role executor --effort high --model gpt-5 --parent self"
        ));
        assert!(mixed.contains("--worktree spec/cache --base main --prompt-file"));
        assert!(mixed.contains("workspace.path"));
    }

    #[test]
    fn an_artifact_is_read_with_the_artifact_tool_not_fetched() {
        let mut spec = run(
            Coder {
                provider: "opencode".into(),
                effort: None,
                model: None,
            },
            None,
        );
        spec.source = SpecSource::ClaudeArtifact("https://claude.ai/public/artifacts/abc".into());
        spec.planner_is_session = false;
        let brief = compose_planner_prompt(&spec);
        assert!(brief.contains("read_artifact"));
        assert!(!brief.contains("--parent self"));
        assert!(!brief.contains("--effort"));
    }
}

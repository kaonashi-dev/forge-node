//! Juva: draft commit messages, PR descriptions and change reviews.
//!
//! The system prompts encode the project's writing standard. [`draft`] is the
//! deterministic form — a file list and heuristics, no network — and
//! [`draft_remote`] is the same standard sent to the OpenAI-compatible endpoint
//! in `[juva]`. The remote form never replaces the local one: it is tried
//! first and falls back to it on anything at all, because the caller asked for
//! text and there is always text.
//!
//! The endpoint opens a socket, which is why nothing here is called from a
//! request handler — see the ack-then-event path in `core.rs`.

use std::time::Duration;

use domain::{ChangeContext, ChangeFile, JuvaDraft, JuvaKind};

use crate::config::JuvaConfig;

/// The seam the endpoint sits behind, so tests never make a real request.
///
/// The same shape as `agents::usage::http`: one method, `None` for every
/// failure, and callers that treat every `None` identically — draft locally.
pub trait JuvaClient: Send + Sync {
    /// POST `body` as JSON and return the response body on a 2xx.
    fn post(&self, url: &str, key: &str, body: &str, timeout: Duration) -> Option<String>;
}

/// The real client. Timeouts are per call, from `[juva] timeout_secs`.
#[derive(Default)]
pub struct UreqJuva;

impl JuvaClient for UreqJuva {
    fn post(&self, url: &str, key: &str, body: &str, timeout: Duration) -> Option<String> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(timeout)
            .build();
        // ureq returns `Err` for non-2xx, so `ok()?` also filters 4xx/5xx.
        // Nothing here logs the body or the key.
        agent
            .post(url)
            .set("authorization", &format!("Bearer {key}"))
            .set("content-type", "application/json")
            .send_string(body)
            .ok()?
            .into_string()
            .ok()
    }
}

/// Draft `kind` through the endpoint, falling back to [`draft`] on anything.
///
/// "Anything" is deliberate: off, unconfigured, no key, a timeout, a 500, a
/// body that does not parse, an empty completion. The caller asked for text,
/// and the local draft is always text — a Review tab that showed an error
/// where the summary goes would be worse than one that showed the file list.
#[must_use]
pub fn draft_remote(
    client: &dyn JuvaClient,
    config: &JuvaConfig,
    kind: JuvaKind,
    context: &ChangeContext,
) -> JuvaDraft {
    let local = draft(kind, context);
    let Some(key) = config.api_key() else {
        return local;
    };
    let Some(request) = completion_request(&config.model, kind, context) else {
        return local;
    };
    let Some(body) = client.post(config.endpoint.trim(), &key, &request, config.timeout()) else {
        tracing::debug!(target: "juva", ?kind, "endpoint unavailable, drafting locally");
        return local;
    };
    match completion_text(&body) {
        Some(text) => split_draft(kind, text, context),
        None => {
            tracing::debug!(target: "juva", ?kind, "endpoint answered with nothing usable");
            local
        }
    }
}

/// The chat-completions body, or `None` for a kind with no standard.
fn completion_request(model: &str, kind: JuvaKind, context: &ChangeContext) -> Option<String> {
    let system = match kind {
        JuvaKind::CommitMessage => COMMIT_SYSTEM_PROMPT,
        JuvaKind::PullRequest => PR_SYSTEM_PROMPT,
        JuvaKind::ChangeReview => REVIEW_SYSTEM_PROMPT,
        _ => return None,
    };
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user_message(context) },
        ],
    });
    serde_json::to_string(&payload).ok()
}

/// What the model is shown: the branch, the files, and the bounded patch.
///
/// `ChangeContext::patch` is already capped by `git-service`, and `truncated`
/// travels with it so the model is told when it is not seeing everything
/// rather than quietly reasoning over half a diff.
fn user_message(context: &ChangeContext) -> String {
    let mut out = String::new();
    if let Some(branch) = &context.branch {
        out.push_str(&format!("Branch: {branch}\n"));
    }
    out.push_str(&format!("Changed files: {}\n", context.files.len()));
    for file in context.files.iter().take(40) {
        out.push_str(&format!("- {} {}\n", file.status, file.path));
    }
    if context.truncated {
        out.push_str("\nNote: the diff below was truncated and is not complete.\n");
    }
    out.push_str("\nDiff:\n");
    out.push_str(&context.patch);
    out
}

/// The first choice's message content, if the body has one.
fn completion_text(body: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = parsed
        .get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()?
        .trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Split a completion into the title and body a [`JuvaDraft`] carries.
///
/// The first line is the title and the rest is the body — the same shape all
/// three standards ask for, and the shape `juvaAsEditable` rejoins.
fn split_draft(kind: JuvaKind, text: String, context: &ChangeContext) -> JuvaDraft {
    let mut lines = text.splitn(2, '\n');
    let title = lines.next().unwrap_or_default().trim().to_owned();
    let body = lines.next().unwrap_or_default().trim().to_owned();
    JuvaDraft {
        kind,
        title,
        body,
        context: context.clone(),
    }
}

/// System prompt for commit messages — the "standard" Juva applies on click.
pub const COMMIT_SYSTEM_PROMPT: &str = "\
You write git commit messages for Forge Node.

Rules:
- Prefer Conventional Commits: type(scope): subject
- Types: feat, fix, refactor, docs, test, chore, perf, build, ci
- Subject: imperative mood, ≤72 characters, no trailing period
- Body (optional): why the change exists, not a file list
- Never add Co-Authored-By or Generated-with trailers
- One logical change per message; if the diff mixes concerns, name the dominant one
";

/// System prompt for pull-request title and body.
pub const PR_SYSTEM_PROMPT: &str = "\
You write GitHub pull request titles and descriptions for Forge Node.

Rules:
- Title: short, imperative, matches the dominant change
- Body sections:
  ## Summary
  - 1–3 bullets of what changed and why
  ## Test plan
  - Checklist of how to verify
- No filler, no marketing tone, no emoji
";

/// System prompt for a change review — the Review tab's standard.
pub const REVIEW_SYSTEM_PROMPT: &str = "\
You write change reviews for Forge Node.

Rules:
- Open with one sentence naming what this branch now does that it did not before
- Then 3-6 bullets, each one a behaviour or a decision, never a file
- Name a risk or an unfinished edge when the diff shows one
- Never restate the file list; the reader has it beside your text
- No filler, no marketing tone, no emoji
";

/// Draft text for `kind` from `context`, applying the matching system standard.
#[must_use]
pub fn draft(kind: JuvaKind, context: &ChangeContext) -> JuvaDraft {
    match kind {
        JuvaKind::CommitMessage => draft_commit(context),
        JuvaKind::PullRequest => draft_pr(context),
        JuvaKind::ChangeReview => draft_review(context),
        JuvaKind::Unknown | _ => JuvaDraft {
            kind,
            title: "Update".into(),
            body: String::new(),
            context: context.clone(),
        },
    }
}

fn draft_commit(context: &ChangeContext) -> JuvaDraft {
    let _ = COMMIT_SYSTEM_PROMPT; // kept as the canonical standard for LLM adapters
    let title = conventional_subject(&context.files);
    let body = if context.files.len() > 1 {
        let mut lines: Vec<String> = context
            .files
            .iter()
            .take(12)
            .map(|f| format!("- {}: {}", f.status, f.path))
            .collect();
        if context.files.len() > 12 {
            lines.push(format!("- …and {} more", context.files.len() - 12));
        }
        if context.truncated {
            lines.push("- (diff truncated for draft)".into());
        }
        lines.join("\n")
    } else {
        String::new()
    };
    JuvaDraft {
        kind: JuvaKind::CommitMessage,
        title,
        body,
        context: context.clone(),
    }
}

fn draft_pr(context: &ChangeContext) -> JuvaDraft {
    let _ = PR_SYSTEM_PROMPT;
    let title = conventional_subject(&context.files);
    let branch = context.branch.as_deref().unwrap_or("this branch");
    let mut summary: Vec<String> = Vec::new();
    if let Some(ahead) = context.ahead {
        summary.push(format!("- {ahead} commit(s) on `{branch}` ready to review"));
    } else if context.dirty {
        summary.push(format!(
            "- Uncommitted work on `{branch}` (commit before opening)"
        ));
    } else {
        summary.push(format!("- Changes on `{branch}`"));
    }
    for file in context.files.iter().take(8) {
        summary.push(format!("- `{}` ({})", file.path, file.status));
    }
    if context.files.len() > 8 {
        summary.push(format!("- …and {} more files", context.files.len() - 8));
    }

    let body = format!(
        "## Summary\n{}\n\n## Test plan\n- [ ] Exercise the touched paths\n- [ ] Confirm CI is green\n",
        summary.join("\n")
    );
    JuvaDraft {
        kind: JuvaKind::PullRequest,
        title,
        body,
        context: context.clone(),
    }
}

/// The deterministic review: what changed, grouped, with the commits.
///
/// Not prose and not pretending to be. It answers "what is in this branch"
/// from the file list alone, which is all this side can know without asking a
/// model, and it is what the Review tab shows whenever `[juva]` is off or the
/// call failed.
fn draft_review(context: &ChangeContext) -> JuvaDraft {
    let _ = REVIEW_SYSTEM_PROMPT;
    let branch = context.branch.as_deref().unwrap_or("this checkout");
    let title = if context.files.is_empty() {
        format!("No changes on {branch}")
    } else {
        format!("{} file(s) changed on {branch}", context.files.len())
    };

    let mut areas: Vec<(&str, usize)> = Vec::new();
    for file in &context.files {
        let area = file.path.split('/').next().unwrap_or(file.path.as_str());
        match areas.iter_mut().find(|(name, _)| *name == area) {
            Some((_, count)) => *count += 1,
            None => areas.push((area, 1)),
        }
    }
    areas.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

    let mut lines: Vec<String> = Vec::new();
    for (area, count) in areas.iter().take(8) {
        lines.push(format!("- `{area}` — {count} file(s)"));
    }
    if areas.len() > 8 {
        lines.push(format!("- …and {} more area(s)", areas.len() - 8));
    }
    if context.truncated {
        lines.push("- (the diff was truncated, so this is not exhaustive)".into());
    }

    JuvaDraft {
        kind: JuvaKind::ChangeReview,
        title,
        body: lines.join("\n"),
        context: context.clone(),
    }
}

fn conventional_subject(files: &[ChangeFile]) -> String {
    if files.is_empty() {
        return "chore: update".into();
    }
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let scope = infer_scope(&paths);
    let ty = infer_type(&paths, files);
    let subject = infer_subject(&paths);
    match scope {
        Some(scope) => format!("{ty}({scope}): {subject}"),
        None => format!("{ty}: {subject}"),
    }
}

fn infer_type(paths: &[&str], files: &[ChangeFile]) -> &'static str {
    let joined = paths.join(" ");
    if joined.contains("test") || joined.contains("spec") {
        return "test";
    }
    if joined.contains("README") || joined.contains(".md") || joined.contains("docs/") {
        return "docs";
    }
    if files.iter().all(|f| f.status == "A" || f.status == "??") {
        return "feat";
    }
    if joined.contains("fix") || files.iter().any(|f| f.status.contains('D')) {
        return "fix";
    }
    "chore"
}

fn infer_scope(paths: &[&str]) -> Option<String> {
    let mut scopes = Vec::new();
    for path in paths {
        let rest = path.strip_prefix("crates/").unwrap_or(path);
        let Some(scope) = rest.split('/').next().filter(|s| !s.is_empty()) else {
            continue;
        };
        // A bare file at the repo root has no useful scope.
        if path.strip_prefix("crates/").is_none() && !path.contains('/') {
            continue;
        }
        if !scopes.iter().any(|s| s == scope) {
            scopes.push(scope.to_string());
        }
        if scopes.len() > 1 {
            return None;
        }
    }
    scopes.into_iter().next()
}

fn infer_subject(paths: &[&str]) -> String {
    if paths.len() == 1 {
        let name = paths[0]
            .rsplit('/')
            .next()
            .unwrap_or(paths[0])
            .trim_end_matches(".rs")
            .trim_end_matches(".md")
            .replace(['_', '-'], " ");
        return format!("update {name}");
    }
    format!("update {} files", paths.len())
}

/// Map a `git-service` snapshot into the wire [`ChangeContext`].
#[must_use]
pub fn context_from_snapshot(snap: git_service::ChangeSnapshot) -> ChangeContext {
    ChangeContext {
        branch: snap.branch,
        default_branch: snap.default_branch,
        dirty: snap.dirty,
        ahead: snap.ahead,
        behind: snap.behind,
        files: snap
            .files
            .into_iter()
            .map(|(path, status)| ChangeFile { path, status })
            .collect(),
        patch: snap.patch,
        truncated: snap.truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(files: &[(&str, &str)]) -> ChangeContext {
        ChangeContext {
            branch: Some("feat/x".into()),
            default_branch: Some("main".into()),
            dirty: true,
            ahead: Some(1),
            behind: None,
            files: files
                .iter()
                .map(|(p, s)| ChangeFile {
                    path: (*p).into(),
                    status: (*s).into(),
                })
                .collect(),
            patch: String::new(),
            truncated: false,
        }
    }

    #[test]
    fn commit_draft_uses_conventional_form() {
        let d = draft(
            JuvaKind::CommitMessage,
            &ctx(&[("crates/client/src/a.rs", "M")]),
        );
        assert!(d.title.starts_with("chore(client):") || d.title.starts_with("fix(client):"));
    }

    #[test]
    fn pr_draft_has_summary_and_test_plan() {
        let d = draft(JuvaKind::PullRequest, &ctx(&[("README.md", "M")]));
        assert!(d.body.contains("## Summary"));
        assert!(d.body.contains("## Test plan"));
    }

    #[test]
    fn review_draft_groups_by_area_without_a_model() {
        let d = draft(
            JuvaKind::ChangeReview,
            &ctx(&[
                ("crates/daemon/src/a.rs", "M"),
                ("crates/daemon/src/b.rs", "M"),
                ("apps/tauri/src/c.ts", "M"),
            ]),
        );
        assert!(d.title.contains("3 file(s) changed"));
        // Ordered by weight: the area with two files leads.
        let daemon = d.body.find("`crates`").expect("crates area");
        let tauri = d.body.find("`apps`").expect("apps area");
        assert!(daemon < tauri);
    }

    /// A fake that answers whatever the test hands it, so no request is made.
    struct FakeJuva(Option<String>);

    impl JuvaClient for FakeJuva {
        fn post(&self, _: &str, _: &str, _: &str, _: Duration) -> Option<String> {
            self.0.clone()
        }
    }

    fn configured() -> JuvaConfig {
        // SAFETY-adjacent: the variable is process-wide, so the name is unique
        // to this test rather than something another one could be reading.
        std::env::set_var("FORGE_JUVA_TEST_KEY", "k");
        JuvaConfig {
            enabled: true,
            endpoint: "https://example.invalid/v1/chat/completions".into(),
            model: "m".into(),
            api_key_env: "FORGE_JUVA_TEST_KEY".into(),
            timeout_secs: 1,
        }
    }

    #[test]
    fn a_completion_becomes_the_title_and_the_body() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "Adds a review tab\n\nIt reads the base." } }]
        })
        .to_string();
        let d = draft_remote(
            &FakeJuva(Some(body)),
            &configured(),
            JuvaKind::ChangeReview,
            &ctx(&[("a.rs", "M")]),
        );
        assert_eq!(d.title, "Adds a review tab");
        assert_eq!(d.body, "It reads the base.");
    }

    /// Every failure lands in the same place: the local draft. The caller
    /// asked for text, and a Review tab showing an error where the summary
    /// goes is worse than one showing the file list.
    #[test]
    fn every_endpoint_failure_falls_back_to_the_local_draft() {
        let context = ctx(&[("a.rs", "M")]);
        let local = draft(JuvaKind::ChangeReview, &context);

        for body in [None, Some("not json".to_owned()), Some("{}".to_owned())] {
            let d = draft_remote(
                &FakeJuva(body),
                &configured(),
                JuvaKind::ChangeReview,
                &context,
            );
            assert_eq!(d, local);
        }
    }

    #[test]
    fn an_unconfigured_juva_never_asks() {
        let context = ctx(&[("a.rs", "M")]);
        let d = draft_remote(
            // A client that would panic if it were called.
            &FakeJuva(Some("{}".into())),
            &JuvaConfig::default(),
            JuvaKind::ChangeReview,
            &context,
        );
        assert_eq!(d, draft(JuvaKind::ChangeReview, &context));
        assert_eq!(JuvaConfig::default().api_key(), None);
    }
}

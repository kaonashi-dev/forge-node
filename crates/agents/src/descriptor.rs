//! The provider adapter contract (ADR-007) and the launch-spec builder (§13.3).
//!
//! ADR-007 defines two levels. Level 1 is the [`AgentDescriptor`] (static
//! data, see [`crate::builtins`]). Level 2 is the [`AgentAdapter`] trait, for
//! providers that need special behavior (resume, initial prompts, session
//! detection). The MVP ships **no** real adapter: every built-in is served by
//! the generic [`DescriptorAdapter`], which wraps a descriptor with no special
//! behavior.

use std::path::{Path, PathBuf};

use domain::{
    AgentDescriptor, AgentProviderId, DetectionResult, LaunchAgentRequest, ResolvedEnvironment,
    SpawnSpec,
};

use crate::detection;

/// Errors from detecting or launching an agent provider.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// No candidate binary (and no usable override) was found for the provider.
    #[error("agent provider `{0}` is not installed: no candidate binary was found")]
    NotInstalled(AgentProviderId),
    /// The requested provider id is not registered.
    #[error("no agent provider is registered for `{0}`")]
    UnknownProvider(AgentProviderId),
    /// A resume was asked for by a provider that offers no way to resume.
    #[error("agent provider `{0}` cannot resume an earlier session")]
    ResumeUnsupported(AgentProviderId),
    /// A prompt was handed to a provider that only takes one typed into its
    /// own interface.
    #[error("agent provider `{0}` cannot be given a prompt at launch")]
    PromptUnsupported(AgentProviderId),
    /// A read-only launch was asked for by a provider that declares no such
    /// mode. Refused rather than downgraded: a review that can write is not a
    /// weaker review, it is a different thing (§16.9).
    #[error("agent provider `{0}` has no read-only mode")]
    ReviewUnsupported(AgentProviderId),
    /// A version probe could not be executed.
    #[error("version probe for `{candidate}` failed: {source}")]
    ProbeFailed {
        candidate: String,
        #[source]
        source: std::io::Error,
    },
    /// An underlying I/O error while preparing a launch.
    #[error("i/o error while preparing an agent launch: {0}")]
    Io(#[from] std::io::Error),
}

/// Behavioral contract for an agent provider (ADR-007).
///
/// The MVP implements this only via [`DescriptorAdapter`]. Real adapters would
/// override [`build_launch`](AgentAdapter::build_launch) (e.g. to add resume
/// flags) or [`detect`](AgentAdapter::detect) (e.g. to inspect a config file).
pub trait AgentAdapter: Send + Sync {
    /// The static descriptor backing this adapter.
    fn descriptor(&self) -> &AgentDescriptor;

    /// Detect whether the provider is installed in `env` (§13.1).
    fn detect(&self, env: &ResolvedEnvironment) -> DetectionResult;

    /// Build a fully-resolved [`SpawnSpec`] for a launch request (§13.3).
    ///
    /// # Errors
    /// Returns [`AgentError::NotInstalled`] when neither the request's override
    /// nor any candidate binary can be resolved to an executable.
    fn build_launch(
        &self,
        req: &LaunchAgentRequest,
        env: &ResolvedEnvironment,
    ) -> Result<SpawnSpec, AgentError>;

    /// Build a [`SpawnSpec`] pointed at a profile's config directory (§13.4).
    ///
    /// Defaulted, because a profile's directory is a property of the launch and
    /// not of the provider: an adapter only overrides this if its own
    /// `build_launch` does something the generic builder cannot express.
    ///
    /// # Errors
    /// As [`build_launch`](AgentAdapter::build_launch).
    fn build_launch_with_config_dir(
        &self,
        req: &LaunchAgentRequest,
        env: &ResolvedEnvironment,
        config_dir: Option<&Path>,
    ) -> Result<SpawnSpec, AgentError> {
        build_launch_with_config_dir(self.descriptor(), req, env, config_dir)
    }
}

/// The generic, data-only adapter used for every MVP built-in: it wraps an
/// [`AgentDescriptor`] and adds no special behavior (ADR-007, level 1).
pub struct DescriptorAdapter {
    descriptor: AgentDescriptor,
}

impl DescriptorAdapter {
    /// Wrap a descriptor.
    #[must_use]
    pub fn new(descriptor: AgentDescriptor) -> Self {
        Self { descriptor }
    }
}

impl AgentAdapter for DescriptorAdapter {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    fn detect(&self, env: &ResolvedEnvironment) -> DetectionResult {
        // No user override is known at the adapter level; the registry applies
        // per-provider overrides when it drives detection.
        detection::detect(&self.descriptor, env, None)
    }

    fn build_launch(
        &self,
        req: &LaunchAgentRequest,
        env: &ResolvedEnvironment,
    ) -> Result<SpawnSpec, AgentError> {
        build_launch(&self.descriptor, req, env)
    }
}

/// Build a fully-resolved [`SpawnSpec`] for a descriptor and request (§13.3).
///
/// - `program` is resolved to an absolute path: the request's
///   `executable_override` if present, else the first candidate found on
///   `env.path_entries`.
/// - `args` is `descriptor.default_args`, then the resume arguments when
///   `req.resume_session_id` is set (§13.5), then `req.extra_args`.
/// - `env` is the complete resolved environment plus the terminal hints
///   `TERM=xterm-256color` and `COLORTERM=truecolor`.
///
/// **The `PATH` fallback is unverified**: it finds a *file with the right name*,
/// not a binary the version probe accepted, and those differ exactly where it
/// matters — `agent` on `PATH` is often the Grok CLI, which [`detection::detect`]
/// rejects for the `cursor` descriptor (§7.5, §13.1 step 4). A caller that has a
/// detection result must therefore pass its
/// [`DetectionStatus::Installed`](domain::DetectionStatus::Installed)
/// executable as `req.executable_override`; the daemon always does (§13.3), so
/// the fallback only serves callers with no detection at all.
///
/// The per-session variables `FORGE_SESSION_ID` and `FORGE_WORKSPACE` (§13.3)
/// are deliberately **not** set here: this crate has no `SessionId`, and the
/// daemon injects them per session when it spawns the PTY, keeping this builder
/// pure and independent of session state.
///
/// # Errors
/// - [`AgentError::NotInstalled`] when no program can be resolved.
/// - [`AgentError::ResumeUnsupported`] when a resume is asked of a provider
///   that declares no [`domain::ResumeStyle`].
/// - [`AgentError::PromptUnsupported`] when a prompt is handed to a provider
///   that declares no [`domain::PromptStyle`].
pub fn build_launch(
    descriptor: &AgentDescriptor,
    req: &LaunchAgentRequest,
    env: &ResolvedEnvironment,
) -> Result<SpawnSpec, AgentError> {
    build_launch_with_config_dir(descriptor, req, env, None)
}

/// [`build_launch`] pointed at a profile's own config directory (§13.4).
///
/// `config_dir` must already be absolute — [`domain::AgentProfile::resolve_config_dir`]
/// is what makes it so. Every variable the descriptor's
/// [`domain::ConfigDirSpec`] names is set to it, after the terminal hints and
/// before the `FORGE_*` the daemon injects, so a directory is the whole of what
/// a profile may change about the environment. A provider that declares no such
/// spelling ignores the directory rather than inventing a variable for it.
///
/// # Errors
/// As [`build_launch`].
pub fn build_launch_with_config_dir(
    descriptor: &AgentDescriptor,
    req: &LaunchAgentRequest,
    env: &ResolvedEnvironment,
    config_dir: Option<&Path>,
) -> Result<SpawnSpec, AgentError> {
    let program = resolve_program(descriptor, req, env)?;

    let mut args = descriptor.default_args.clone();
    // Resume leads the arguments: `codex resume <id>` is a subcommand, and a
    // subcommand that came after a profile's flags would not parse. The two
    // that spell it as a flag do not care where it sits, so one order serves
    // both (§13.5).
    if let Some(session_id) = &req.resume_session_id {
        let style = descriptor
            .resume
            .as_ref()
            .ok_or_else(|| AgentError::ResumeUnsupported(descriptor.id.clone()))?;
        args.extend(style.args(session_id));
    }
    args.extend(req.extra_args.iter().cloned());
    // The read-only flags go *after* the profile's own, so they win: a profile
    // that names OpenCode's `build` agent must not quietly turn a review into
    // a session that can edit the checkout it was started in (§16.9).
    if req.read_only {
        let style = descriptor
            .review
            .as_ref()
            .ok_or_else(|| AgentError::ReviewUnsupported(descriptor.id.clone()))?;
        args.extend(style.args.iter().cloned());
    }
    // The prompt goes last, after every flag: the three providers that take it
    // positionally would read a following argument as part of the prompt, and
    // the one that names it (`opencode --prompt`) does not care.
    if let Some(prompt) = &req.initial_prompt {
        let style = descriptor
            .prompt
            .as_ref()
            .ok_or_else(|| AgentError::PromptUnsupported(descriptor.id.clone()))?;
        args.extend(style.args(prompt));
    }

    let mut vars = env.vars.clone();
    upsert(&mut vars, "TERM", "xterm-256color");
    upsert(&mut vars, "COLORTERM", "truecolor");
    apply_config_dir(descriptor, &mut vars, config_dir);

    Ok(SpawnSpec {
        program,
        args,
        cwd: req.cwd.clone(),
        env: vars,
    })
}

/// Set every variable `descriptor`'s [`domain::ConfigDirSpec`] names to `dir`.
///
/// A provider that declares no such spelling is left alone rather than given an
/// invented variable, and `None` is the default account.
fn apply_config_dir(
    descriptor: &AgentDescriptor,
    vars: &mut Vec<(String, String)>,
    config_dir: Option<&Path>,
) {
    let (Some(spec), Some(dir)) = (descriptor.config_dir.as_ref(), config_dir) else {
        return;
    };
    let dir = dir.to_string_lossy();
    for var in &spec.vars {
        upsert(vars, var, &dir);
    }
}

/// `env` as a profile's account sees it: the same environment with the
/// provider's config-directory variables pointed at `config_dir` (§13.4).
///
/// This is what makes a usage probe read the *profile's* credentials rather
/// than the default account's — the reading and the launch have to agree on
/// which login they are talking about, so both go through the descriptor's own
/// spelling instead of naming `CLAUDE_CONFIG_DIR` outside this crate.
#[must_use]
pub fn env_for_config_dir(
    descriptor: &AgentDescriptor,
    env: &ResolvedEnvironment,
    config_dir: Option<&Path>,
) -> ResolvedEnvironment {
    if config_dir.is_none() {
        return env.clone();
    }
    let mut env = env.clone();
    apply_config_dir(descriptor, &mut env.vars, config_dir);
    env
}

/// Create a profile's config directory before launching (§13.4).
///
/// It is the `mkdir -p` a hand-written shell wrapper ran before exporting
/// `CLAUDE_CONFIG_DIR`, and `dir` must already be absolute
/// ([`domain::AgentProfile::resolve_config_dir`]). Created with `0700`: it
/// holds the agent's credentials. A provider that declares no
/// [`domain::ConfigDirSpec`] would never be told about the directory, so
/// nothing is created for it.
///
/// # Errors
/// Returns the [`std::io::Error`] from creating the directory.
pub fn ensure_config_dir(descriptor: &AgentDescriptor, dir: &Path) -> std::io::Result<()> {
    if descriptor.config_dir.is_none() || dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Resolve the program to launch to an absolute path (§13.3).
///
/// The candidate walk runs no version probe — see the warning on
/// [`build_launch`] about who may rely on it.
fn resolve_program(
    descriptor: &AgentDescriptor,
    req: &LaunchAgentRequest,
    env: &ResolvedEnvironment,
) -> Result<PathBuf, AgentError> {
    if let Some(over) = &req.executable_override {
        return resolve_executable(over, env)
            .ok_or_else(|| AgentError::NotInstalled(descriptor.id.clone()));
    }
    descriptor
        .binary_candidates
        .iter()
        .find_map(|name| detection::find_executable(name, &env.path_entries))
        .and_then(|found| absolutize(&found))
        .ok_or_else(|| AgentError::NotInstalled(descriptor.id.clone()))
}

/// Resolve a profile's chosen executable to an absolute path (§13.4).
///
/// A bare name is looked up on the resolved login-shell PATH before anything
/// else: that is where the wrapper script standing in for a shell alias lives,
/// and canonicalizing it against the daemon's working directory — `/` under
/// launchd — would never find it. A shell alias itself is not a program and
/// cannot be launched here at all.
///
/// Public because the daemon resolves the same name twice: once when the
/// profile is saved and the version probe has to run against something, and
/// again when a session starts.
#[must_use]
pub fn resolve_executable(over: &Path, env: &ResolvedEnvironment) -> Option<PathBuf> {
    if over.is_absolute() {
        return Some(over.to_path_buf());
    }
    if over.components().count() == 1 {
        if let Some(found) = detection::find_executable(&over.to_string_lossy(), &env.path_entries)
        {
            return absolutize(&found);
        }
    }
    absolutize(over)
}

/// Make `path` absolute: keep it as-is when already absolute, otherwise resolve
/// it relative to the current directory (returning `None` if that fails).
fn absolutize(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        std::fs::canonicalize(path).ok()
    }
}

/// Insert or replace `key` in a complete environment vector.
fn upsert(vars: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(slot) = vars.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value.to_owned();
    } else {
        vars.push((key.to_owned(), value.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins;
    use crate::test_support::{echo_script, env_with_path, write_script};

    fn launch_req(cwd: PathBuf) -> LaunchAgentRequest {
        LaunchAgentRequest {
            provider_id: AgentProviderId::new("claude"),
            cwd,
            extra_args: Vec::new(),
            executable_override: None,
            resume_session_id: None,
            initial_prompt: None,
            read_only: false,
        }
    }

    #[test]
    fn resolves_absolute_program_and_sets_terminal_env() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", &echo_script("Claude Code 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.extra_args = vec!["--print".to_owned()];

        let spec = build_launch(&builtins::builtin("claude").unwrap(), &req, &env).unwrap();

        assert!(spec.program.is_absolute());
        assert_eq!(spec.program.file_name().unwrap(), "claude");
        // args = default_args ([]) + extra_args.
        assert_eq!(spec.args, vec!["--print".to_owned()]);
        assert_eq!(spec.cwd, dir.path());
        assert!(spec
            .env
            .iter()
            .any(|(k, v)| k == "TERM" && v == "xterm-256color"));
        assert!(spec
            .env
            .iter()
            .any(|(k, v)| k == "COLORTERM" && v == "truecolor"));
        // FORGE_* are injected by the daemon per session, never here.
        assert!(!spec.env.iter().any(|(k, _)| k.starts_with("FORGE_")));
    }

    #[test]
    fn terminal_hints_override_inherited_values() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", &echo_script("v"));
        let mut env = env_with_path(vec![dir.path().to_path_buf()]);
        env.vars.push(("TERM".to_owned(), "dumb".to_owned()));

        let req = launch_req(dir.path().to_path_buf());
        let spec = build_launch(&builtins::builtin("claude").unwrap(), &req, &env).unwrap();

        let terms: Vec<&String> = spec
            .env
            .iter()
            .filter(|(k, _)| k == "TERM")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(terms, ["xterm-256color"]);
    }

    #[test]
    fn honors_executable_override() {
        let dir = tempfile::tempdir().unwrap();
        let over = write_script(dir.path(), "claude-custom", &echo_script("x"));
        // Empty PATH: only the override can supply the program.
        let env = env_with_path(vec![]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.executable_override = Some(over.clone());

        let spec = build_launch(&builtins::builtin("claude").unwrap(), &req, &env).unwrap();
        assert_eq!(spec.program, over);
    }

    #[test]
    fn a_profile_directory_replaces_the_resolved_one() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", &echo_script("v"));
        let mut env = env_with_path(vec![dir.path().to_path_buf()]);
        env.vars
            .push(("CLAUDE_CONFIG_DIR".to_owned(), "/old".to_owned()));

        let req = launch_req(dir.path().to_path_buf());
        let spec = build_launch_with_config_dir(
            &builtins::builtin("claude").unwrap(),
            &req,
            &env,
            Some(Path::new("/home/me/.claude-personal")),
        )
        .unwrap();

        let dirs: Vec<&String> = spec
            .env
            .iter()
            .filter(|(k, _)| k == "CLAUDE_CONFIG_DIR")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(
            dirs,
            ["/home/me/.claude-personal"],
            "the profile replaces, never duplicates"
        );
    }

    /// OpenCode splits configuration from credentials, so one directory has to
    /// arrive as both of its variables or the account is only half switched.
    #[test]
    fn opencodes_two_variables_point_at_the_one_directory() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "opencode", &echo_script("1.18.21"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.provider_id = AgentProviderId::new("opencode");
        let spec = build_launch_with_config_dir(
            &builtins::builtin("opencode").unwrap(),
            &req,
            &env,
            Some(Path::new("/home/me/.opencode-personal")),
        )
        .unwrap();

        for var in ["OPENCODE_CONFIG_DIR", "XDG_DATA_HOME"] {
            assert!(
                spec.env
                    .iter()
                    .any(|(k, v)| k == var && v == "/home/me/.opencode-personal"),
                "{var}"
            );
        }
    }

    /// Cursor CLI declares no directory of its own, so a profile's directory is
    /// dropped rather than exported under a variable nobody reads.
    #[test]
    fn a_provider_without_a_config_directory_ignores_one() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "agent", &echo_script("cursor 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.provider_id = AgentProviderId::new("cursor");
        let before = build_launch(&builtins::builtin("cursor").unwrap(), &req, &env)
            .unwrap()
            .env;
        let after = build_launch_with_config_dir(
            &builtins::builtin("cursor").unwrap(),
            &req,
            &env,
            Some(&dir.path().join("cursor-personal")),
        )
        .unwrap()
        .env;

        assert_eq!(before, after);
        assert!(!dir.path().join("cursor-personal").exists());
    }

    /// The alias case: a profile names `claude-personal`, which is a wrapper
    /// script on the login shell's PATH and not a path anyone typed out.
    #[test]
    fn a_bare_executable_name_is_found_on_the_resolved_path() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = write_script(
            dir.path(),
            "claude-personal",
            &echo_script("Claude Code 1.0"),
        );
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.executable_override = Some(PathBuf::from("claude-personal"));

        let spec = build_launch(&builtins::builtin("claude").unwrap(), &req, &env).unwrap();
        assert_eq!(spec.program, wrapper);
    }

    #[test]
    fn a_profile_directory_is_created_before_the_launch() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude-personal");
        let claude = builtins::builtin("claude").unwrap();

        ensure_config_dir(&claude, &config).unwrap();
        assert!(config.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&config).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }

        // Idempotent: an existing directory is left exactly as it is.
        ensure_config_dir(&claude, &config).unwrap();

        // Nothing is created for a provider that could never be told about it.
        let unused = dir.path().join("cursor-personal");
        ensure_config_dir(&builtins::builtin("cursor").unwrap(), &unused).unwrap();
        assert!(!unused.exists());
    }

    /// The flags OpenCode profiles suggest reach the child as written, in the
    /// order the profile lists them.
    #[test]
    fn an_opencode_profiles_flags_reach_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "opencode", &echo_script("1.18.21"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.provider_id = AgentProviderId::new("opencode");
        req.extra_args = vec![
            "--model".to_owned(),
            "anthropic/claude-sonnet-4-5".to_owned(),
            "--agent".to_owned(),
            "plan".to_owned(),
        ];

        let spec = build_launch(&builtins::builtin("opencode").unwrap(), &req, &env).unwrap();

        assert_eq!(spec.program.file_name().unwrap(), "opencode");
        assert_eq!(
            spec.args,
            vec![
                "--model".to_owned(),
                "anthropic/claude-sonnet-4-5".to_owned(),
                "--agent".to_owned(),
                "plan".to_owned(),
            ]
        );
    }

    /// A resumed Claude launch is the command line a user would type by hand,
    /// with the profile's own flags still after it.
    #[test]
    fn a_resume_leads_the_arguments_a_profile_adds() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", &echo_script("Claude Code 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.resume_session_id = Some("39c2ae5c-4fe3".to_owned());
        req.extra_args = vec!["--model".to_owned(), "opus".to_owned()];

        let spec = build_launch(&builtins::builtin("claude").unwrap(), &req, &env).unwrap();

        assert_eq!(
            spec.args,
            [
                "--resume".to_owned(),
                "39c2ae5c-4fe3".to_owned(),
                "--model".to_owned(),
                "opus".to_owned(),
            ]
        );
    }

    /// Codex spells resume as a subcommand, so it has to come first — after a
    /// flag it would not parse at all.
    #[test]
    fn codex_resumes_with_a_leading_subcommand() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "codex", &echo_script("codex 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.provider_id = AgentProviderId::new("codex");
        req.resume_session_id = Some("abc".to_owned());
        req.extra_args = vec!["--model".to_owned(), "gpt-5.3-codex".to_owned()];

        let spec = build_launch(&builtins::builtin("codex").unwrap(), &req, &env).unwrap();

        assert_eq!(spec.args.first().unwrap(), "resume");
        assert_eq!(spec.args[1], "abc");
    }

    /// Asking a provider that cannot resume is refused rather than silently
    /// starting a fresh conversation the user did not ask for.
    #[test]
    fn resuming_a_provider_that_cannot_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "cursor-agent", &echo_script("cursor 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.provider_id = AgentProviderId::new("cursor");
        req.resume_session_id = Some("chat-1".to_owned());

        let err = build_launch(&builtins::builtin("cursor").unwrap(), &req, &env).unwrap_err();
        assert!(matches!(err, AgentError::ResumeUnsupported(_)));
    }

    /// The prompt is one argument and it is the last one: every provider that
    /// takes one takes it positionally, so a flag appended after it would be
    /// read as part of the prompt.
    #[test]
    fn an_initial_prompt_is_one_trailing_argument() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", &echo_script("Claude Code 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.extra_args = vec!["--model".to_owned(), "opus".to_owned()];
        req.initial_prompt = Some("open a PR --now\nand $(whoami)".to_owned());

        let spec = build_launch(&builtins::builtin("claude").unwrap(), &req, &env).unwrap();
        assert_eq!(
            spec.args,
            [
                "--model",
                "opus",
                // Verbatim, and whole: it reaches the child through `execve`,
                // so a prompt full of flags, newlines and `$(…)` is text.
                "open a PR --now\nand $(whoami)"
            ]
        );
    }

    /// opencode reads its positional argument as a *project directory*, so a
    /// prompt put there would silently launch it in a folder named after the
    /// prompt. `--prompt` is the spelling that does not.
    #[test]
    fn opencode_names_its_prompt_instead_of_passing_it_positionally() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "opencode", &echo_script("opencode 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.provider_id = AgentProviderId::new("opencode");
        req.initial_prompt = Some("do the thing".to_owned());

        let spec = build_launch(&builtins::builtin("opencode").unwrap(), &req, &env).unwrap();
        assert_eq!(spec.args, ["--prompt", "do the thing"]);
    }

    #[test]
    fn prompting_a_provider_that_cannot_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", &echo_script("Claude Code 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut descriptor = builtins::builtin("claude").unwrap();
        descriptor.prompt = None;

        let mut req = launch_req(dir.path().to_path_buf());
        req.initial_prompt = Some("do the thing".to_owned());

        let err = build_launch(&descriptor, &req, &env).unwrap_err();
        assert!(matches!(err, AgentError::PromptUnsupported(_)));
    }

    /// The review flags come after the profile's own so they win: a profile
    /// that named OpenCode's `build` agent must not turn a review into a
    /// session that can edit the checkout it started in (§16.9).
    #[test]
    fn a_read_only_launch_appends_the_providers_flags_after_the_profiles() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "opencode", &echo_script("opencode 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut req = launch_req(dir.path().to_path_buf());
        req.provider_id = AgentProviderId::new("opencode");
        req.extra_args = vec!["--agent".to_owned(), "build".to_owned()];
        req.read_only = true;
        req.initial_prompt = Some("review it".to_owned());

        let spec = build_launch(&builtins::builtin("opencode").unwrap(), &req, &env).unwrap();
        assert_eq!(
            spec.args,
            [
                "--agent",
                "build",
                "--agent",
                "plan",
                "--prompt",
                "review it"
            ]
        );
    }

    /// Refused, never downgraded: a review that can write is a different thing
    /// from the one that was asked for.
    #[test]
    fn a_read_only_launch_without_a_read_only_mode_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", &echo_script("Claude Code 1.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let mut descriptor = builtins::builtin("claude").unwrap();
        descriptor.review = None;

        let mut req = launch_req(dir.path().to_path_buf());
        req.read_only = true;

        let err = build_launch(&descriptor, &req, &env).unwrap_err();
        assert!(matches!(err, AgentError::ReviewUnsupported(_)));
    }

    #[test]
    fn errors_when_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_path(vec![dir.path().to_path_buf()]);
        let req = launch_req(dir.path().to_path_buf());
        let err = build_launch(&builtins::builtin("claude").unwrap(), &req, &env).unwrap_err();
        assert!(matches!(err, AgentError::NotInstalled(_)));
    }
}

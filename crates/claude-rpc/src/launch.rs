//! The `claude` command line and environment of a draft's process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::session::{DraftSession, Start};

/// Variables passed through from [`LaunchSpec::env`]. Everything else is
/// dropped: an app started from a Claude Code session inherits variables
/// such as `CLAUDECODE` and `CLAUDE_CODE_SESSION_ID` that change how the CLI
/// behaves.
const ENV_ALLOWLIST: &[&str] = &[
    "HOME", "PATH", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "TERM",
];

/// The oldest Claude Code whose protocol this crate was checked against.
const MIN_VERSION: [u32; 3] = [2, 1, 293];
/// `program --version` with the variables of `env` a `claude` process
/// gets. Its output goes to [`check_version`].
pub fn version_command(program: &Path, env: &[(OsString, OsString)]) -> Command {
    let mut command = clean_command(program, env);
    command.arg("--version");
    command
}

/// `program` with only the variables of `env` a `claude` process gets.
pub(crate) fn clean_command(program: &Path, env: &[(OsString, OsString)]) -> Command {
    let mut command = Command::new(program);
    command.env_clear().envs(allowed(env).map(|(k, v)| (k, v)));
    command
}

/// The version `program` printed for `--version` when this crate supports
/// it, else a message for the user.
pub fn check_version(program: &Path, output: &str) -> Result<String, String> {
    supported(output.trim()).map_err(|found| {
        format!(
            "Draftmux needs Claude Code {} or later; {} reports {found}. Update Claude Code, \
             then restart Draftmux.",
            MIN_VERSION.map(|n| n.to_string()).join("."),
            program.display(),
        )
    })
}

/// The version in `claude --version` output such as `2.1.293 (Claude
/// Code)` when it is supported, else what was found.
fn supported(output: &str) -> Result<String, String> {
    let version = output.split_whitespace().next().unwrap_or_default();
    let parts: Option<Vec<u32>> = version.split('.').map(|n| n.parse().ok()).collect();
    match parts {
        Some(parts) if parts.len() == 3 && parts[..] >= MIN_VERSION[..] => Ok(version.into()),
        _ if output.is_empty() => Err("nothing".into()),
        _ => Err(format!("\"{output}\"")),
    }
}

/// The `HOME` a `claude` process gets: the first one in `vars`, else this
/// process's.
pub(crate) fn home<'a>(mut vars: impl Iterator<Item = &'a (OsString, OsString)>) -> PathBuf {
    vars.find(|(key, _)| key == "HOME")
        .map(|(_, home)| PathBuf::from(home))
        .or_else(std::env::home_dir)
        .unwrap_or_default()
}

/// The variables of `env` a `claude` process gets.
fn allowed(env: &[(OsString, OsString)]) -> impl Iterator<Item = &(OsString, OsString)> {
    env.iter().filter(|(key, _)| {
        let name = key.to_string_lossy();
        ENV_ALLOWLIST.contains(&name.as_ref()) || name.starts_with("LC_")
    })
}

/// Everything needed to start a draft's `claude` process.
pub struct LaunchSpec {
    /// The `claude` executable itself. A shell alias can add flags such as
    /// `--dangerously-skip-permissions`, so resolve the binary's path.
    pub program: PathBuf,
    pub cwd: PathBuf,
    /// The environment to take allowlisted variables from, such as the
    /// login-shell environment. `LANG` and `LC_*` are kept with the basics.
    pub env: Vec<(OsString, OsString)>,
    /// Variables set as given. A [`Session::Draft`] sets or unsets
    /// `CLAUDE_CONFIG_DIR` from its own `config_dir`.
    pub extra_env: Vec<(OsString, OsString)>,
    /// The model of the first process. A successful [`Agent::set_model`]
    /// replaces it for later processes, since an explicit `--model`
    /// overrides the model a resumed session last used.
    ///
    /// [`Agent::set_model`]: crate::Agent::set_model
    pub model: Option<String>,
    /// The built-in tools the model can use; empty disables them all.
    pub tools: Vec<Tool>,
    /// The settings files to load; empty loads none, which also skips
    /// `CLAUDE.md`. Settings can enable executable hooks.
    pub setting_sources: Vec<SettingSource>,
    pub session: Option<Session>,
    /// Registered with `initialize`; each call arrives as
    /// [`AgentEvent::Hook`](crate::AgentEvent::Hook). Unlike permission
    /// prompts, these also run for tools the settings already allow.
    pub hooks: Vec<PreToolUseHook>,
    /// Ask for tool permission through
    /// [`AgentEvent::Permission`](crate::AgentEvent::Permission)
    /// (`--permission-prompt-tool stdio`).
    pub permission_prompt: bool,
    /// Text added to Claude Code's system prompt
    /// (`--append-system-prompt`).
    pub system_prompt: Option<String>,
}

/// The tools a draft's agent may get. Tools that run processes, such as
/// Bash and Task, are left out: a background process escapes the process
/// group, so stopping the agent couldn't end it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Read,
    Edit,
    Write,
    Glob,
    Grep,
}

impl Tool {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Tool::Read => "Read",
            Tool::Edit => "Edit",
            Tool::Write => "Write",
            Tool::Glob => "Glob",
            Tool::Grep => "Grep",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    User,
    Project,
    Local,
}

impl SettingSource {
    fn name(self) -> &'static str {
        match self {
            SettingSource::User => "user",
            SettingSource::Project => "project",
            SettingSource::Local => "local",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// Start a session with this id (`--session-id`, a UUID).
    New(String),
    /// Continue a session by id or transcript path (`--resume`). A missing
    /// session makes the process exit with "No conversation found".
    Resume(OsString),
    /// A draft's session, which each process starts, forks or resumes
    /// depending on what exists then.
    Draft(DraftSession),
}

/// A `PreToolUse` hook for the tools `matcher` matches, such as
/// `"Edit|Write"`, or for every tool call without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreToolUseHook {
    pub matcher: Option<String>,
    /// Seconds the CLI waits for the answer. A hook that times out blocks
    /// the tool call.
    pub timeout: Option<u32>,
}

impl LaunchSpec {
    /// `claude` in stream-json mode with an allowlisted environment,
    /// continuing the session as `start` says.
    pub(crate) fn command(
        &self,
        model: Option<&str>,
        tools: &[Tool],
        start: Option<&Start>,
    ) -> Command {
        let mut command = clean_command(&self.program, &self.env);
        command
            .args([
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--strict-mcp-config",
            ])
            .arg("--setting-sources")
            .arg(join(self.setting_sources.iter().map(|s| s.name())))
            .arg("--tools")
            .arg(join(tools.iter().map(|t| t.name())));
        if self.permission_prompt {
            command.args(["--permission-prompt-tool", "stdio"]);
        }
        if let Some(prompt) = &self.system_prompt {
            command.arg("--append-system-prompt").arg(prompt);
        }
        if let Some(model) = model {
            command.arg("--model").arg(model);
        }
        match start {
            Some(Start::New(id)) => {
                command.arg("--session-id").arg(id);
            }
            Some(Start::Fork { snapshot, id }) => {
                command
                    .arg("--resume")
                    .arg(snapshot)
                    .args(["--fork-session", "--session-id"])
                    .arg(id);
            }
            Some(Start::Resume(target)) => {
                command.arg("--resume").arg(target);
            }
            None => {}
        }
        command.current_dir(&self.cwd);
        command.envs(self.extra_env.iter().map(|(k, v)| (k, v)));
        // Claude must write where the agent looks for the transcript.
        if let Some(Session::Draft(draft)) = &self.session {
            match &draft.config_dir {
                Some(dir) => command.env("CLAUDE_CONFIG_DIR", dir),
                None => command.env_remove("CLAUDE_CONFIG_DIR"),
            };
        }
        command
            // Reports `session_state_changed`, which tells when a run is
            // over.
            .env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1");
        command
    }
}

fn join<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names.collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LaunchSpec {
        LaunchSpec {
            program: PathBuf::from("/opt/claude/bin/claude"),
            cwd: PathBuf::from("/drafts/one"),
            env: [
                ("HOME", "/home/u"),
                ("PATH", "/usr/bin"),
                ("LC_ALL", "C"),
                ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_SESSION_ID", "parent"),
                ("ANTHROPIC_API_KEY", "inherited"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
            extra_env: vec![("CLAUDE_CONFIG_DIR".into(), "/cfg".into())],
            model: Some("haiku".into()),
            tools: vec![Tool::Read, Tool::Edit],
            setting_sources: vec![],
            session: Some(Session::New("0b5c".into())),
            hooks: vec![],
            permission_prompt: true,
            system_prompt: None,
        }
    }

    #[test]
    fn versions_older_than_the_checked_one_are_refused() {
        assert_eq!(supported("2.1.293 (Claude Code)"), Ok("2.1.293".into()));
        assert_eq!(supported("2.2.0 (Claude Code)"), Ok("2.2.0".into()));
        assert_eq!(supported("3.0.1"), Ok("3.0.1".into()));
        assert_eq!(
            supported("2.1.292 (Claude Code)"),
            Err("\"2.1.292 (Claude Code)\"".into())
        );
        assert_eq!(supported("1.0"), Err("\"1.0\"".into()));
        assert_eq!(supported(""), Err("nothing".into()));
    }

    #[test]
    fn the_version_check_gets_only_the_allowlisted_variables() {
        let env = [
            ("PATH".into(), "/usr/bin".into()),
            ("ANTHROPIC_API_KEY".into(), "secret".into()),
        ];
        let command = version_command(Path::new("/opt/claude"), &env);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["--version"]);
        let vars: Vec<_> = command.get_envs().map(|(k, _)| k).collect();
        assert_eq!(vars, ["PATH"]);
        let program = Path::new("/opt/claude");
        assert_eq!(
            check_version(program, "2.1.300 (Claude Code)\n"),
            Ok("2.1.300".into())
        );
        let err = check_version(program, "2.0.1 (Claude Code)").unwrap_err();
        assert!(err.contains("needs Claude Code 2.1.293 or later"), "{err}");
    }

    #[test]
    fn command_line_and_environment() {
        let spec = spec();
        let command = spec.command(
            Some("sonnet"),
            &spec.tools,
            Some(&Start::New("0b5c".into())),
        );
        assert_eq!(command.get_program(), "/opt/claude/bin/claude");
        assert_eq!(command.get_current_dir().unwrap(), spec.cwd);
        let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--tools",
                "Read,Edit",
                "--permission-prompt-tool",
                "stdio",
                "--model",
                "sonnet",
                "--session-id",
                "0b5c",
            ]
        );
        let mut env: Vec<_> = command
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?, v?.to_str()?)))
            .collect();
        env.sort();
        assert_eq!(
            env,
            [
                ("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1"),
                ("CLAUDE_CONFIG_DIR", "/cfg"),
                ("HOME", "/home/u"),
                ("LC_ALL", "C"),
                ("PATH", "/usr/bin"),
            ]
        );
    }

    #[test]
    fn resume_without_tools_or_model() {
        let mut spec = spec();
        spec.setting_sources = vec![SettingSource::Project, SettingSource::Local];
        spec.permission_prompt = false;
        spec.system_prompt = Some("Be brief.".into());
        let command = spec.command(
            None,
            &[],
            Some(&Start::Resume("/drafts/one/s.jsonl".into())),
        );
        let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            &args[7..],
            [
                "--setting-sources",
                "project,local",
                "--tools",
                "",
                "--append-system-prompt",
                "Be brief.",
                "--resume",
                "/drafts/one/s.jsonl",
            ]
        );
    }
}

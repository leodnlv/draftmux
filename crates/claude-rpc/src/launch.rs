//! The `claude` command line and environment of a draft's process.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

/// Variables passed through from [`LaunchSpec::env`]. Everything else is
/// dropped: an app started from a Claude Code session inherits variables
/// such as `CLAUDECODE` and `CLAUDE_CODE_SESSION_ID` that change how the CLI
/// behaves.
const ENV_ALLOWLIST: &[&str] = &[
    "HOME", "PATH", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "TERM",
];

/// Everything needed to start a draft's `claude` process.
pub struct LaunchSpec {
    /// The `claude` executable itself. A shell alias can add flags such as
    /// `--dangerously-skip-permissions`, so resolve the binary's path.
    pub program: PathBuf,
    pub cwd: PathBuf,
    /// The environment to take allowlisted variables from, such as the
    /// login-shell environment. `LANG` and `LC_*` are kept with the basics.
    pub env: Vec<(OsString, OsString)>,
    /// Variables set as given, such as `CLAUDE_CONFIG_DIR`.
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
    fn name(self) -> &'static str {
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
}

/// A `PreToolUse` hook for the tools `matcher` matches, such as
/// `"Edit|Write"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreToolUseHook {
    pub matcher: String,
    /// Seconds the CLI waits for the answer. A hook that times out blocks
    /// the tool call.
    pub timeout: Option<u32>,
}

impl LaunchSpec {
    /// `claude` in stream-json mode with an allowlisted environment.
    pub(crate) fn command(&self, model: Option<&str>) -> Command {
        let mut command = Command::new(&self.program);
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
            .arg(join(self.tools.iter().map(|t| t.name())));
        if self.permission_prompt {
            command.args(["--permission-prompt-tool", "stdio"]);
        }
        if let Some(model) = model {
            command.arg("--model").arg(model);
        }
        match &self.session {
            Some(Session::New(id)) => {
                command.arg("--session-id").arg(id);
            }
            Some(Session::Resume(target)) => {
                command.arg("--resume").arg(target);
            }
            None => {}
        }
        command.current_dir(&self.cwd).env_clear();
        for (key, value) in &self.env {
            let name = key.to_string_lossy();
            if ENV_ALLOWLIST.contains(&name.as_ref()) || name.starts_with("LC_") {
                command.env(key, value);
            }
        }
        command
            .envs(self.extra_env.iter().map(|(k, v)| (k, v)))
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
        }
    }

    #[test]
    fn command_line_and_environment() {
        let spec = spec();
        let command = spec.command(Some("sonnet"));
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
        spec.tools.clear();
        spec.setting_sources = vec![SettingSource::Project, SettingSource::Local];
        spec.session = Some(Session::Resume("/drafts/one/s.jsonl".into()));
        spec.permission_prompt = false;
        let command = spec.command(None);
        let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            &args[7..],
            [
                "--setting-sources",
                "project,local",
                "--tools",
                "",
                "--resume",
                "/drafts/one/s.jsonl",
            ]
        );
    }
}

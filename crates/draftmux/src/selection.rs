//! Selection actions as one-off agent calls that leave the draft's own
//! conversation alone, on the agent the draft runs on.
//!
//! pi: each call forks the draft's session into a temporary session
//! directory: `pi --mode json -p <prompt> --fork <draft session> --session-dir
//! <temp> -e <ext>`, with the draft's profile and cwd, stdin closed and
//! `DRAFTMUX_MODE=selection`, so the extension blocks every tool call.
//!
//! Claude Code: each call continues a copy of the draft's conversation in the
//! temporary directory as a new session that isn't saved, with no tools
//! ([`draftmux_claude_rpc::Oneshot`]), and gets the prompt on stdin.
//!
//! The temporary directory is deleted once the agent has exited, so nothing
//! reaches the draft's own session or chat. The prompt carries the editor
//! text and the selection, and the agent answers with replacement options as
//! JSON.

use std::ffi::OsString;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use draftmux_claude_rpc::{DraftSession, Oneshot};
use draftmux_pi_rpc::ThinkingLevel;
use draftmux_storage::{Backend as DraftBackend, Meta, Target};
use serde::Deserialize;

use crate::process;

mod stream;

pub(crate) use stream::Agent;
use stream::Stream;

/// A selection call waits for a full model turn over the whole session.
const TIMEOUT: Duration = Duration::from_secs(120);

/// pi gets the prompt on the command line, and macOS limits arguments plus
/// environment to 1 MiB.
const MAX_PROMPT_BYTES: usize = 256 * 1024;

/// Selection-call settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionConfig {
    /// `None` keeps the model the agent restores from the forked session,
    /// which reuses the session's prompt cache. pi takes `provider/model`,
    /// Claude Code a model such as `haiku`.
    pub model: Option<String>,
    /// pi's, sent with `model` only; pi restores the session's otherwise.
    pub thinking: Option<ThinkingLevel>,
    /// How many options each action asks for.
    pub alternatives: usize,
    /// Off runs pi with `--no-context-files`, as the draft's own pi.
    pub context_files: bool,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            model: None,
            thinking: None,
            alternatives: draftmux_settings::DEFAULT_ALTERNATIVES,
            context_files: true,
        }
    }
}

impl SelectionConfig {
    /// For a draft, from the settings it kept at creation, asking for
    /// `alternatives` options. A Claude draft without a selection model of
    /// its own uses the model chosen for the draft.
    pub fn for_draft(meta: &Meta, alternatives: usize) -> Self {
        let settings = &meta.settings;
        if let DraftBackend::Claude(claude) = &meta.backend {
            return Self {
                model: claude.selection_model.clone().or(claude.model.clone()),
                alternatives,
                ..Self::default()
            };
        }
        let model = settings.selection_model.as_ref();
        Self {
            model: model.map(|m| format!("{}/{}", m.provider, m.model_id)),
            thinking: model.and_then(|m| ThinkingLevel::parse(&m.thinking)),
            alternatives,
            context_files: settings.context_files,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Alternatives,
    /// The prompt of one of the user's selection actions.
    Instruction(String),
}

impl Action {
    fn instruction(&self) -> &str {
        match self {
            Action::Alternatives => {
                "Suggest alternative phrasings that preserve the meaning and fit the surrounding text."
            }
            Action::Instruction(text) => text,
        }
    }
}

/// The agent a call runs on: the draft's, with what starting it takes.
pub enum Backend {
    Pi {
        /// A command for the pi executable.
        pi: Command,
        extension: PathBuf,
        /// The draft's pi session file (`PiSession::session_file`).
        session_file: PathBuf,
        agent_dir: PathBuf,
    },
    Claude {
        /// The `claude` executable; its version is checked before the call.
        program: PathBuf,
        /// The login shell's environment, which the call's is taken from.
        env: Vec<(OsString, OsString)>,
        session: DraftSession,
    },
}

/// What a call is about: the selection in the editor's current text.
#[derive(Debug, Clone)]
pub struct Request {
    pub cwd: PathBuf,
    pub target: Target,
    pub text: String,
    pub range: Range<usize>,
    pub action: Action,
}

impl Request {
    pub fn new(meta: &Meta, text: String, range: Range<usize>, action: Action) -> Self {
        Self {
            cwd: meta.cwd.clone(),
            target: meta.target,
            text,
            range,
            action,
        }
    }

    fn selected(&self) -> &str {
        &self.text[self.range.clone()]
    }
}

/// Runs the call on `backend` and returns up to `config.alternatives`
/// replacements for the selection, with the selection's own leading and
/// trailing whitespace kept. The temporary directory is created in
/// `temp_parent` and is gone when this returns. `preview` receives
/// provisional complete options (or an empty list to retract them); only
/// the returned options may be applied to the editor.
pub fn run(
    backend: Backend,
    request: &Request,
    config: &SelectionConfig,
    temp_parent: &Path,
    cancel: &AtomicBool,
    mut preview: impl FnMut(Vec<String>),
) -> Result<Vec<String>, String> {
    let prompt = prompt(request, config.alternatives)?;
    let temp = tempfile::Builder::new()
        .prefix("draftmux-selection-")
        .tempdir_in(temp_parent)
        .map_err(|err| format!("Couldn't create a temporary session directory: {err}"))?;
    let (agent, command) = match backend {
        Backend::Pi {
            pi,
            extension,
            session_file,
            agent_dir,
        } => {
            let command = command(
                pi,
                request,
                &session_file,
                &agent_dir,
                &extension,
                temp.path(),
                config,
            );
            (Agent::Pi, command)
        }
        Backend::Claude {
            program,
            env,
            session,
        } => {
            crate::chat::check_claude(&program, &env)?;
            let oneshot = Oneshot {
                program,
                cwd: request.cwd.clone(),
                env,
                session,
                model: config.model.clone(),
            };
            (Agent::Claude, oneshot.command(temp.path())?)
        }
    };
    let (leading, trailing) = surrounding_whitespace(request.selected());
    let with_whitespace = |options: Vec<String>| {
        options
            .into_iter()
            .map(|option| format!("{leading}{option}{trailing}"))
            .collect()
    };
    let mut stream = Stream::new(agent);
    // The directory lives until the child has exited, including cancellation.
    let output = process::run_streaming(
        agent.name(),
        command,
        Some(prompt.as_str()),
        TIMEOUT,
        Some(cancel),
        |chunk| {
            stream.feed(chunk, config.alternatives, &mut |options| {
                preview(with_whitespace(options))
            })
        },
    );
    drop(temp);
    let result = match output {
        Ok(()) => stream.finish(),
        Err(err) if cancel.load(Ordering::Relaxed) => Err(err),
        Err(err) => Err(stream.reported_error().unwrap_or(err)),
    }
    .map(with_whitespace);
    if result.is_err() {
        preview(Vec::new());
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn command(
    mut pi: Command,
    request: &Request,
    session_file: &Path,
    agent_dir: &Path,
    extension: &Path,
    session_dir: &Path,
    config: &SelectionConfig,
) -> Command {
    // The prompt goes to stdin, which pi reads as the prompt: Windows
    // limits a command line to 32K characters, and a `.cmd` shim's arguments
    // can't span lines.
    pi.args(["--mode", "json"])
        .arg("-p")
        .arg("--fork")
        .arg(session_file)
        .arg("--session-dir")
        .arg(session_dir)
        .arg("-e")
        .arg(extension);
    if !config.context_files {
        pi.arg("--no-context-files");
    }
    if let Some(model) = &config.model {
        pi.arg("--model").arg(model);
        if let Some(thinking) = config.thinking {
            pi.arg("--thinking").arg(thinking.as_str());
        }
    }
    pi.current_dir(&request.cwd)
        .env("PI_CODING_AGENT_DIR", agent_dir)
        .env("DRAFTMUX_MODE", "selection")
        // The captured environment can carry values from a pi session the
        // app was launched from; selection mode reads none of them.
        .env_remove("DRAFTMUX_DRAFT_PATH")
        .env_remove("DRAFTMUX_INSTRUCTIONS");
    pi
}

const START: &str = "<<<SELECTION>>>";
const END: &str = "<<<END SELECTION>>>";

/// What the draft is written in. `pi-extension/draftmux.ts` describes the
/// targets with the same text, and Claude drafts get it as their target
/// platform.
pub(crate) fn syntax(target: Target) -> &'static str {
    match target {
        Target::Github => {
            "GitHub-flavored Markdown, rendered as in a GitHub comment: every line break inside a paragraph shows, so keep each paragraph on one line rather than wrapping it"
        }
        Target::GithubFile => {
            "GitHub-flavored Markdown for a file in a repository, such as README.md, rendered as GitHub shows a repository file: a single line break inside a paragraph shows as a space, so wrap paragraphs and list items to the width the file already uses; frontmatter is part of the file"
        }
        Target::Slack => {
            "Markdown for a Slack message: every line break shows as written; **bold**, *italic*, ~~strike~~, `code`, [text](url) links; :shortcode: emoji and <@U…> mentions work; headings paste as a bold line and tables as a code block; keep list items to one line"
        }
        Target::Rich => {
            "Markdown, pasted as rich text into an email or document (headings, bold, italic, links, lists, quotes, code and tables carry over; no raw HTML, alerts or @mentions; no subject line)"
        }
        Target::Plain => {
            "plain text, such as an email body (no Markdown syntax such as **bold**, # headings, backticks, [text](url) links or tables; write URLs in full; no subject line)"
        }
    }
}

/// Starts with a fixed line, so pi never reads it as a flag (`-…`) or a
/// file reference (`@…`), and Claude Code never as a slash command.
fn prompt(request: &Request, count: usize) -> Result<String, String> {
    let selected = request.selected().trim();
    if selected.is_empty() {
        return Err("Select some text first".into());
    }
    let text = &request.text;
    let marked = format!(
        "{}{START}{}{END}{}",
        &text[..request.range.start],
        request.selected(),
        &text[request.range.end..]
    );
    let syntax = syntax(request.target);
    let prompt = format!(
        "[Draftmux selection request]\n\
         This is a one-off request about the draft, separate from editing the draft file. \
         Don't edit files or use tools; answer with JSON only.\n\
         \n\
         The current draft, written in {syntax}, with the selected passage between {START} and {END}:\n\
         \n\
         {marked}\n\
         \n\
         Selected passage:\n\
         \n\
         {selected}\n\
         \n\
         Task for the selected passage: {instruction}\n\
         \n\
         Reply with exactly this JSON shape and nothing else: {{\"alternatives\": [\"...\"]}}\n\
         - Give {count} distinct options, each a complete replacement for the selected passage only, \
         fitting the text around it.\n\
         - Write them in {syntax}, keeping the passage's formatting and line breaks where they still fit.\n\
         - No commentary, numbering or code fences.",
        instruction = request.action.instruction(),
    );
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err("The draft is too long for a selection action".into());
    }
    Ok(prompt)
}

#[derive(Deserialize)]
struct Reply {
    alternatives: Vec<String>,
}

/// The options in `agent`'s reply: the JSON object it contains, even when
/// the model wrapped it in a code fence or text. Empty and repeated options
/// are dropped.
fn parse(agent: &str, output: &str, count: usize) -> Result<Vec<String>, String> {
    let none = || format!("{agent}'s reply had no alternatives");
    let json = output
        .find('{')
        .zip(output.rfind('}'))
        .filter(|(start, end)| start < end)
        .map(|(start, end)| &output[start..=end]);
    let reply: Reply = json
        .and_then(|json| serde_json::from_str(json).ok())
        .ok_or_else(none)?;
    let mut options: Vec<String> = Vec::new();
    for option in reply.alternatives {
        let option = option.trim();
        if !option.is_empty() && !options.iter().any(|o| o == option) {
            options.push(option.to_string());
        }
    }
    options.truncate(count);
    if options.is_empty() {
        return Err(none());
    }
    Ok(options)
}

/// The whitespace before and after the selection's text, which options
/// replace without it.
fn surrounding_whitespace(selected: &str) -> (&str, &str) {
    let start = selected.len() - selected.trim_start().len();
    let end = selected.trim_end().len().max(start);
    (&selected[..start], &selected[end..])
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::AtomicBool;

    use draftmux_storage::{ClaudeSession, DraftSettings};

    use super::*;

    fn request(text: &str, selected: &str, action: Action) -> Request {
        let start = text.find(selected).unwrap();
        Request {
            cwd: "/repo".into(),
            target: Target::Github,
            text: text.to_string(),
            range: start..start + selected.len(),
            action,
        }
    }

    fn meta(backend: DraftBackend) -> Meta {
        Meta {
            target: Target::Github,
            syntax: None,
            status: draftmux_storage::Status::Active,
            title: String::new(),
            source: None,
            cwd: "/repo".into(),
            backend,
            note: None,
            first_draft_pending: false,
            settings: DraftSettings::default(),
            file: None,
            created_at: Default::default(),
            updated_at: Default::default(),
        }
    }

    #[cfg(unix)]
    fn pi(pi: Command) -> Backend {
        Backend::Pi {
            pi,
            extension: "/ext".into(),
            session_file: "/drafts/x/session/s.jsonl".into(),
            agent_dir: "/profile".into(),
        }
    }

    #[test]
    fn prompt_marks_the_selection_in_the_draft() {
        let request = request(
            "Hello there.\n\n-- a dash line\n",
            "-- a dash line\n",
            Action::Instruction("make it formal".into()),
        );
        let prompt = prompt(&request, 3).unwrap();
        assert!(prompt.starts_with("[Draftmux selection request]\n"));
        assert!(
            prompt.contains("Hello there.\n\n<<<SELECTION>>>-- a dash line\n<<<END SELECTION>>>\n")
        );
        assert!(prompt.contains("Selected passage:\n\n-- a dash line\n\nTask"));
        assert!(prompt.contains("Task for the selected passage: make it formal"));
        assert!(prompt.contains("Give 3 distinct options"));
        assert!(prompt.contains("GitHub-flavored Markdown"));
    }

    #[test]
    fn custom_actions_pass_the_prompt_unchanged() {
        let mut actions = draftmux_settings::default_selection_actions();
        actions.push(draftmux_settings::SelectionAction {
            name: "Custom".into(),
            prompt: "Keep the details.\n  Use a warmer tone.  ".into(),
        });
        for action in actions {
            let request = request(
                "Hi there",
                "there",
                Action::Instruction(action.prompt.clone()),
            );
            assert_eq!(request.action.instruction(), action.prompt);
            assert!(prompt(&request, 3).unwrap().contains(&format!(
                "Task for the selected passage: {}\n\n",
                action.prompt
            )));
        }
    }

    #[test]
    fn syntax_matches_the_draft_prompt() {
        let extension = include_str!("../../../pi-extension/draftmux.ts");
        for &target in Target::ALL {
            let literal = format!("\"{}\"", syntax(target));
            assert!(extension.contains(&literal), "{literal}");
        }
    }

    #[test]
    fn prompt_names_slack_syntax_and_refuses_blank_or_huge_selections() {
        let mut slack = request(
            "Ship it today",
            "today",
            Action::Instruction("Shorten it".into()),
        );
        slack.target = Target::Slack;
        assert!(
            prompt(&slack, 3)
                .unwrap()
                .contains("Markdown for a Slack message")
        );
        slack.target = Target::Plain;
        assert!(
            prompt(&slack, 3)
                .unwrap()
                .contains("plain text, such as an email")
        );

        let blank = request("a \n b", " \n ", Action::Alternatives);
        assert!(prompt(&blank, 3).is_err());

        let long = "word ".repeat(60_000);
        let huge = request(&long, "word", Action::Alternatives);
        assert_eq!(
            prompt(&huge, 3).unwrap_err(),
            "The draft is too long for a selection action"
        );
    }

    #[test]
    fn parse_reads_json_inside_fences_and_text() {
        let output = "Sure!\n```json\n{\"alternatives\": [\"one\", \"two\\nlines\", \" one \", \"\", \"four\"]}\n```\n";
        assert_eq!(
            parse("pi", output, 3).unwrap(),
            ["one", "two\nlines", "four"]
        );
        assert_eq!(parse("pi", output, 1).unwrap(), ["one"]);
    }

    #[test]
    fn parse_rejects_replies_without_options() {
        for output in [
            "No JSON here",
            "{\"alternatives\": []}",
            "{\"other\": 1}",
            "} {",
        ] {
            assert_eq!(
                parse("pi", output, 3).unwrap_err(),
                "pi's reply had no alternatives"
            );
        }
    }

    #[test]
    fn whitespace_around_the_selection_is_kept() {
        assert_eq!(surrounding_whitespace("  word \n"), ("  ", " \n"));
        assert_eq!(surrounding_whitespace("word"), ("", ""));
        assert_eq!(surrounding_whitespace("   "), ("   ", ""));
    }

    #[test]
    fn command_forks_into_the_temp_dir_with_selection_mode() {
        let mut config = SelectionConfig::default();
        let request = request("Hi there", "there", Action::Alternatives);
        let build = |config: &SelectionConfig| {
            let command = command(
                Command::new("pi"),
                &request,
                Path::new("/drafts/x/session/s.jsonl"),
                Path::new("/profile"),
                Path::new("/ext/draftmux.ts"),
                Path::new("/tmp/s"),
                config,
            );
            let args: Vec<String> = command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            (command, args)
        };

        let (command, args) = build(&config);
        assert_eq!(
            args,
            [
                "--mode",
                "json",
                "-p",
                "--fork",
                "/drafts/x/session/s.jsonl",
                "--session-dir",
                "/tmp/s",
                "-e",
                "/ext/draftmux.ts"
            ]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/repo")));
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&("PI_CODING_AGENT_DIR".as_ref(), Some("/profile".as_ref()))));
        assert!(envs.contains(&("DRAFTMUX_MODE".as_ref(), Some("selection".as_ref()))));
        assert!(envs.contains(&("DRAFTMUX_INSTRUCTIONS".as_ref(), None)));

        config.thinking = Some(ThinkingLevel::High);
        assert_eq!(build(&config).1.len(), 9, "thinking only with a model");
        config.model = Some("anthropic/claude-haiku-5-5".into());
        assert_eq!(
            build(&config).1[9..],
            [
                "--model",
                "anthropic/claude-haiku-5-5",
                "--thinking",
                "high"
            ]
        );

        config.context_files = false;
        assert_eq!(build(&config).1[9], "--no-context-files");
    }

    #[test]
    fn config_for_a_draft_comes_from_its_settings() {
        let mut meta = meta(DraftBackend::pi("/profile"));
        assert_eq!(
            SelectionConfig::for_draft(&meta, 4),
            SelectionConfig {
                alternatives: 4,
                ..SelectionConfig::default()
            }
        );
        meta.settings = DraftSettings {
            instructions: None,
            context_files: false,
            selection_model: Some(draftmux_storage::ModelChoice {
                provider: "anthropic".into(),
                model_id: "claude-haiku-5-5".into(),
                thinking: "xhigh".into(),
            }),
        };
        assert_eq!(
            SelectionConfig::for_draft(&meta, 2),
            SelectionConfig {
                model: Some("anthropic/claude-haiku-5-5".into()),
                thinking: Some(ThinkingLevel::Xhigh),
                alternatives: 2,
                context_files: false,
            }
        );
    }

    #[test]
    fn a_claude_draft_s_config_takes_its_own_models() {
        let mut claude = ClaudeSession {
            session_id: "0196f1d2-7c1e-7a40-9b8e-3d1c2b4a5f60".parse().unwrap(),
            transcript: None,
            config_dir: None,
            fork: None,
            model: None,
            effort: None,
            selection_model: None,
        };
        let config = |claude: &ClaudeSession| {
            SelectionConfig::for_draft(&meta(DraftBackend::Claude(claude.clone())), 3)
        };
        assert_eq!(config(&claude), SelectionConfig::default());
        claude.model = Some("opus".into());
        assert_eq!(config(&claude).model.as_deref(), Some("opus"));
        claude.selection_model = Some("haiku".into());
        assert_eq!(
            config(&claude),
            SelectionConfig {
                model: Some("haiku".into()),
                ..SelectionConfig::default()
            }
        );
    }

    /// A fake pi that records its arguments and checks the temp session dir
    /// exists while it runs.
    #[cfg(unix)]
    #[test]
    fn run_returns_options_and_removes_the_temp_dir() {
        let bin = tempfile::tempdir().unwrap();
        let script = bin.path().join("pi");
        fs::write(
            &script,
            r#"cat > "$0.stdin"
while [ "$1" ]; do [ "$1" = --session-dir ] && test -d "$2" && touch "$2/s.jsonl"; shift; done
echo '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"{\"alternatives\":[\"over here\","}}'
echo '{"type":"message_end","message":{"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"{\"alternatives\":[\"over here\",\"here\"]}"}]}}'
echo '{"type":"agent_settled","aborted":false}'"#,
        )
        .unwrap();
        let mut pi = Command::new("/bin/sh");
        pi.arg(&script);
        let parent = tempfile::tempdir().unwrap();
        let mut request = request("Hi there\n", "there\n", Action::Alternatives);
        request.cwd = parent.path().to_path_buf();

        let mut previews = Vec::new();
        let options = run(
            self::pi(pi),
            &request,
            &SelectionConfig::default(),
            parent.path(),
            &AtomicBool::new(false),
            |options| previews.push(options),
        )
        .unwrap();
        assert_eq!(options, ["over here\n", "here\n"]);
        let prompt = fs::read_to_string(bin.path().join("pi.stdin")).unwrap();
        assert!(
            prompt.contains("<<<SELECTION>>>there\n<<<END SELECTION>>>"),
            "{prompt}"
        );
        assert_eq!(previews[0], ["over here\n"]);
        assert_eq!(previews.last().unwrap(), &options);
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn failed_process_retracts_complete_previews() {
        let mut pi = Command::new("/bin/sh");
        pi.args(["-c", r#"echo '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"{\"alternatives\":[\"preview\","}}'; exit 1"#]);
        let parent = tempfile::tempdir().unwrap();
        let mut request = request("word", "word", Action::Alternatives);
        request.cwd = parent.path().to_owned();
        let mut previews = Vec::new();
        assert!(
            run(
                self::pi(pi),
                &request,
                &SelectionConfig::default(),
                parent.path(),
                &AtomicBool::new(false),
                |options| previews.push(options)
            )
            .is_err()
        );
        assert_eq!(previews, [vec!["preview".to_string()], Vec::new()]);
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn run_reports_pi_errors() {
        let mut pi = Command::new("/bin/sh");
        pi.args(["-c", "echo 'No API key found for anthropic' >&2; exit 1"]);
        let parent = tempfile::tempdir().unwrap();
        let mut request = request(
            "Hi there",
            "there",
            Action::Instruction("Shorten it".into()),
        );
        request.cwd = parent.path().to_path_buf();
        let err = run(
            self::pi(pi),
            &request,
            &SelectionConfig::default(),
            parent.path(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(err, "No API key found for anthropic");
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    }

    /// Needs pi on `PATH` with a working default model in `~/.pi/agent`:
    /// `cargo test -j 2 -p draftmux --bin draftmux real_pi_selection -- --ignored`.
    #[test]
    #[ignore]
    fn real_pi_selection_call_leaves_the_draft_session_alone() {
        let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".pi/agent"));
        let cwd = tempfile::tempdir().unwrap();
        let draft_session = tempfile::tempdir().unwrap();
        let status = Command::new("pi")
            .args(["-p", "Reply with OK and nothing else.", "--session-dir"])
            .arg(draft_session.path())
            .current_dir(cwd.path())
            .env("PI_CODING_AGENT_DIR", &agent_dir)
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "creating the source session");
        let session_file = fs::read_dir(draft_session.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let before = fs::read(&session_file).unwrap();

        let text = "Thanks for the detailed report! I'll take a look tomorrow.\n";
        let start = text.find("I'll").unwrap();
        let request = Request {
            cwd: cwd.path().to_path_buf(),
            target: Target::Github,
            text: text.to_string(),
            range: start..text.len(),
            // Tool calls are blocked; the model still answers in text.
            action: Action::Instruction(
                "First run `ls` with bash, then make it more casual.".into(),
            ),
        };
        let parent = tempfile::tempdir().unwrap();
        let extension = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../pi-extension/draftmux.ts"
        ));
        let started = std::time::Instant::now();
        let mut first_preview = None;
        let backend = Backend::Pi {
            pi: Command::new("pi"),
            extension: extension.to_path_buf(),
            session_file: session_file.clone(),
            agent_dir,
        };
        let options = run(
            backend,
            &request,
            &SelectionConfig::default(),
            parent.path(),
            &AtomicBool::new(false),
            |options| {
                if !options.is_empty() {
                    first_preview.get_or_insert(started.elapsed());
                }
                eprintln!("preview at {:?}: {options:?}", started.elapsed());
            },
        )
        .unwrap();
        eprintln!(
            "first preview: {first_preview:?}; finished at {:?}; options: {options:?}",
            started.elapsed()
        );
        assert!(first_preview.is_some());
        assert!(!options.is_empty() && options.len() <= 3);
        assert!(options.iter().all(|o| o.ends_with('\n')));

        assert_eq!(
            fs::read(&session_file).unwrap(),
            before,
            "draft session unchanged"
        );
        assert_eq!(fs::read_dir(draft_session.path()).unwrap().count(), 1);
        assert_eq!(
            fs::read_dir(parent.path()).unwrap().count(),
            0,
            "temp dir removed"
        );
    }

    /// A fake `claude` (`claude.sh`) that answers `--version`, records its
    /// arguments and stdin in `args` and `stdin`, checks the conversation's
    /// copy exists while it runs, and prints `frames`.
    #[cfg(unix)]
    fn fake_claude(dir: &Path, frames: &str, status: i32) -> PathBuf {
        let script = dir.join("claude.sh");
        fs::write(
            &script,
            format!(
                r#"#!/bin/sh
[ "$1" = --version ] && echo '2.1.293 (Claude Code)' && exit 0
printf '%s\n' "$@" > "{dir}/args"
cat > "{dir}/stdin"
while [ $# -gt 0 ]; do [ "$1" = --resume ] && test -f "$2" && touch "{dir}/copy-existed"; shift; done
cat <<'FRAMES'
{frames}
FRAMES
exit {status}
"#,
                dir = dir.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    /// A Claude draft whose conversation is the claude-rpc fixture, adopted
    /// into `dir`.
    fn claude_session(dir: &Path) -> DraftSession {
        let session = DraftSession {
            id: "11111111-1111-4111-8111-111111111111".into(),
            dir: dir.to_path_buf(),
            config_dir: Some(dir.join("config")),
            fork_boundary: None,
            transcript: None,
        };
        fs::create_dir_all(dir).unwrap();
        fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../claude-rpc/tests/fixtures/transcripts/fork-source.jsonl"
            ),
            session.owned(),
        )
        .unwrap();
        session
    }

    #[cfg(unix)]
    fn run_claude(
        tmp: &Path,
        frames: &str,
        status: i32,
        previews: &mut Vec<Vec<String>>,
    ) -> Result<Vec<String>, String> {
        let session = claude_session(&tmp.join("draft"));
        let parent = tmp.join("temp");
        fs::create_dir_all(&parent).unwrap();
        let mut request = request("Hi there\n", "there\n", Action::Alternatives);
        request.cwd = tmp.to_path_buf();
        let config = SelectionConfig {
            model: Some("haiku".into()),
            ..SelectionConfig::default()
        };
        let before = fs::read(session.owned()).unwrap();
        let backend = Backend::Claude {
            program: fake_claude(tmp, frames, status),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            session: session.clone(),
        };
        let result = run(
            backend,
            &request,
            &config,
            &parent,
            &AtomicBool::new(false),
            |options| previews.push(options),
        );
        assert_eq!(fs::read(session.owned()).unwrap(), before);
        assert_eq!(fs::read_dir(&session.dir).unwrap().count(), 1);
        assert_eq!(
            fs::read_dir(&parent).unwrap().count(),
            0,
            "temp dir removed"
        );
        result
    }

    #[cfg(unix)]
    #[test]
    fn claude_answers_from_a_copy_of_the_conversation() {
        let tmp = tempfile::tempdir().unwrap();
        let frames = r#"{"type":"system","subtype":"init"}
{"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant"}}}
{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\"alternatives\":[\"over here\","}}}
{"type":"assistant","message":{"content":[{"type":"text","text":"{\"alternatives\":[\"over here\",\"here\"]}"}]}}
{"type":"result","subtype":"success","is_error":false,"result":"{\"alternatives\":[\"over here\",\"here\"]}","stop_reason":"end_turn","terminal_reason":"completed"}"#;
        let mut previews = Vec::new();
        let options = run_claude(tmp.path(), frames, 0, &mut previews).unwrap();
        assert_eq!(options, ["over here\n", "here\n"]);
        assert_eq!(previews[0], ["over here\n"]);
        assert_eq!(previews.last().unwrap(), &options);

        let args = fs::read_to_string(tmp.path().join("args")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert!(args.starts_with(&["-p", "--output-format", "stream-json"]));
        for flag in [
            "--no-session-persistence",
            "--fork-session",
            "--strict-mcp-config",
        ] {
            assert!(args.contains(&flag), "{flag}");
        }
        let tools = args.iter().position(|a| *a == "--tools").unwrap();
        assert_eq!(args[tools + 1], "");
        let model = args.iter().position(|a| *a == "--model").unwrap();
        assert_eq!(args[model + 1], "haiku");
        assert!(tmp.path().join("copy-existed").exists());
        let stdin = fs::read_to_string(tmp.path().join("stdin")).unwrap();
        assert!(
            stdin.starts_with("[Draftmux selection request]\n"),
            "{stdin}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_failures_retract_previews_and_say_why() {
        // A missing login: claude reports it, then exits 1.
        let tmp = tempfile::tempdir().unwrap();
        let frames = r#"{"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant"}}}
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"{\"alternatives\":[\"early\","}}}
{"type":"assistant","error":"authentication_failed","message":{"content":[{"type":"text","text":"Not logged in · Please run /login"}]}}
{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login","terminal_reason":"api_error"}"#;
        let mut previews = Vec::new();
        let err = run_claude(tmp.path(), frames, 1, &mut previews).unwrap_err();
        assert!(err.contains("isn't logged in"), "{err}");
        assert_eq!(previews.first().unwrap(), &["early\n"]);
        assert!(previews.last().unwrap().is_empty());

        // A reply with nothing but thinking.
        let tmp = tempfile::tempdir().unwrap();
        let frames = r#"{"type":"user","isSynthetic":true,"message":{"content":[{"type":"text","text":"[Your previous response had no visible output.]"}]}}
{"type":"result","subtype":"success","is_error":false,"result":"","stop_reason":"end_turn","terminal_reason":"completed"}"#;
        let err = run_claude(tmp.path(), frames, 0, &mut Vec::new()).unwrap_err();
        assert_eq!(err, "Claude Code's reply had no alternatives");

        // No result before exiting.
        let tmp = tempfile::tempdir().unwrap();
        let frames = r#"{"type":"system","subtype":"init"}"#;
        let err = run_claude(tmp.path(), frames, 0, &mut Vec::new()).unwrap_err();
        assert_eq!(err, "Claude Code didn't finish the selection request");
    }

    #[cfg(unix)]
    #[test]
    fn a_claude_draft_whose_transcript_is_lost_runs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = claude_session(&tmp.path().join("draft"));
        fs::remove_file(session.owned()).unwrap();
        session.transcript = Some(session.owned());
        let mut request = request("Hi there", "there", Action::Alternatives);
        request.cwd = tmp.path().to_path_buf();
        let backend = Backend::Claude {
            program: fake_claude(tmp.path(), "", 0),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            session,
        };
        let err = run(
            backend,
            &request,
            &SelectionConfig::default(),
            tmp.path(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(err.contains("transcript"), "{err}");
        assert!(!tmp.path().join("args").exists(), "claude never ran");
    }

    /// Needs a logged-in Claude Code 2.1.293 or later as `claude` on `PATH`:
    /// `cargo test -j 2 -p draftmux --bin draftmux real_claude_selection -- --ignored`.
    /// Leaves no transcript behind.
    #[test]
    #[ignore]
    fn real_claude_selection_call_leaves_the_draft_session_alone() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let program = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("claude"))
            .find(|path| path.is_file())
            .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().canonicalize().unwrap().join("cwd");
        fs::create_dir(&cwd).unwrap();
        let mut session = claude_session(&tmp.path().join("draft"));
        // Keychain logins don't follow another config dir.
        session.config_dir = None;
        let before = fs::read(session.owned()).unwrap();

        let text = "Thanks for the detailed report! I'll take a look tomorrow.\n";
        let start = text.find("I'll").unwrap();
        let request = Request {
            cwd: cwd.clone(),
            target: Target::Github,
            text: text.to_string(),
            range: start..text.len(),
            action: Action::Instruction("Make it more casual.".into()),
        };
        let parent = tmp.path().join("temp");
        fs::create_dir(&parent).unwrap();
        let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let mut first_preview = false;
        let options = run(
            Backend::Claude {
                program,
                env,
                session: session.clone(),
            },
            &request,
            &SelectionConfig {
                model: Some("haiku".into()),
                ..SelectionConfig::default()
            },
            &parent,
            &AtomicBool::new(false),
            |options| first_preview |= !options.is_empty(),
        )
        .unwrap();
        eprintln!("options: {options:?}");
        assert!(first_preview);
        assert!(!options.is_empty() && options.len() <= 3);
        assert!(options.iter().all(|o| o.ends_with('\n')));
        assert_eq!(fs::read(session.owned()).unwrap(), before);
        assert_eq!(fs::read_dir(&session.dir).unwrap().count(), 1);
        assert_eq!(
            fs::read_dir(&parent).unwrap().count(),
            0,
            "temp dir removed"
        );

        // Claude Code names the project directory after the cwd; whatever it
        // made for this temporary one must hold no transcript.
        let name = cwd.to_str().unwrap().replace(['/', '.', '_'], "-");
        let project = home.join(".claude/projects").join(&name);
        if project.exists() {
            let files: Vec<_> = walk(&project);
            assert!(files.is_empty(), "left behind: {files:?}");
            let memory = project.join("memory");
            if memory.is_dir() {
                fs::remove_dir(memory).unwrap();
            }
            fs::remove_dir(&project).unwrap();
        }
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
        files
    }
}

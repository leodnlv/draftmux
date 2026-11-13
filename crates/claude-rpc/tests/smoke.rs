//! Live checks against the installed `claude` (`CLAUDE_BIN`, by default
//! `~/.local/bin/claude`), with synthetic prompts in a temporary directory.
//! They use the account's login and spend a few `haiku` turns, so they are
//! ignored by default:
//!
//! ```sh
//! cargo test -j 2 -p draftmux-claude-rpc --test smoke -- --ignored
//! ```
//!
//! The transcripts the CLI writes under `~/.claude/projects/` for the
//! temporary directory are removed afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use draftmux_claude_rpc::{Agent, AgentEvent, Config, Error, LaunchSpec, Session};
use futures::channel::mpsc::UnboundedReceiver;
use futures::executor::block_on;
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(90);

fn program() -> PathBuf {
    std::env::var_os("CLAUDE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/bin/claude"))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap())
}

/// A temporary working directory whose native transcripts are removed on
/// drop.
struct Scratch {
    dir: tempfile::TempDir,
    cwd: PathBuf,
    /// The session whose process must be gone first: claude appends to the
    /// transcript as it exits.
    session: String,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        Self {
            dir,
            cwd,
            session: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// `~/.claude/projects/<cwd with / and . as ->`.
    fn projects(&self) -> PathBuf {
        let encoded: String = self
            .cwd
            .to_str()
            .unwrap()
            .chars()
            .map(|c| if c == '/' || c == '.' { '-' } else { c })
            .collect();
        home().join(".claude/projects").join(encoded)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while running(&self.session) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let projects = self.projects();
        // Only a directory named after this temporary directory.
        let name = self.dir.path().file_name().unwrap().to_str().unwrap();
        assert!(
            projects
                .to_str()
                .unwrap()
                .ends_with(&name.replace('.', "-"))
        );
        let _ = std::fs::remove_dir_all(projects);
    }
}

fn spec(cwd: &Path, session: Session) -> LaunchSpec {
    LaunchSpec {
        program: program(),
        cwd: cwd.to_path_buf(),
        env: std::env::vars_os().collect(),
        extra_env: vec![],
        model: Some("haiku".into()),
        tools: vec![],
        setting_sources: vec![],
        session: Some(session),
        hooks: vec![],
        permission_prompt: true,
    }
}

fn config() -> Config {
    Config {
        idle_timeout: Duration::from_secs(120),
        request_timeout: Duration::from_secs(30),
        max_record: 16 << 20,
    }
}

fn next(events: &mut UnboundedReceiver<AgentEvent>) -> AgentEvent {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match events.try_recv() {
            Ok(event) => return event,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Err(err) => panic!("no event within {TIMEOUT:?}: {err:?}"),
        }
    }
}

/// Frames until the run settles, and how it settled.
fn run(events: &mut UnboundedReceiver<AgentEvent>) -> (Vec<Value>, bool, Option<String>) {
    let mut frames = Vec::new();
    loop {
        match next(events) {
            AgentEvent::Frame(frame) => frames.push(frame),
            AgentEvent::RunSettled { aborted, error } => return (frames, aborted, error),
            AgentEvent::Crashed { error, .. } => panic!("crashed: {error}"),
            _ => {}
        }
    }
}

fn text(frames: &[Value]) -> (String, String) {
    let deltas = frames
        .iter()
        .filter(|f| f["type"] == "stream_event" && f["event"]["delta"]["type"] == "text_delta")
        .filter_map(|f| f["event"]["delta"]["text"].as_str())
        .collect();
    let blocks = frames
        .iter()
        .filter(|f| f["type"] == "assistant")
        .flat_map(|f| {
            f["message"]["content"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str().map(str::to_owned))
        .collect();
    (deltas, blocks)
}

/// Whether a process with `needle` in its arguments is running.
fn running(needle: &str) -> bool {
    let out = Command::new("pgrep")
        .arg("-f")
        .arg(needle)
        .output()
        .unwrap();
    !out.stdout.is_empty()
}

#[test]
#[ignore = "runs the real claude"]
fn live_turns_interrupt_and_models() {
    let scratch = Scratch::new();
    let id = scratch.session.clone();
    let (agent, mut events) = Agent::new(spec(&scratch.cwd, Session::New(id.clone())), config());

    block_on(agent.prompt("Reply with exactly the word OK.".into())).unwrap();
    let AgentEvent::Ready(init) = next(&mut events) else {
        panic!("expected Ready first");
    };
    assert!(init.models.iter().any(|m| m.value == "haiku"), "{init:?}");
    assert!(matches!(next(&mut events), AgentEvent::RunStarted));
    let (frames, aborted, error) = run(&mut events);
    assert_eq!((aborted, error), (false, None));
    let (deltas, blocks) = text(&frames);
    assert_eq!(deltas, blocks, "streamed text equals the final text");
    assert!(blocks.contains("OK"), "{blocks}");
    assert!(running(&id));

    block_on(agent.prompt("Write the numbers from 1 to 400 as words, one per line.".into()))
        .unwrap();
    assert!(matches!(next(&mut events), AgentEvent::RunStarted));
    loop {
        if let AgentEvent::Frame(frame) = next(&mut events)
            && frame["type"] == "stream_event"
            && frame["event"]["delta"]["type"] == "text_delta"
        {
            break;
        }
    }
    let started = Instant::now();
    block_on(agent.interrupt()).unwrap();
    let acked = started.elapsed();
    let (_, aborted, error) = run(&mut events);
    assert!(aborted, "the run was interrupted");
    assert_eq!(error, None);
    eprintln!(
        "interrupt acknowledged after {acked:?}, settled after {:?}",
        started.elapsed()
    );

    assert!(matches!(
        block_on(agent.set_model("not-a-model".into())),
        Err(Error::Rejected(_))
    ));
    block_on(agent.set_model("sonnet".into())).unwrap();
    // The switch is echoed as a replayed `user` frame outside any run.
    let AgentEvent::Frame(echo) = next(&mut events) else {
        panic!("expected the model switch echo");
    };
    assert_eq!(
        (&echo["type"], &echo["isReplay"]),
        (&"user".into(), &true.into())
    );
    block_on(agent.prompt("Reply with exactly the word OK.".into())).unwrap();
    assert!(matches!(next(&mut events), AgentEvent::RunStarted));
    let (frames, aborted, error) = run(&mut events);
    assert_eq!((aborted, error), (false, None));
    let init = frames
        .iter()
        .find(|f| f["type"] == "system" && f["subtype"] == "init")
        .unwrap();
    assert!(init["model"].as_str().unwrap().contains("sonnet"), "{init}");

    drop(agent);
    let deadline = Instant::now() + Duration::from_secs(10);
    while running(&id) {
        assert!(Instant::now() < deadline, "claude still running after drop");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "runs the real claude"]
fn live_missing_session() {
    let scratch = Scratch::new();
    let id = scratch.session.clone();
    let (agent, mut events) = Agent::new(
        spec(&scratch.cwd, Session::Resume(id.clone().into())),
        config(),
    );
    assert_eq!(block_on(agent.prompt("hi".into())), Err(Error::Exited));
    let mut early_result = false;
    loop {
        match next(&mut events) {
            AgentEvent::Frame(frame) if frame["type"] == "result" => early_result = true,
            AgentEvent::Crashed {
                error,
                failed_prompt,
            } => {
                assert!(error.contains("No conversation found"), "{error}");
                assert_eq!(failed_prompt, None);
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(early_result);
    assert!(!running(&id));
}

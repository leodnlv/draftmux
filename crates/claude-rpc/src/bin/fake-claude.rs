//! A scripted stand-in for `claude` in stream-json mode, used by the
//! integration tests.
//!
//! `FAKE_CLAUDE_DIR` holds `script-<n>` for the n-th process started with
//! it; each line is one step:
//!
//! - `>>> {json}`: read one stdin line and require it to equal `json`,
//!   ignoring a control request's `request_id`. The script's id is mapped to
//!   the received one.
//! - `{json}`: print it; a control response's `request_id` is rewritten
//!   through that map.
//! - `### sleep <ms>`, `### stderr <text>`, `### exit <code>`.
//! - `### eof`: wait for stdin to close, then exit 0. `eof-<n>` is created
//!   first.
//! - `### hang`: sleep until killed. `### ignore-term`: ignore SIGTERM.
//! - `### close-stdin`: close stdin and go on.
//! - `### big <bytes>`: print a record of that many bytes.
//! - `### spawn-child`: start a `sleep` in the process group and write its
//!   pid to `child-<n>`.
//! - `### no-input <ms>`: require that no line arrives for that long.
//! - `### append <path> <line>`: append a line to a file, creating it and
//!   its directory, as claude writes its transcript.
//! - `### write <path> <json string>`: replace a file's content with the
//!   string, as a tool writes the draft.
//!
//! A mismatch prints to stderr and exits 99. The pid, arguments, cwd and
//! environment are written to `record-<n>` first.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    let dir = PathBuf::from(std::env::var_os("FAKE_CLAUDE_DIR").unwrap());
    let n = claim(&dir);
    let record = json!({
        "pid": std::process::id(),
        "args": std::env::args().skip(1).collect::<Vec<_>>(),
        "cwd": std::env::current_dir().unwrap(),
        "env": std::env::vars().collect::<HashMap<_, _>>(),
    });
    std::fs::write(dir.join(format!("record-{n}")), record.to_string()).unwrap();
    let script = std::fs::read_to_string(dir.join(format!("script-{n}"))).unwrap();
    // Unbuffered by std, so `no-input` can poll the descriptor.
    // SAFETY: fd 0 is open and only used through this reader.
    let mut stdin = BufReader::new(unsafe { File::from_raw_fd(0) });
    let mut stdout = std::io::stdout().lock();
    let mut ids = HashMap::<String, Value>::new();

    for step in script.lines().filter(|line| !line.trim().is_empty()) {
        if let Some(expected) = step.strip_prefix(">>> ") {
            let mut expected: Value = serde_json::from_str(expected).unwrap();
            let mut line = String::new();
            if stdin.read_line(&mut line).unwrap() == 0 {
                fail(&format!("stdin closed, expected {expected}"));
            }
            let mut received: Value = serde_json::from_str(&line).unwrap();
            if expected["type"] == "control_request" {
                let from = expected.as_object_mut().unwrap().remove("request_id");
                let to = received.as_object_mut().unwrap().remove("request_id");
                if let (Some(Value::String(from)), Some(to)) = (from, to) {
                    ids.insert(from, to);
                }
            }
            if received != expected {
                fail(&format!("expected {expected}, received {received}"));
            }
        } else if let Some(directive) = step.strip_prefix("### ") {
            let (name, arg) = directive.split_once(' ').unwrap_or((directive, ""));
            match name {
                "sleep" => std::thread::sleep(Duration::from_millis(arg.parse().unwrap())),
                "stderr" => eprintln!("{arg}"),
                "exit" => std::process::exit(arg.parse().unwrap()),
                "eof" => {
                    std::fs::write(dir.join(format!("eof-{n}")), "").unwrap();
                    let mut rest = String::new();
                    if stdin.read_line(&mut rest).unwrap() > 0 {
                        fail(&format!("expected stdin to close, received {rest}"));
                    }
                    std::process::exit(0);
                }
                "hang" => loop {
                    std::thread::sleep(Duration::from_secs(60));
                },
                // SAFETY: plain syscalls on this process.
                "ignore-term" => unsafe {
                    libc::signal(libc::SIGTERM, libc::SIG_IGN);
                },
                "close-stdin" => unsafe {
                    libc::close(0);
                },
                "big" => {
                    let pad = "x".repeat(arg.parse::<usize>().unwrap().saturating_sub(24));
                    writeln!(stdout, "{{\"type\":\"big\",\"pad\":\"{pad}\"}}").unwrap();
                    stdout.flush().unwrap();
                }
                "spawn-child" => {
                    // Left running on purpose: the agent must end it.
                    let child = std::process::Command::new("sleep")
                        .arg("600")
                        .spawn()
                        .unwrap();
                    std::fs::write(dir.join(format!("child-{n}")), child.id().to_string()).unwrap();
                    std::mem::forget(child);
                }
                "no-input" => {
                    let ms: i32 = arg.parse().unwrap();
                    if !stdin.buffer().is_empty() || readable(ms) {
                        let mut line = String::new();
                        if stdin.read_line(&mut line).unwrap() > 0 {
                            fail(&format!("expected no input, received {line}"));
                        }
                    }
                }
                "append" => {
                    let (path, line) = arg.split_once(' ').unwrap();
                    let path = Path::new(path);
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    let mut file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .unwrap();
                    writeln!(file, "{line}").unwrap();
                }
                "write" => {
                    let (path, text) = arg.split_once(' ').unwrap();
                    let text: String = serde_json::from_str(text).unwrap();
                    std::fs::write(path, text).unwrap();
                }
                other => fail(&format!("unknown directive {other}")),
            }
        } else {
            let mut value: Value = serde_json::from_str(step).unwrap();
            if value["type"] == "control_response"
                && let Some(Value::String(id)) = value["response"].get("request_id")
                && let Some(to) = ids.get(id)
            {
                value["response"]["request_id"] = to.clone();
            }
            writeln!(stdout, "{value}").unwrap();
            stdout.flush().unwrap();
        }
    }
}

/// The lowest process number not yet taken in `dir`.
fn claim(dir: &Path) -> usize {
    (0..)
        .find(|n| {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join(format!("claimed-{n}")))
            {
                Ok(_) => true,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => false,
                // The test is over and removed its directory.
                Err(err) => fail(&format!("claiming a script in {}: {err}", dir.display())),
            }
        })
        .unwrap()
}

/// Whether stdin becomes readable within `ms`.
fn readable(ms: i32) -> bool {
    let mut fd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: polls one valid pollfd.
    unsafe { libc::poll(&mut fd, 1, ms) > 0 }
}

fn fail(message: &str) -> ! {
    eprintln!("fake-claude: {message}");
    std::process::exit(99);
}

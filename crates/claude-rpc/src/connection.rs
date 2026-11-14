//! One running `claude` process: stdio pipes, reader and writer threads and
//! shutdown.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long claude gets to exit after stdin closes before SIGTERM, then
/// before the process group is killed. claude exits about half a second after stdin closes
/// while idle; a run in progress is finished first.
const EOF_GRACE: Duration = Duration::from_secs(3);
const TERM_GRACE: Duration = Duration::from_secs(2);
/// How long to wait for stdout and stderr to reach EOF after claude exits.
/// A process claude started can keep the pipes open.
const DRAIN_GRACE: Duration = Duration::from_secs(1);
const STDERR_TAIL: usize = 16 * 1024;
/// How long a stopped claude can take to exit: the stop escalation, with a
/// second to spare.
pub(crate) const EXIT_BOUND: Duration = EOF_GRACE
    .saturating_add(TERM_GRACE)
    .saturating_add(Duration::from_secs(1));

pub(crate) enum Message {
    Line(Value),
    /// stdout ended, so nothing more is read from it.
    Closed(Closed),
    /// Writing to stdin failed; nothing more is written.
    WriteFailed(String),
    /// The process exited and its output was read. Always the last message.
    Exited {
        status: String,
        stderr: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Closed {
    Eof,
    Error(String),
    /// A record was longer than the limit. Reading stops there: skipping it
    /// could drop a callback request the CLI then waits for.
    Oversized(usize),
}

/// A line for stdin. A ticketed line is skipped if its ticket was cancelled
/// before the writer got to it.
pub(crate) struct Outgoing {
    pub line: String,
    pub ticket: Option<Arc<Ticket>>,
}

/// Whether a queued callback response may still be written.
#[derive(Default)]
pub(crate) struct Ticket(AtomicU8);

const QUEUED: u8 = 0;
const WRITING: u8 = 1;
const CANCELLED: u8 = 2;

impl Ticket {
    /// Withdraws the line unless the writer already started it; a started
    /// line completes so stdin stays framed.
    pub(crate) fn cancel(&self) {
        let _ = self
            .0
            .compare_exchange(QUEUED, CANCELLED, Ordering::SeqCst, Ordering::SeqCst);
    }

    fn begin(&self) -> bool {
        self.0
            .compare_exchange(QUEUED, WRITING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub(crate) fn queued(&self) -> bool {
        self.0.load(Ordering::SeqCst) == QUEUED
    }
}

pub(crate) struct Connection {
    /// Lines for the writer thread, which owns stdin. Writing blocks while
    /// claude doesn't read, so callers only queue. `None` once stopping.
    stdin: Option<mpsc::Sender<Outgoing>>,
    group: draftmux_process::Group,
    exit: Arc<ExitState>,
}

/// Tells when a process has exited, after its connection is gone.
#[derive(Clone)]
pub(crate) struct Exit(Arc<ExitState>);

impl Exit {
    /// Waits until the process has exited, at most until `deadline`.
    /// Returns whether it has.
    pub(crate) fn wait(&self, deadline: Instant) -> bool {
        self.0.wait_until(deadline, |inner| inner.exited)
    }
}

#[derive(Default)]
struct ExitState {
    inner: Mutex<ExitInner>,
    changed: Condvar,
}

#[derive(Default)]
struct ExitInner {
    exited: bool,
    stdout_done: bool,
    stderr_done: bool,
}

type OnMessage = Arc<dyn Fn(Message) + Send + Sync>;

impl Connection {
    /// Spawns `command` in its own process group with piped stdio. Every
    /// record claude prints, then its exit, is passed to `on_message` from a
    /// background thread. A record longer than `max_record` bytes ends
    /// reading.
    pub(crate) fn spawn(
        mut command: Command,
        max_record: usize,
        on_message: impl Fn(Message) + Send + Sync + 'static,
    ) -> io::Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (child, group) = draftmux_process::spawn(&mut command)?;
        // Shared with the wait thread, so it can still be reaped here if
        // that thread fails to start.
        let child = Arc::new(Mutex::new(child));
        let exit = Arc::new(ExitState::default());
        match start_threads(&child, &group, max_record, Arc::new(on_message), &exit) {
            Ok(stdin) => Ok(Self {
                stdin: Some(stdin),
                group,
                exit,
            }),
            Err(err) => {
                group.kill();
                let _ = child.lock().unwrap().wait();
                Err(err)
            }
        }
    }

    /// Queues `line` for claude. Fails once stopping or writing has failed.
    pub(crate) fn send(&self, line: String, ticket: Option<Arc<Ticket>>) -> io::Result<()> {
        self.stdin
            .as_ref()
            .and_then(|stdin| stdin.send(Outgoing { line, ticket }).ok())
            .ok_or_else(|| io::Error::new(ErrorKind::BrokenPipe, "claude's stdin is closed"))
    }

    /// Closes stdin after the queued lines so claude exits once any run
    /// ends, then escalates to SIGTERM and killing the process group if it
    /// doesn't, which also ends a write claude isn't reading. Returns at
    /// once. Calling it again does nothing.
    pub(crate) fn stop(&mut self) {
        if self.stdin.take().is_none() {
            return;
        }
        let (group, exit) = (self.group.clone(), self.exit.clone());
        let escalate = move || {
            if exit.wait_until(Instant::now() + EOF_GRACE, |inner| inner.exited) {
                return;
            }
            // The child can't be reaped, and its pid reused, before
            // `exited` is set.
            group.terminate();
            if exit.wait_until(Instant::now() + TERM_GRACE, |inner| inner.exited) {
                return;
            }
            group.kill();
        };
        if let Err(err) = thread::Builder::new()
            .name("claude-stop".into())
            .spawn(escalate)
        {
            eprintln!("claude-rpc: couldn't start the stop thread: {err}");
            self.group.kill();
        }
    }

    pub(crate) fn exit(&self) -> Exit {
        Exit(self.exit.clone())
    }

    /// Kills the process group now, for a process that can't be trusted
    /// to exit.
    pub(crate) fn kill(&mut self) {
        self.stdin = None;
        if !self.exit.inner.lock().unwrap().exited {
            self.group.kill();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts the reader, writer and wait threads and returns the writer's
/// queue.
fn start_threads(
    child: &Arc<Mutex<Child>>,
    group: &draftmux_process::Group,
    max_record: usize,
    on_message: OnMessage,
    exit: &Arc<ExitState>,
) -> io::Result<mpsc::Sender<Outgoing>> {
    let (stdin, stdout, stderr) = {
        let mut child = child.lock().unwrap();
        let stdin: ChildStdin = child.stdin.take().expect("stdin is piped");
        let stdout: ChildStdout = child.stdout.take().expect("stdout is piped");
        (stdin, stdout, child.stderr.take().expect("stderr is piped"))
    };
    let tail = Arc::new(Mutex::new(VecDeque::new()));

    let (exit_out, on_line) = (exit.clone(), on_message.clone());
    thread::Builder::new()
        .name("claude-stdout".into())
        .spawn(move || {
            let closed = read_records(stdout, max_record, |v| on_line(Message::Line(v)));
            on_line(Message::Closed(closed));
            exit_out.update(|inner| inner.stdout_done = true);
        })?;

    let (exit_err, tail_err) = (exit.clone(), tail.clone());
    thread::Builder::new()
        .name("claude-stderr".into())
        .spawn(move || {
            read_tail(stderr, &tail_err);
            exit_err.update(|inner| inner.stderr_done = true);
        })?;

    let (lines, queue) = mpsc::channel::<Outgoing>();
    let on_write = on_message.clone();
    thread::Builder::new()
        .name("claude-stdin".into())
        .spawn(move || write_lines(stdin, queue, |err| on_write(Message::WriteFailed(err))))?;

    let (child, group, exit_wait) = (child.clone(), group.clone(), exit.clone());
    thread::Builder::new()
        .name("claude-wait".into())
        .spawn(move || {
            let status = match child.lock().unwrap().wait() {
                Ok(status) => describe(status),
                Err(err) => format!("wait failed: {err}"),
            };
            exit_wait.update(|inner| inner.exited = true);
            // Whatever is left of the group, such as a tool's process,
            // goes with claude.
            group.kill();
            // Deliver everything claude wrote before reporting the exit.
            exit_wait.wait_until(Instant::now() + DRAIN_GRACE, |inner| {
                inner.stdout_done && inner.stderr_done
            });
            let stderr = String::from_utf8_lossy(tail.lock().unwrap().make_contiguous())
                .trim()
                .to_string();
            on_message(Message::Exited { status, stderr });
        })?;
    Ok(lines)
}

/// `exit status: N`, or on Unix `signal: N` for a process a signal ended.
fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return format!("signal: {signal}");
    }
    match status.code() {
        Some(code) => format!("exit status: {code}"),
        None => status.to_string(),
    }
}

/// Writes queued lines until the queue closes or a write fails, then
/// closes stdin.
fn write_lines(
    mut stdin: impl Write,
    queue: mpsc::Receiver<Outgoing>,
    on_error: impl FnOnce(String),
) {
    for outgoing in queue {
        if outgoing
            .ticket
            .as_ref()
            .is_some_and(|ticket| !ticket.begin())
        {
            continue;
        }
        if let Err(err) = stdin
            .write_all(outgoing.line.as_bytes())
            .and_then(|()| stdin.flush())
        {
            on_error(err.to_string());
            return;
        }
    }
}

impl ExitState {
    fn update(&self, f: impl FnOnce(&mut ExitInner)) {
        f(&mut self.inner.lock().unwrap());
        self.changed.notify_all();
    }

    /// Waits until `done` holds or `deadline` passes. Returns whether it holds.
    fn wait_until(&self, deadline: Instant, done: impl Fn(&ExitInner) -> bool) -> bool {
        let mut inner = self.inner.lock().unwrap();
        while !done(&inner) {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            inner = self.changed.wait_timeout(inner, deadline - now).unwrap().0;
        }
        true
    }
}

/// Reads LF-terminated JSON records of at most `max` bytes until stdout
/// ends. Only `\n` frames a record; U+2028 and other separators can appear
/// inside JSON strings. Lines that aren't JSON are logged and skipped.
fn read_records(stdout: impl Read, max: usize, mut on_record: impl FnMut(Value)) -> Closed {
    let mut reader = BufReader::new(stdout);
    let mut record = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Closed::Error(err.to_string()),
        };
        if available.is_empty() {
            parse_record(&record, &mut on_record);
            return Closed::Eof;
        }
        let (chunk, end) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (&available[..i], true),
            None => (available, false),
        };
        if record.len() + chunk.len() > max {
            return Closed::Oversized(max);
        }
        record.extend_from_slice(chunk);
        let used = chunk.len() + usize::from(end);
        reader.consume(used);
        if end {
            parse_record(&record, &mut on_record);
            record.clear();
        }
    }
}

fn parse_record(record: &[u8], on_record: &mut impl FnMut(Value)) {
    let line = record.strip_suffix(b"\r").unwrap_or(record);
    if line.is_empty() {
        return;
    }
    match serde_json::from_slice(line) {
        Ok(value) => on_record(value),
        Err(err) => eprintln!(
            "claude-rpc: skipping a malformed record from claude ({err}): {}",
            String::from_utf8_lossy(&line[..line.len().min(200)])
        ),
    }
}

/// Keeps the last [`STDERR_TAIL`] bytes of stderr for error reports.
fn read_tail(mut stderr: impl Read, tail: &Mutex<VecDeque<u8>>) {
    let mut chunk = [0; 4096];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                let mut tail = tail.lock().unwrap();
                tail.extend(&chunk[..n]);
                let excess = tail.len().saturating_sub(STDERR_TAIL);
                tail.drain(..excess);
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn records(input: &[u8], max: usize) -> (Vec<Value>, Closed) {
        let mut out = Vec::new();
        // A one-byte buffer exercises records split across reads.
        let closed = read_records(io::BufReader::with_capacity(1, input), max, |v| out.push(v));
        (out, closed)
    }

    #[test]
    fn framing() {
        let input = "{\"a\":1}\r\n\n{\"b\":\"x\u{2028}y\"}\nnot json\n{\"c\":3}";
        let (out, closed) = records(input.as_bytes(), 1024);
        assert_eq!(
            out,
            [json!({"a":1}), json!({"b":"x\u{2028}y"}), json!({"c":3})]
        );
        assert_eq!(closed, Closed::Eof);
    }

    #[test]
    fn record_limit() {
        let small = "{\"a\":\"0123456789\"}\n";
        let (out, closed) = records(small.repeat(3).as_bytes(), small.len() - 1);
        assert_eq!(out.len(), 3, "the limit applies per record");
        assert_eq!(closed, Closed::Eof);

        let input = format!("{small}{{\"big\":\"{}\"}}\n{small}", "x".repeat(100));
        let (out, closed) = records(input.as_bytes(), 64);
        assert_eq!(out.len(), 1);
        assert_eq!(closed, Closed::Oversized(64));
    }

    #[test]
    fn cancelled_lines_are_skipped() {
        let (tx, rx) = mpsc::channel();
        let sent = Arc::new(Ticket::default());
        let withdrawn = Arc::new(Ticket::default());
        withdrawn.cancel();
        for (line, ticket) in [("a\n", None), ("b\n", Some(withdrawn)), ("c\n", Some(sent))] {
            tx.send(Outgoing {
                line: line.into(),
                ticket,
            })
            .unwrap();
        }
        drop(tx);
        let mut out = Vec::new();
        write_lines(&mut out, rx, |err| panic!("{err}"));
        assert_eq!(out, b"a\nc\n");
    }

    #[test]
    fn a_started_line_cannot_be_cancelled() {
        let ticket = Ticket::default();
        assert!(ticket.begin());
        ticket.cancel();
        assert!(!ticket.queued());
        assert!(!ticket.begin());
        assert_eq!(ticket.0.load(Ordering::SeqCst), WRITING);
    }
}

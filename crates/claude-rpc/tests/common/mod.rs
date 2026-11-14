//! Building `fake-claude` scripts for a draft's agent: the lines
//! `ClaudeAgent` writes and claude's answers.

#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use draftmux_agent::Event;
use serde_json::{Value, json};

pub const TIMEOUT: Duration = Duration::from_secs(10);

pub const INIT: &str = r#"
>>> {"type":"control_request","request_id":"init","request":{"subtype":"initialize","hooks":{"PreToolUse":[{"hookCallbackIds":["draftmux_hook_0"],"timeout":30}]}}}
{"type":"control_response","response":{"subtype":"success","request_id":"init","response":{"models":[{"value":"haiku"}],"hooks_applied":true}}}
"#;

pub const SLACK: &str = "Markdown for a Slack message";

pub fn line(value: Value) -> String {
    value.to_string()
}

pub fn expect(value: Value) -> String {
    format!(">>> {value}")
}

/// The prompt as written: `blocks` ahead of `text`, or `text` alone.
pub fn prompt_in(blocks: &[String], text: &str) -> String {
    let content = if blocks.is_empty() {
        json!(text)
    } else {
        let blocks: Vec<Value> = blocks
            .iter()
            .map(String::as_str)
            .chain([text])
            .map(|text| json!({"type": "text", "text": text}))
            .collect();
        json!(blocks)
    };
    expect(
        json!({"type": "user", "session_id": "", "parent_tool_use_id": null,
        "message": {"role": "user", "content": content}}),
    )
}

pub fn target_block(syntax: &str) -> String {
    format!("<draftmux-context>\nTarget platform: {syntax}\n</draftmux-context>")
}

pub fn draft_block(text: &str) -> String {
    let newline = if text.ends_with('\n') { "" } else { "\n" };
    format!("<draftmux-draft>\n{text}{newline}</draftmux-draft>")
}

pub fn state(state: &str) -> String {
    line(json!({"type": "system", "subtype": "session_state_changed", "state": state}))
}

pub fn settle(terminal: &str) -> String {
    let aborted = terminal.starts_with("aborted");
    [
        line(
            json!({"type": "result", "is_error": aborted, "terminal_reason": terminal,
            "subtype": if aborted { "error_during_execution" } else { "success" }}),
        ),
        state("idle"),
    ]
    .join("\n")
}

pub fn tool_use(id: &str, name: &str, input: Value) -> String {
    line(
        json!({"type": "assistant", "message": {"id": format!("m-{id}"),
        "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}}),
    )
}

pub fn hook(request: &str, tool_use_id: &str, name: &str, input: Value) -> String {
    line(
        json!({"type": "control_request", "request_id": request, "request": {
        "subtype": "hook_callback", "callback_id": "draftmux_hook_0", "tool_use_id": tool_use_id,
        "input": {"hook_event_name": "PreToolUse", "tool_name": name, "tool_input": input,
            "tool_use_id": tool_use_id}}}),
    )
}

pub fn permission(request: &str, tool_use_id: &str, name: &str, input: Value) -> String {
    line(
        json!({"type": "control_request", "request_id": request, "request": {
        "subtype": "can_use_tool", "tool_name": name, "input": input,
        "tool_use_id": tool_use_id}}),
    )
}

pub fn answer(request: &str, response: Value) -> String {
    expect(json!({"type": "control_response", "response":
        {"subtype": "success", "request_id": request, "response": response}}))
}

pub fn proceed(request: &str) -> String {
    answer(request, json!({}))
}

/// The hook's refusal, whatever its reason.
pub fn denied(reason: &str) -> Value {
    json!({"hookSpecificOutput": {"hookEventName": "PreToolUse",
        "permissionDecision": "deny", "permissionDecisionReason": reason}})
}

pub fn tool_result(id: &str, is_error: bool) -> String {
    let mut block = json!({"type": "tool_result", "tool_use_id": id, "content": "done"});
    if is_error {
        block["is_error"] = json!(true);
    }
    line(json!({"type": "user", "message": {"role": "user", "content": [block]}}))
}

pub fn write_file(path: &Path, text: &str) -> String {
    format!("### write {} {}", path.display(), json!(text))
}

/// `(id, draft_write, is_error)` of each finished tool.
pub fn finished_tools(events: &[Event]) -> Vec<(String, bool, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ToolFinished {
                id,
                result,
                draft_write,
            } => Some((id.clone(), *draft_write, result.is_error)),
            _ => None,
        })
        .collect()
}

pub fn edit(path: &Path, old: &str, new: &str) -> Value {
    json!({"file_path": path, "old_string": old, "new_string": new, "replace_all": false})
}

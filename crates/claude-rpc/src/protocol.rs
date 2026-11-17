//! Envelopes of the stream-json control protocol and the few frame fields
//! the lifecycle reads. Conversation frames stay `serde_json::Value`, so
//! fields and frame types the CLI adds pass through.

use serde_json::{Value, json};

use crate::launch::PreToolUseHook;

/// What a process reported at `initialize`. The response also holds
/// account details, which are dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Init {
    pub models: Vec<ModelInfo>,
    /// claude confirmed it registered the hooks sent with `initialize`
    /// (`hooks_applied`).
    pub hooks_applied: bool,
}

/// A model `set_model` accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// What `set_model` and `--model` take, such as `sonnet`.
    pub value: String,
    /// The model id it currently stands for.
    pub resolved_model: Option<String>,
    pub display_name: String,
    pub description: String,
    /// The efforts `--effort` and `set_effort` take for the model
    /// (`supportedEffortLevels`), lowest first; empty when it has none.
    pub effort_levels: Vec<String>,
}

impl Init {
    pub(crate) fn parse(response: &Value) -> Self {
        let text = |v: &Value, key| v[key].as_str().map(str::to_owned);
        let models = response["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| {
                let value = text(model, "value")?;
                Some(ModelInfo {
                    resolved_model: text(model, "resolvedModel"),
                    display_name: text(model, "displayName").unwrap_or_else(|| value.clone()),
                    description: text(model, "description").unwrap_or_default(),
                    effort_levels: model["supportedEffortLevels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|level| level.as_str().map(str::to_owned))
                        .collect(),
                    value,
                })
            })
            .collect();
        Self {
            models,
            hooks_applied: response["hooks_applied"] == true,
        }
    }
}

pub(crate) fn line(value: Value) -> String {
    let mut line = value.to_string();
    line.push('\n');
    line
}

pub(crate) fn control_request(id: &str, request: Value) -> String {
    line(json!({"type": "control_request", "request_id": id, "request": request}))
}

pub(crate) fn control_success(id: &str, response: Value) -> String {
    line(json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": response},
    }))
}

pub(crate) fn control_error(id: &str, error: &str) -> String {
    line(json!({
        "type": "control_response",
        "response": {"subtype": "error", "request_id": id, "error": error},
    }))
}

/// A user message: `text` alone, or after the `context` blocks.
pub(crate) fn user_message(context: &[String], text: &str) -> String {
    let content = if context.is_empty() {
        json!(text)
    } else {
        let blocks: Vec<Value> = context
            .iter()
            .map(String::as_str)
            .chain([text])
            .map(|text| json!({"type": "text", "text": text}))
            .collect();
        json!(blocks)
    };
    line(json!({
        "type": "user",
        "session_id": "",
        "parent_tool_use_id": null,
        "message": {"role": "user", "content": content},
    }))
}

pub(crate) fn permission_allow(input: Value) -> Value {
    json!({"behavior": "allow", "updatedInput": input})
}

pub(crate) fn permission_deny(message: &str) -> Value {
    json!({"behavior": "deny", "message": message})
}

/// No decision: the tool call proceeds to the usual permission checks.
pub(crate) fn hook_continue() -> Value {
    json!({})
}

/// Blocks the call. An error response or a malformed decision would let it
/// run, so every refusal, including internal failures, is this.
pub(crate) fn hook_deny(reason: &str) -> Value {
    json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }})
}

/// The callback id of `LaunchSpec::hooks[index]`.
pub(crate) fn hook_id(index: usize) -> String {
    format!("draftmux_hook_{index}")
}

/// The `hooks` field of `initialize`.
pub(crate) fn hooks(hooks: &[PreToolUseHook]) -> Value {
    let matchers: Vec<Value> = hooks
        .iter()
        .enumerate()
        .map(|(i, hook)| {
            let mut entry = json!({"hookCallbackIds": [hook_id(i)]});
            if let Some(matcher) = &hook.matcher {
                entry["matcher"] = json!(matcher);
            }
            if let Some(timeout) = hook.timeout {
                entry["timeout"] = json!(timeout);
            }
            entry
        })
        .collect();
    json!({ "PreToolUse": matchers })
}

/// Whether a run's `result` says it was interrupted (`aborted_streaming`,
/// `aborted_tools`).
pub(crate) fn result_aborted(result: &Value) -> bool {
    result["terminal_reason"]
        .as_str()
        .is_some_and(|reason| reason.starts_with("aborted"))
}

/// The error a `result` reports. A failed login gives `is_error` with
/// subtype `success`, so the subtype isn't consulted.
pub(crate) fn result_error(result: &Value) -> Option<String> {
    if result["is_error"] != true {
        return None;
    }
    let text = result["result"].as_str().filter(|text| !text.is_empty());
    let errors = result["errors"].as_array().map(|errors| {
        errors
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("; ")
    });
    Some(
        text.map(str::to_owned)
            .or(errors.filter(|errors| !errors.is_empty()))
            .unwrap_or_else(|| "the run failed".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_keeps_models_only() {
        let response = json!({
            "account": {"email": "someone@example.com"},
            "models": [
                {"value": "sonnet", "resolvedModel": "claude-sonnet-5-5",
                 "displayName": "Sonnet 5.5", "description": "Efficient",
                 "supportsEffort": true, "supportedEffortLevels": ["low", "high"]},
                {"value": "claude-x"},
                {"displayName": "no value"},
            ],
        });
        let init = Init::parse(&response);
        assert_eq!(
            init.models,
            [
                ModelInfo {
                    value: "sonnet".into(),
                    resolved_model: Some("claude-sonnet-5-5".into()),
                    display_name: "Sonnet 5.5".into(),
                    description: "Efficient".into(),
                    effort_levels: vec!["low".into(), "high".into()],
                },
                ModelInfo {
                    value: "claude-x".into(),
                    resolved_model: None,
                    display_name: "claude-x".into(),
                    description: String::new(),
                    effort_levels: vec![],
                },
            ]
        );
        assert!(!format!("{init:?}").contains("someone"));
    }

    #[test]
    fn results() {
        // Recorded from 2.1.293: a missing login, an interrupt, a success.
        let auth = json!({"type": "result", "subtype": "success", "is_error": true,
            "result": "Not logged in · Please run /login", "terminal_reason": "api_error"});
        let aborted = json!({"type": "result", "subtype": "error_during_execution",
            "is_error": true, "terminal_reason": "aborted_streaming",
            "errors": ["[ede_diagnostic] result_type=user"]});
        let missing = json!({"type": "result", "subtype": "error_during_execution",
            "is_error": true, "num_turns": 0,
            "errors": ["No conversation found with session ID: x"]});
        let done = json!({"type": "result", "subtype": "success", "is_error": false,
            "result": "OK", "terminal_reason": "completed"});
        assert_eq!(
            result_error(&auth).as_deref(),
            Some("Not logged in · Please run /login")
        );
        assert!(!result_aborted(&auth));
        assert!(result_aborted(&aborted));
        assert_eq!(
            result_error(&missing).as_deref(),
            Some("No conversation found with session ID: x")
        );
        assert_eq!(result_error(&done), None);
        assert!(!result_aborted(&done));
    }
}

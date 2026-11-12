//! Complete-option previews from pi's one-shot JSON event stream.

use serde_json::Value;

const MAX_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct Stream {
    line: Vec<u8>,
    text: String,
    shown: Vec<String>,
    result: Option<Result<Vec<String>, String>>,
    settled: bool,
}

impl Stream {
    pub fn feed(
        &mut self,
        bytes: &[u8],
        count: usize,
        preview: &mut impl FnMut(Vec<String>),
    ) -> Result<(), String> {
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            if self.line.len() + part.len() > MAX_BYTES {
                return Err("pi's selection event was too large".into());
            }
            self.line.extend_from_slice(part);
            if part.last() == Some(&b'\n') {
                let event: Value = serde_json::from_slice(&self.line)
                    .map_err(|_| "pi printed an invalid selection event")?;
                self.line.clear();
                self.event(event, count, preview)?;
            }
        }
        Ok(())
    }

    fn publish(&mut self, options: Vec<String>, preview: &mut impl FnMut(Vec<String>)) {
        if self.shown != options {
            self.shown = options.clone();
            preview(options);
        }
    }

    fn reset(&mut self, preview: &mut impl FnMut(Vec<String>)) {
        self.text.clear();
        self.result = None;
        self.settled = false;
        self.publish(Vec::new(), preview);
    }

    fn event(
        &mut self,
        event: Value,
        count: usize,
        preview: &mut impl FnMut(Vec<String>),
    ) -> Result<(), String> {
        match event["type"].as_str() {
            Some("message_start") if event["message"]["role"] == "assistant" => {
                self.reset(preview);
            }
            Some("auto_retry_start") => self.reset(preview),
            Some("message_update") => {
                let delta = &event["assistantMessageEvent"];
                match delta["type"].as_str() {
                    Some("text_start") if !self.text.is_empty() => {
                        if self.text.len() + 2 > MAX_BYTES {
                            return Err("pi's selection reply was too large".into());
                        }
                        self.text.push_str("\n\n");
                    }
                    Some("text_delta") => {
                        let text = delta["delta"]
                            .as_str()
                            .ok_or("pi sent an invalid text delta")?;
                        if self.text.len() + text.len() > MAX_BYTES {
                            return Err("pi's selection reply was too large".into());
                        }
                        self.text.push_str(text);
                        self.publish(completed_options(&self.text, count), preview);
                    }
                    _ => {}
                }
            }
            Some("message_end") if event["message"]["role"] == "assistant" => {
                let message = &event["message"];
                let result = if message["stopReason"] == "stop" {
                    let text = message["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|part| part["type"] == "text")
                        .filter_map(|part| part["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    super::parse(&text, count)
                } else {
                    Err(match message["stopReason"].as_str() {
                        Some("length") => "pi's selection reply was truncated".into(),
                        Some("aborted") => "pi's selection request was cancelled".into(),
                        _ => "pi couldn't finish the selection request".into(),
                    })
                };
                self.publish(result.as_ref().cloned().unwrap_or_default(), preview);
                self.result = Some(result);
            }
            Some("agent_settled") => {
                self.settled = event["aborted"] == false;
                if !self.settled {
                    self.publish(Vec::new(), preview);
                    self.result = Some(Err("pi's selection request was cancelled".into()));
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<String>, String> {
        if !self.line.is_empty() {
            return Err("pi's selection event was incomplete".into());
        }
        if let Some(Err(error)) = &self.result {
            return Err(error.clone());
        }
        if !self.settled {
            return Err("pi didn't finish the selection request".into());
        }
        self.result
            .unwrap_or_else(|| Err("pi's reply had no alternatives".into()))
    }
}

/// Only canonical envelopes stream. Wrapped replies still use the tolerant
/// final parser. A delimiter after the string proves the whole value arrived.
fn completed_options(text: &str, count: usize) -> Vec<String> {
    let mut rest = text;
    for token in ["{", "\"alternatives\"", ":", "["] {
        let Some(tail) = rest.trim_start().strip_prefix(token) else {
            return Vec::new();
        };
        rest = tail;
    }
    let mut options = Vec::new();
    while options.len() < count {
        let mut values =
            serde_json::Deserializer::from_str(rest.trim_start()).into_iter::<String>();
        let Some(Ok(value)) = values.next() else {
            break;
        };
        rest = rest.trim_start()[values.byte_offset()..].trim_start();
        let Some(delimiter @ (b',' | b']')) = rest.as_bytes().first().copied() else {
            break;
        };
        let option = value.trim();
        if !option.is_empty() && !options.iter().any(|o| o == option) {
            options.push(option.to_string());
        }
        if delimiter == b']' {
            break;
        }
        rest = &rest[1..];
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn send(stream: &mut Stream, event: Value, updates: &mut Vec<Vec<String>>) {
        let mut bytes = serde_json::to_vec(&event).unwrap();
        bytes.push(b'\n');
        // Split inside Unicode, JSON strings, and event framing.
        for byte in bytes {
            stream
                .feed(&[byte], 3, &mut |options| updates.push(options))
                .unwrap();
        }
    }

    fn delta(text: &str) -> Value {
        json!({"type":"message_update", "assistantMessageEvent":{"type":"text_delta","delta":text}})
    }

    fn end(text: &str, reason: &str) -> Value {
        json!({"type":"message_end","message":{"role":"assistant","stopReason":reason,"content":[{"type":"text","text":text}]}})
    }

    #[test]
    fn complete_strings_only_with_escapes_unicode_and_deduplication() {
        let text =
            r#"{"alternatives":["  "," café 🙂\n\"quote\" ","café 🙂\n\"quote\"","two","three"]}"#;
        let mut stream = Stream::default();
        let mut updates = Vec::new();
        for char in text.chars() {
            send(&mut stream, delta(&char.to_string()), &mut updates);
        }
        assert_eq!(updates.iter().map(Vec::len).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(updates[2], super::super::parse(text, 3).unwrap());
        assert_eq!(completed_options(text, 1).len(), 1);
        for text in [
            r#"{"alternatives":["unfinished"#,
            r#"{"alternatives":["closed"  "#,
            r#"{"alternatives":["\u12"#,
        ] {
            assert!(completed_options(text, 3).is_empty());
        }
        send(&mut stream, end(text, "stop"), &mut updates);
        send(
            &mut stream,
            json!({"type":"agent_settled","aborted":false}),
            &mut updates,
        );
        assert_eq!(stream.finish().unwrap(), updates[2]);
    }

    #[test]
    fn retry_clears_preview_and_final_message_is_authoritative() {
        let mut stream = Stream::default();
        let mut updates = Vec::new();
        send(
            &mut stream,
            delta(r#"{"alternatives":["old","#),
            &mut updates,
        );
        send(
            &mut stream,
            json!({"type":"auto_retry_start"}),
            &mut updates,
        );
        assert!(updates[1].is_empty());
        send(
            &mut stream,
            end("```json\n{\"alternatives\":[\"final\"]}\n```", "stop"),
            &mut updates,
        );
        send(
            &mut stream,
            json!({"type":"agent_settled","aborted":false}),
            &mut updates,
        );
        assert_eq!(stream.finish().unwrap(), ["final"]);
    }

    #[test]
    fn rejects_errors_truncation_abort_and_missing_settlement() {
        for reason in ["error", "length", "aborted", "stop"] {
            let mut stream = Stream::default();
            let mut updates = Vec::new();
            send(
                &mut stream,
                delta(r#"{"alternatives":["old","#),
                &mut updates,
            );
            send(
                &mut stream,
                end(r#"{"alternatives":["new"]}"#, reason),
                &mut updates,
            );
            assert!(stream.finish().is_err());
            if reason != "stop" {
                assert!(updates.last().unwrap().is_empty());
            }
        }
        let mut stream = Stream::default();
        send(
            &mut stream,
            end(r#"{"alternatives":["new"]}"#, "stop"),
            &mut Vec::new(),
        );
        send(
            &mut stream,
            json!({"type":"agent_settled","aborted":true}),
            &mut Vec::new(),
        );
        assert!(stream.finish().is_err());
    }

    #[test]
    fn thinking_and_wrapped_output_do_not_preview_and_input_is_bounded() {
        let mut stream = Stream::default();
        let mut updates = Vec::new();
        send(
            &mut stream,
            json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"thinking"}}),
            &mut updates,
        );
        send(
            &mut stream,
            delta("```json\n{\"alternatives\":[\"one\"]}"),
            &mut updates,
        );
        assert!(updates.is_empty());
        assert!(
            stream
                .feed(&vec![b'x'; MAX_BYTES + 1], 3, &mut |_| {})
                .is_err()
        );
        assert!(
            Stream::default()
                .feed(b"not json\n", 3, &mut |_| {})
                .is_err()
        );
        let mut stream = Stream {
            text: "x".repeat(MAX_BYTES),
            ..Stream::default()
        };
        let start = json!({"type":"message_update","assistantMessageEvent":{"type":"text_start"}});
        assert!(stream.event(start, 3, &mut |_| {}).is_err());
        assert!(stream.event(delta("x"), 3, &mut |_| {}).is_err());
        let mut stream = Stream::default();
        stream.feed(b"{", 3, &mut |_| {}).unwrap();
        assert!(stream.finish().is_err());
    }
}

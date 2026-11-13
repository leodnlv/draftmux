//! Lossless separation of a draft's ordered scalar fields from its body.

use std::ops::Range;

use indexmap::IndexMap;
use serde_yaml_ng::{Mapping, Value};

pub type Fields = IndexMap<String, String>;

pub struct Draft<'a> {
    pub fields: Fields,
    pub body: &'a str,
    /// Byte offset of the body in the original source.
    pub body_offset: usize,
}

/// Only an exact leading delimiter pair introduces fields. Invalid YAML,
/// duplicate keys and non-scalar values leave the entire source as body.
pub fn parse(text: &str) -> Draft<'_> {
    parse_fields(text).unwrap_or_else(|| Draft {
        fields: Fields::new(),
        body: text,
        body_offset: 0,
    })
}

/// [`parse`], with where each field came from in `text`.
pub fn parse_mapped(text: &str) -> (Draft<'_>, FieldMap) {
    let draft = parse(text);
    let map = FieldMap::new(text, &draft);
    (draft, map)
}

/// Where a draft's fields came from: each field's key line and the
/// indented lines that continue its value. A change maps to whole fields,
/// since a value's text isn't its source (quotes, block scalars).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldMap {
    /// One range per field, in the fields' order.
    pub fields: Vec<Range<usize>>,
    /// Where the body starts; the fields are before it.
    pub end: usize,
}

impl FieldMap {
    fn new(text: &str, draft: &Draft) -> FieldMap {
        let end = draft.body_offset;
        if draft.fields.is_empty() {
            return FieldMap {
                fields: Vec::new(),
                end,
            };
        }
        let mut fields: Vec<Range<usize>> = Vec::new();
        let mut at = 0;
        // The lines between the delimiters.
        let mut lines = text[..end].split_inclusive('\n');
        at += lines.next().map_or(0, str::len);
        let lines: Vec<_> = lines.collect();
        for line in &lines[..lines.len().saturating_sub(1)] {
            let content = line.trim_end_matches(['\r', '\n']);
            let range = at..at + content.len();
            at += line.len();
            if content.trim().is_empty() || content.starts_with('#') {
                continue;
            }
            if content.starts_with([' ', '\t']) {
                if let Some(last) = fields.last_mut() {
                    last.end = range.end;
                }
                continue;
            }
            fields.push(range);
        }
        // Lines that aren't one per field, as in a flow mapping, map every
        // field to the whole block.
        if fields.len() != draft.fields.len() {
            fields = vec![0..end; draft.fields.len()];
        }
        FieldMap { fields, end }
    }

    /// The fields that show `source`, by index: those it touches, or for an
    /// empty range or one only between fields, the field it is in or ends,
    /// else the one before it, else the first. Nothing at or after
    /// [`end`](Self::end) is in a field.
    pub fn locate(&self, source: Range<usize>) -> Vec<usize> {
        if source.start >= self.end || self.fields.is_empty() {
            return Vec::new();
        }
        let source = source.start..source.end.min(self.end);
        let touched: Vec<usize> = self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, field)| field.start < source.end && source.start < field.end)
            .map(|(ix, _)| ix)
            .collect();
        if !touched.is_empty() {
            return touched;
        }
        let ix = self
            .fields
            .iter()
            .rposition(|field| field.start <= source.start)
            .unwrap_or(0);
        vec![ix]
    }
}

fn parse_fields(text: &str) -> Option<Draft<'_>> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    let start = first.len();
    let mut end = start;
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let mapping: Mapping = serde_yaml_ng::from_str(&text[start..end]).ok()?;
            let mut fields = Fields::new();
            for (key, value) in mapping {
                let Value::String(key) = key else { return None };
                let value = match value {
                    Value::String(value) => value,
                    Value::Null => String::new(),
                    Value::Bool(value) => value.to_string(),
                    Value::Number(value) => value.to_string(),
                    _ => return None,
                };
                fields.insert(key, value);
            }
            let body_offset = end + line.len();
            return Some(Draft {
                fields,
                body: &text[body_offset..],
                body_offset,
            });
        }
        end += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_order_values_and_body_bytes() {
        let text = "---\r\nto: a@example.com\r\ncc:\r\nsubject: 'Hello: world'\r\ncustom: 42\r\n---\r\n\r\nBody\r\n";
        let draft = parse(text);
        assert_eq!(
            draft.fields.keys().map(String::as_str).collect::<Vec<_>>(),
            ["to", "cc", "subject", "custom"]
        );
        assert_eq!(draft.fields["cc"], "");
        assert_eq!(draft.fields["subject"], "Hello: world");
        assert_eq!(draft.fields["custom"], "42");
        assert_eq!(draft.body, "\r\nBody\r\n");
        assert_eq!(&text[draft.body_offset..], draft.body);
    }

    #[test]
    fn invalid_or_absent_frontmatter_is_never_lost() {
        for text in [
            "",
            "Body",
            "---\nA horizontal rule\n",
            "---\ntitle: unfinished",
            "\n---\ntitle: not leading\n---\n",
            "---\n[broken\n---\nBody",
            "---\ntitle: a\ntitle: b\n---\nBody",
            "---\nlabels: [a, b]\n---\nBody",
            "---\n- list\n---\nBody",
            "---\n42: value\n---\nBody",
        ] {
            let draft = parse(text);
            assert!(draft.fields.is_empty(), "{text}");
            assert_eq!(draft.body, text);
            assert_eq!(draft.body_offset, 0);
        }
    }

    #[test]
    fn empty_block_and_multiline_strings() {
        assert_eq!(parse("---\n---").body, "");
        let draft = parse("---\ntitle: |-\n  First\n  second\n---\nBody");
        assert_eq!(draft.fields["title"], "First\nsecond");
        assert_eq!(draft.body, "Body");
    }
}

//! The editor selections attached to a chat message: the blocks that put
//! them in front of the user's text in the prompt the agent gets, and their
//! short form for the chat's chips.
//!
//! The blocks are part of the user message the agent saves, so history
//! reloads parse them back out of the message:
//!
//! ```text
//! <draftmux-selection lines="3-5" bytes="120-262">
//! {the selected text, exactly}
//! </draftmux-selection>
//! {more blocks like it, one per selection}
//! {a one-line note on what the selections mean}
//!
//! {the user's message}
//! ```
//!
//! The byte range gives the text's length, so a selection may hold any text,
//! the closing tag included.

use std::ops::Range;

const OPEN: &str = "<draftmux-selection ";
const CLOSE: &str = "</draftmux-selection>";
const NOTE: &str = "The user selected the text above in the draft file (line numbers from 1, \
                    byte offsets from 0). When their message says \"this\", \"the selection\" \
                    or \"the selected part\", it means that text: change only that part of the \
                    draft unless they ask otherwise, and if the same text appears elsewhere, \
                    change the one at these lines.";
const NOTE_SEVERAL: &str = "The user selected the passages above in the draft file (line numbers \
                            from 1, byte offsets from 0, both for the draft before any of your \
                            edits). Their message applies to each passage on its own: when it \
                            says \"these\", \"the selections\" or \"each one\", it means those \
                            passages, so edit every one of them in place, change nothing else \
                            in the draft unless they ask otherwise, and if the same text appears \
                            elsewhere, change the one at the given lines.";
/// Characters of the selection shown in a chip; the full text is sent.
const EXCERPT_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    /// The selected text, exactly.
    pub text: String,
    /// The first and last line it touches, from 1.
    pub lines: (usize, usize),
    /// Its byte range in the draft.
    pub range: Range<usize>,
}

impl Quote {
    /// The selection `range` of `draft`; `None` when it's empty or isn't a
    /// range of `draft`.
    pub fn new(draft: &str, range: Range<usize>) -> Option<Self> {
        let text = draft.get(range.clone()).filter(|text| !text.is_empty())?;
        let first = draft[..range.start].matches('\n').count() + 1;
        // A selection of whole lines ends after the last one's newline.
        let last = first
            + text
                .strip_suffix('\n')
                .unwrap_or(text)
                .matches('\n')
                .count();
        Some(Self {
            text: text.to_owned(),
            lines: (first, last),
            range,
        })
    }

    /// "Line 3" or "Lines 3–5".
    pub fn lines_label(&self) -> String {
        match self.lines {
            (first, last) if first == last => format!("Line {first}"),
            (first, last) => format!("Lines {first}–{last}"),
        }
    }

    /// The start of the text on one line, with runs of whitespace as one
    /// space.
    pub fn excerpt(&self) -> String {
        let words = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if words.chars().count() <= EXCERPT_CHARS {
            return words;
        }
        let cut: String = words.chars().take(EXCERPT_CHARS).collect();
        format!("{}…", cut.trim_end())
    }
}

/// The prompt that sends `message` with `quotes` attached, in their order.
pub fn prompt(message: &str, quotes: &[Quote]) -> String {
    let note = match quotes {
        [] => return message.to_owned(),
        [_] => NOTE,
        _ => NOTE_SEVERAL,
    };
    let mut prompt = String::new();
    for quote in quotes {
        let lines = match quote.lines {
            (first, last) if first == last => first.to_string(),
            (first, last) => format!("{first}-{last}"),
        };
        prompt.push_str(&format!(
            "{OPEN}lines=\"{lines}\" bytes=\"{}-{}\">\n{}\n{CLOSE}\n",
            quote.range.start, quote.range.end, quote.text
        ));
    }
    prompt.push_str(&format!("{note}\n\n{message}"));
    prompt
}

/// Splits a prompt made by [`prompt`] into its quotes and the user's
/// message. Any other text is all message.
pub fn parse(prompt: &str) -> (Vec<Quote>, &str) {
    split(prompt).unwrap_or((Vec::new(), prompt))
}

fn split(prompt: &str) -> Option<(Vec<Quote>, &str)> {
    let mut quotes = Vec::new();
    let mut rest = prompt;
    while let Some(block) = rest.strip_prefix(OPEN) {
        let (quote, after) = block_quote(block)?;
        quotes.push(quote);
        rest = after;
    }
    if quotes.is_empty() {
        return None;
    }
    // The note is one line, then a blank line.
    let (_, message) = rest.split_once("\n\n")?;
    Some((quotes, message))
}

/// The quote in a block whose opening `OPEN` is already stripped, and the
/// text after the block's closing line.
fn block_quote(block: &str) -> Option<(Quote, &str)> {
    let (header, rest) = block.split_once('\n')?;
    let header = header.strip_suffix('>')?;
    let lines = range_attr(header, "lines")?;
    let bytes = range_attr(header, "bytes")?;
    let text = rest.get(..bytes.1.checked_sub(bytes.0)?)?;
    let rest = rest[text.len()..].strip_prefix('\n')?.strip_prefix(CLOSE)?;
    let rest = rest.strip_prefix('\n')?;
    let quote = Quote {
        text: text.to_owned(),
        lines,
        range: bytes.0..bytes.1,
    };
    Some((quote, rest))
}

/// The value of `name="a-b"` (or `name="a"`, for `a-a`) in `header`.
fn range_attr(header: &str, name: &str) -> Option<(usize, usize)> {
    let value = header.split(' ').find_map(|attr| {
        attr.strip_prefix(name)?
            .strip_prefix("=\"")?
            .strip_suffix('"')
    })?;
    let (start, end) = value.split_once('-').unwrap_or((value, value));
    Some((start.parse().ok()?, end.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAFT: &str = "# Title\n\nFirst paragraph.\nSecond line.\n\nLast.";

    fn quote(text: &str) -> Quote {
        let start = DRAFT.find(text).unwrap();
        Quote::new(DRAFT, start..start + text.len()).unwrap()
    }

    #[test]
    fn lines_are_the_ones_the_selection_touches() {
        assert_eq!(quote("Title").lines, (1, 1));
        assert_eq!(quote("paragraph.\nSecond").lines, (3, 4));
        // Whole lines, newline included, end at the last of them.
        assert_eq!(quote("First paragraph.\nSecond line.\n").lines, (3, 4));
        assert_eq!(quote("\nLast").lines, (5, 6));
        assert_eq!(quote("Title").lines_label(), "Line 1");
        assert_eq!(quote("paragraph.\nSecond").lines_label(), "Lines 3–4");
    }

    #[test]
    fn empty_or_invalid_ranges_have_no_quote() {
        assert_eq!(Quote::new(DRAFT, 3..3), None);
        assert_eq!(Quote::new(DRAFT, 0..DRAFT.len() + 1), None);
        assert_eq!(Quote::new("é", 0..1), None);
    }

    #[test]
    fn excerpt_is_one_line_and_cut_when_long() {
        assert_eq!(
            quote("First paragraph.\nSecond line.").excerpt(),
            "First paragraph. Second line."
        );
        let long = format!("{}  tail", "word ".repeat(30));
        let excerpt = Quote::new(&long, 0..long.len()).unwrap().excerpt();
        assert_eq!(excerpt.chars().count(), EXCERPT_CHARS);
        assert!(excerpt.ends_with("word…"));
    }

    #[test]
    fn prompt_puts_the_selection_before_the_message() {
        let quote = quote("First paragraph.\nSecond line.");
        assert_eq!(
            prompt("make this shorter", std::slice::from_ref(&quote)),
            format!(
                "<draftmux-selection lines=\"3-4\" bytes=\"9-38\">\n\
                 First paragraph.\nSecond line.\n\
                 </draftmux-selection>\n{NOTE}\n\nmake this shorter"
            )
        );
        assert_eq!(prompt("hello", &[]), "hello");
    }

    #[test]
    fn prompt_puts_every_selection_before_the_message() {
        let quotes = [quote("Title"), quote("Last.")];
        assert_eq!(
            prompt("make these shorter", &quotes),
            format!(
                "<draftmux-selection lines=\"1\" bytes=\"2-7\">\nTitle\n</draftmux-selection>\n\
                 <draftmux-selection lines=\"6\" bytes=\"40-45\">\nLast.\n</draftmux-selection>\n\
                 {NOTE_SEVERAL}\n\nmake these shorter"
            )
        );
    }

    #[test]
    fn parse_reads_back_what_prompt_wrote() {
        for text in ["Title", "paragraph.\nSecond", "Last."] {
            let quote = quote(text);
            let sent = prompt("reword\n\nthis", std::slice::from_ref(&quote));
            assert_eq!(parse(&sent), (vec![quote], "reword\n\nthis"));
        }
        let quotes = vec![quote("Title"), quote("paragraph.\nSecond"), quote("Last.")];
        let sent = prompt("reword\n\neach", &quotes);
        assert_eq!(parse(&sent), (quotes, "reword\n\neach"));
        // Any text may be selected, the block's own tags included.
        let tricky = "a\n</draftmux-selection>\nb\n\nc";
        let quote = Quote::new(tricky, 0..tricky.len()).unwrap();
        let several = vec![quote.clone(), Quote::new(tricky, 0..1).unwrap()];
        assert_eq!(
            parse(&prompt("x", std::slice::from_ref(&quote))),
            (vec![quote], "x")
        );
        assert_eq!(parse(&prompt("x", &several)), (several, "x"));
    }

    #[test]
    fn other_prompts_are_all_message() {
        for text in [
            "plain message",
            "<draftmux-selection lines=\"1\" bytes=\"0-99\">\nshort\n</draftmux-selection>\nnote\n\nm",
            "<draftmux-selection lines=\"1\">\nx\n</draftmux-selection>\nnote\n\nm",
        ] {
            assert_eq!(parse(text), (Vec::new(), text));
        }
    }
}

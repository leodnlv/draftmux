//! Compares two versions of a draft for review: the diff
//! ([`draftmux_core::diff`]), and the redline text the preview renders, with
//! each insertion and deletion marked inline by sentinels that pass through
//! the markdown parsers as ordinary text. The sentinels are Unicode
//! noncharacters, which, unlike the private use area, no font gives a glyph;
//! [`neutralize`] keeps a draft from supplying them.

use std::borrow::Cow;
use std::ops::RangeInclusive;

#[cfg(any(test, feature = "capture"))]
pub use draftmux_core::diff::compare;
pub use draftmux_core::diff::{Comparison, Hunk, Kind};

/// Opens inserted text; the hunk's [`tag`] follows.
const INS_OPEN: char = '\u{FDD0}';
const INS_CLOSE: char = '\u{FDD1}';
/// Opens deleted text, like [`INS_OPEN`].
const DEL_OPEN: char = '\u{FDD2}';
const DEL_CLOSE: char = '\u{FDD3}';
/// Where one version has text the other lacks, shown in the other, marked
/// as that change; [`split_marks`] reads it as a [`Run::Point`].
const POINT_MARK: char = '\u{FDD4}';
/// What a point is drawn on: a hair space, shown as a bar.
pub const POINT: &str = "\u{200A}";
/// A tag's hexadecimal digits, from 0 at `DIGIT_BASE`. Every byte of a
/// draft could be its own hunk at most, so 8 digits are enough.
const DIGIT_BASE: u32 = 0xFDE0;
const TAG_DIGITS: usize = 8;
/// The noncharacters the sentinels and digits are taken from.
const RESERVED: RangeInclusive<char> = '\u{FDD0}'..='\u{FDEF}';

/// Which version [`marked`] shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The old text with its deletions marked, and a [`POINT`] where text
    /// was only added.
    Before,
    /// The new text with its insertions marked, and a [`POINT`] where text
    /// was only removed.
    After,
}

/// One version of the comparison with each change marked for
/// [`split_marks`]. Marks open after any list, heading or quote marker so
/// the block still parses, and close before the end of each line; fences,
/// rules, table separators and blank lines stay bare. The changes in
/// `unseen`, whose text this side doesn't show, such as a heading's level
/// or a frontmatter value, get a point instead.
pub fn marked(comparison: &Comparison, side: Side, unseen: &[usize]) -> String {
    let insert = (INS_OPEN, INS_CLOSE);
    let delete = (DEL_OPEN, DEL_CLOSE);
    // A point is marked as the change it stands in for.
    let (shown, hidden, marks, other_marks) = match side {
        Side::Before => (Kind::Delete, Kind::Insert, delete, insert),
        Side::After => (Kind::Insert, Kind::Delete, insert, delete),
    };
    // A change hidden on this side leaves a point only when nothing of it
    // shows here; a replacement shows its other text instead.
    let points = |hunk: &Hunk| match side {
        Side::Before => hunk.is_addition(),
        Side::After => hunk.is_removal(),
    };
    let text: String = comparison
        .ops
        .iter()
        .filter(|op| op.kind == Kind::Equal || op.kind == shown)
        .map(|op| op.text.as_str())
        .collect();
    // The whole side, so a character reference split between ops is still
    // found; it keeps every op's offsets.
    let text = neutralize(&text);
    let mut at = 0;
    let mut out = String::new();
    // Points wait here for a line that can hold them.
    let mut pending: Vec<(usize, (char, char))> = Vec::new();
    for op in &comparison.ops {
        let hunk = op.hunk.unwrap_or_default();
        let mut queue = None;
        if op.kind == Kind::Equal || op.kind == shown {
            let op_text = &text[at..at + op.text.len()];
            let start = flush(&mut out, &mut pending, op_text);
            let rest = &op_text[start..];
            let full = &text[at + start..];
            match op.kind {
                Kind::Equal => out.push_str(rest),
                _ if unseen.contains(&hunk) => {
                    out.push_str(rest);
                    queue = Some((hunk, marks));
                }
                _ => wrap(&mut out, rest, full, marks, hunk),
            }
            at += op.text.len();
        } else if op.kind == hidden && comparison.hunks.get(hunk).is_some_and(points) {
            queue = Some((hunk, other_marks));
        }
        // At a line's end the point goes on that line; within a line it
        // waits for the rest, which says whether it is still inside the
        // line's block marker.
        if let Some((hunk, marks)) = queue {
            if pending.is_empty() && out.ends_with('\n') && holds_point(&out) {
                point(&mut out, hunk, marks);
            } else {
                pending.push((hunk, marks));
            }
        }
    }
    for (hunk, marks) in pending {
        if holds_point(&out) {
            point(&mut out, hunk, marks);
        } else {
            // Nothing before can hold it, so it stands on its own.
            if !out.is_empty() {
                out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
            }
            push_point(&mut out, hunk, marks);
            out.push('\n');
        }
    }
    out
}

/// Writes the points waiting in `pending` into the first line of `text`
/// that can hold one, after its block marker, and returns how much of
/// `text` went before them; 0 when no line can.
fn flush(out: &mut String, pending: &mut Vec<(usize, (char, char))>, text: &str) -> usize {
    if pending.is_empty() {
        return 0;
    }
    let mut frontmatter = in_frontmatter(out);
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        // Only a line that starts here, or continues one that can hold a
        // point, takes one.
        let start = at == 0 && !out.is_empty() && !out.ends_with('\n');
        let full = match start {
            true => format!("{}{body}", current_line(out)),
            false => body.to_owned(),
        };
        let skip = frontmatter || full.trim().is_empty() || is_raw_line(&full) || alert_line(&full);
        if frontmatter && full == "---" {
            frontmatter = false;
        }
        if !skip {
            let prefix = block_prefix(&full).saturating_sub(full.len() - body.len());
            let point_at = at + prefix.min(body.len());
            out.push_str(&text[..point_at]);
            for (hunk, marks) in pending.drain(..) {
                push_point(out, hunk, marks);
            }
            return point_at;
        }
        at += line.len();
    }
    0
}

/// Whether a point can go at the end of `out`'s last line with text: not
/// past a blank line, where it would open a block of its own, nor on a
/// fence, rule, alert marker or frontmatter line, which it would break.
fn holds_point(out: &str) -> bool {
    if out.is_empty() || out.ends_with("\n\n") {
        return false;
    }
    let body = out.trim_end_matches('\n');
    let line = current_line(body);
    !(line.trim().is_empty()
        || is_raw_line(line)
        || alert_line(line)
        || in_frontmatter(&body[..body.len() - line.len()]))
}

/// Marks where the other side's text is, at the end of the last line with
/// text, inside a table row's last cell.
fn point(out: &mut String, hunk: usize, marks: (char, char)) {
    let body = out.trim_end_matches('\n').len();
    let newlines = out.split_off(body);
    let line = current_line(out);
    let line_start = out.len() - line.len();
    let cell = match line.trim_start().starts_with('|') {
        true => line
            .trim_end()
            .strip_suffix('|')
            .filter(|cell| !cell.ends_with('\\'))
            .map(|cell| cell.trim_end().len()),
        false => None,
    };
    let tail = cell.map(|cell| out.split_off(line_start + cell));
    push_point(out, hunk, marks);
    out.push_str(tail.as_deref().unwrap_or_default());
    out.push_str(&newlines);
}

fn push_point(out: &mut String, hunk: usize, (open, close): (char, char)) {
    out.push(open);
    out.push_str(&tag(hunk));
    out.push(POINT_MARK);
    out.push(close);
}

/// The text after `out`'s last newline.
fn current_line(out: &str) -> &str {
    out.rsplit_once('\n').map_or(out, |(_, line)| line)
}

/// Whether text written after `out` is inside its frontmatter.
fn in_frontmatter(out: &str) -> bool {
    out.starts_with("---\n") && !out.get(3..).is_some_and(|rest| rest.contains("\n---\n"))
}

/// A quote line holding only an alert's `[!KIND]`, which a mark or point
/// would turn into text.
fn alert_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('>') && line.contains("[!") && block_prefix(line) >= line.trim_end().len()
}

/// Whether `block` shows any of hunk `hunk`'s marks.
pub fn block_has_hunk(block: &markup::Block, hunk: usize) -> bool {
    let mut found = false;
    opened_hunks(block, &mut |opened| found |= opened == hunk);
    found
}

/// Which of the first `count` hunks `blocks` show any marks of, the same
/// as [`block_has_hunk`] for each hunk but in one pass.
pub fn shown_hunks(blocks: &[markup::Block], count: usize) -> Vec<bool> {
    let mut shown = vec![false; count];
    for block in blocks {
        opened_hunks(block, &mut |hunk| {
            if let Some(shown) = shown.get_mut(hunk) {
                *shown = true;
            }
        });
    }
    shown
}

/// Calls `found` with the hunk of each insertion or deletion mark opened
/// in `block`'s text.
fn opened_hunks(block: &markup::Block, found: &mut dyn FnMut(usize)) {
    use markup::Block;
    match block {
        Block::Paragraph(spans) | Block::Heading { spans, .. } => {
            spans.iter().for_each(|span| opened_in(&span.text, found));
        }
        Block::List(list) => list
            .items
            .iter()
            .flat_map(|item| &item.blocks)
            .for_each(|block| opened_hunks(block, found)),
        Block::Quote(inner) | Block::Alert { blocks: inner, .. } => {
            inner.iter().for_each(|block| opened_hunks(block, found));
        }
        Block::Code { text, .. } | Block::Html(text) => opened_in(text, found),
        Block::Table(table) => table
            .header
            .iter()
            .chain(table.rows.iter().flatten())
            .flatten()
            .for_each(|span| opened_in(&span.text, found)),
        Block::Rule => {}
    }
}

/// [`opened_hunks`] for one run of text.
fn opened_in(text: &str, found: &mut dyn FnMut(usize)) {
    // Tags follow only openers, so an opener and a whole tag can't match
    // anywhere else.
    for (at, open) in text.match_indices([INS_OPEN, DEL_OPEN]) {
        if let Some(hunk) = read_tag(&text[at + open.len()..]) {
            found(hunk);
        }
    }
}

/// Hunk `hunk`'s number in [`TAG_DIGITS`] digits.
fn tag(hunk: usize) -> String {
    let hunk = u32::try_from(hunk).expect("a draft has fewer hunks than u32::MAX");
    (0..TAG_DIGITS)
        .rev()
        .map(|place| digit((hunk >> (place * 4)) & 0xF))
        .collect()
}

fn digit(value: u32) -> char {
    char::from_u32(DIGIT_BASE + value).expect("noncharacters are chars")
}

/// The hunk whose tag starts `text`.
fn read_tag(text: &str) -> Option<usize> {
    let mut chars = text.chars();
    let mut hunk: usize = 0;
    for _ in 0..TAG_DIGITS {
        let value = (chars.next()? as u32).checked_sub(DIGIT_BASE)?;
        if value > 0xF {
            return None;
        }
        hunk = hunk << 4 | value as usize;
    }
    Some(hunk)
}

/// `text` with every character a parser could read as a [`RESERVED`] one
/// replaced by U+FFFD: the characters themselves, and the digits of numeric
/// character references to them, which become FFFD's padded to their
/// width. Every byte offset stays the same.
pub fn neutralize(text: &str) -> Cow<'_, str> {
    if !text.contains(|c| RESERVED.contains(&c)) && !text.contains("&#") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if RESERVED.contains(&c) {
            out.push(char::REPLACEMENT_CHARACTER);
            rest = &rest[c.len_utf8()..];
        } else if let Some(len) = reserved_reference(rest) {
            let (hex, digits) = match rest.as_bytes()[2] {
                b'x' | b'X' => (true, &rest[3..len - 1]),
                _ => (false, &rest[2..len - 1]),
            };
            let replacement = if hex { "FFFD" } else { "65533" };
            out.push_str(&rest[..len - 1 - digits.len()]);
            out.push_str(&format!("{replacement:0>width$}", width = digits.len()));
            out.push(';');
            rest = &rest[len..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    debug_assert_eq!(out.len(), text.len());
    Cow::Owned(out)
}

/// The length of the numeric character reference that starts `text`, when
/// it decodes to a [`RESERVED`] character. Like CommonMark, takes up to 6
/// hexadecimal or 7 decimal digits.
fn reserved_reference(text: &str) -> Option<usize> {
    let rest = text.strip_prefix("&#")?;
    let (digits, radix, max) = match rest.strip_prefix(['x', 'X']) {
        Some(digits) => (digits, 16, 6),
        None => (rest, 10, 7),
    };
    let len = digits
        .find(|c: char| !c.is_digit(radix))
        .unwrap_or(digits.len());
    if !(1..=max).contains(&len) || !digits[len..].starts_with(';') {
        return None;
    }
    let value = u32::from_str_radix(&digits[..len], radix).ok()?;
    char::from_u32(value)
        .filter(|c| RESERVED.contains(c))
        .map(|_| text.len() - digits.len() + len + 1)
}

fn wrap(out: &mut String, text: &str, full: &str, (open, close): (char, char), hunk: usize) {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let newline = &line[body.len()..];
        // The whole line, from what is already written to the end of the
        // version's text, decides where a mark can open.
        let before = current_line(out);
        let after = full[at..].split('\n').next().unwrap_or_default();
        let whole = format!("{before}{after}");
        let reaches_end = body.len() == after.len();
        at += line.len();
        let marked_before = before.contains(is_sentinel);
        if body.trim().is_empty() || (!marked_before && (is_raw_line(&whole) || alert_line(&whole)))
        {
            out.push_str(line);
            continue;
        }
        let line_prefix = match marked_before {
            true => 0,
            false if in_frontmatter(&out[..out.len() - before.len()]) && whole != "---" => {
                frontmatter_key(&whole)
            }
            false => block_prefix(&whole),
        };
        let prefix = line_prefix.saturating_sub(before.len()).min(body.len());
        let rest = &body[prefix..];
        let core = rest.trim_end();
        // A table row's mark stays inside its outer pipes.
        let core = match reaches_end && whole.trim_start().starts_with('|') {
            true => core
                .strip_suffix('|')
                .filter(|core| !core.ends_with('\\'))
                .map_or(core, str::trim_end),
            false => core,
        };
        if core.is_empty() {
            out.push_str(line);
            continue;
        }
        out.push_str(&body[..prefix]);
        out.push(open);
        out.push_str(&tag(hunk));
        out.push_str(core);
        out.push(close);
        out.push_str(&rest[core.len()..]);
        out.push_str(newline);
    }
}

/// The length of a frontmatter line's `key: `.
fn frontmatter_key(line: &str) -> usize {
    let mut chars = line.char_indices();
    if !chars
        .next()
        .is_some_and(|(_, c)| c.is_ascii_alphabetic() || c == '_')
    {
        return 0;
    }
    for (at, c) in chars {
        if c == ':' {
            let after = &line[at + 1..];
            return at + 1 + (after.len() - after.trim_start().len());
        }
        if !(c.is_alphanumeric() || c == '_' || c == '-') {
            return 0;
        }
    }
    0
}

/// The length of the list, task, heading and quote markers that start
/// `line`, nested in any order, or of a table row's leading pipe.
fn block_prefix(line: &str) -> usize {
    let mut at = 0;
    if let Some(len) = row_marker(line) {
        return len;
    }
    while let Some(len) = list_marker(&line[at..])
        .or_else(|| heading_marker(&line[at..]))
        .or_else(|| quote_marker(&line[at..]))
    {
        at += len;
    }
    at
}

fn leading_space(text: &str, max: usize) -> usize {
    text.char_indices()
        .take(max)
        .take_while(|(_, c)| c.is_whitespace())
        .map(|(at, c)| at + c.len_utf8())
        .last()
        .unwrap_or(0)
}

/// A table row's leading pipe.
fn row_marker(text: &str) -> Option<usize> {
    let at = leading_space(text, usize::MAX);
    text[at..]
        .starts_with('|')
        .then(|| at + 1 + leading_space(&text[at + 1..], usize::MAX))
}

/// `- `, `1. ` or `1) `, with a task box after it.
fn list_marker(text: &str) -> Option<usize> {
    let mut at = leading_space(text, usize::MAX);
    let rest = &text[at..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if rest.starts_with(['-', '*', '+']) {
        at += 1;
    } else if (1..=9).contains(&digits) && rest[digits..].starts_with(['.', ')']) {
        at += digits + 1;
    } else {
        return None;
    }
    let space = leading_space(&text[at..], usize::MAX);
    if space == 0 {
        return None;
    }
    at += space;
    let rest = &text[at..];
    if ["[ ]", "[x]", "[X]"]
        .iter()
        .any(|task| rest.starts_with(task))
    {
        let space = leading_space(&rest[3..], usize::MAX);
        if space > 0 {
            at += 3 + space;
        }
    }
    Some(at)
}

/// `## `.
fn heading_marker(text: &str) -> Option<usize> {
    let at = leading_space(text, 3);
    let hashes = text[at..].bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let space = leading_space(&text[at + hashes..], usize::MAX);
    (space > 0).then_some(at + hashes + space)
}

/// `> `, with an alert's `[!NOTE]` after it.
fn quote_marker(text: &str) -> Option<usize> {
    let mut at = leading_space(text, 3);
    if !text[at..].starts_with('>') {
        return None;
    }
    at += 1;
    if text[at..].starts_with(char::is_whitespace) {
        at += leading_space(&text[at..], 1);
    }
    let rest = &text[at..];
    if let Some(inner) = rest.strip_prefix("[!")
        && let Some(end) = inner.find(']')
        && end > 0
        && inner[..end]
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_')
    {
        at += 2 + end + 1;
        at += leading_space(&text[at..], usize::MAX);
    }
    Some(at)
}

/// A fence, a thematic break or a table's delimiter row, which a mark would
/// turn into text.
fn is_raw_line(line: &str) -> bool {
    let text = line.trim_start();
    if text.starts_with("```") || text.starts_with("~~~") {
        return true;
    }
    if let Some(c) = text.chars().next().filter(|c| matches!(c, '-' | '*' | '_'))
        && text.chars().all(|x| x == c || x.is_whitespace())
        && text.chars().filter(|&x| x == c).count() >= 3
    {
        return true;
    }
    let row = text.strip_prefix('|').unwrap_or(text).trim_start();
    let row = row.strip_prefix(':').unwrap_or(row);
    let dashes = row.bytes().take_while(|&b| b == b'-').count();
    if dashes == 0 {
        return false;
    }
    let row = &row[dashes..];
    let row = row.strip_prefix(':').unwrap_or(row).trim_start();
    row.starts_with('|')
}

/// Marked text in a redline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub hunk: usize,
    pub deleted: bool,
}

/// A piece of a parsed redline's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run<'a> {
    Text(&'a str),
    /// The place of a change whose text only the other side has.
    Point,
}

/// Splits `text` from a parsed redline into runs with their marks, dropping
/// the sentinels. `open` carries a mark across calls for text split into
/// several spans. An opener without a whole tag opens no mark.
pub fn split_marks<'a>(text: &'a str, open: &mut Option<Mark>) -> Vec<(Option<Mark>, Run<'a>)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        if !RESERVED.contains(&c) {
            continue;
        }
        if start < at {
            out.push((*open, Run::Text(&text[start..at])));
        }
        start = at + c.len_utf8();
        match c {
            INS_OPEN | DEL_OPEN => {
                *open = read_tag(&text[start..]).map(|hunk| {
                    for _ in 0..TAG_DIGITS {
                        chars.next();
                    }
                    start += TAG_DIGITS * digit(0).len_utf8();
                    Mark {
                        hunk,
                        deleted: c == DEL_OPEN,
                    }
                });
            }
            INS_CLOSE | DEL_CLOSE => *open = None,
            POINT_MARK => out.push((*open, Run::Point)),
            // A digit outside a tag.
            _ => {}
        }
    }
    if start < text.len() {
        out.push((*open, Run::Text(&text[start..])));
    }
    out
}

/// `text` without redline sentinels and points.
pub fn strip_marks(text: &str) -> String {
    text.chars().filter(|&c| !is_sentinel(c)).collect()
}

fn is_sentinel(c: char) -> bool {
    RESERVED.contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readable_side(old: &str, new: &str, side: Side) -> String {
        let marked = marked(&compare(old, new), side, &[]);
        let mut open = None;
        let mut out = String::new();
        let mut last: Option<Mark> = None;
        for (mark, run) in split_marks(&marked, &mut open) {
            let text = match run {
                Run::Text(text) => text,
                Run::Point => "|",
            };
            if let Some(last) = last.filter(|_| mark != last) {
                out.push_str(if last.deleted { "-]" } else { "+}" });
            }
            if let Some(mark) = mark.filter(|_| mark != last) {
                out.push_str(if mark.deleted { "[-" } else { "{+" });
            }
            out.push_str(text);
            last = mark;
        }
        if let Some(last) = last {
            out.push_str(if last.deleted { "-]" } else { "+}" });
        }
        out
    }

    #[test]
    fn sides_show_one_version_each() {
        let (old, new) = ("Ship it on Monday.\n", "Ship it on Friday.\n");
        assert_eq!(
            readable_side(old, new, Side::Before),
            "Ship it on [-Monday.-]\n"
        );
        assert_eq!(
            readable_side(old, new, Side::After),
            "Ship it on {+Friday.+}\n"
        );
    }

    #[test]
    fn after_side_points_to_removed_text() {
        assert_eq!(
            readable_side("Keep this short one.\n", "Keep this one.\n", Side::After),
            "Keep this [-|-]one.\n"
        );
        // Removed lines leave the point at the end of the line before.
        assert_eq!(
            readable_side("- one\n- two\n- three\n", "- one\n- three\n", Side::After),
            "- one[-|-]\n- three\n"
        );
    }

    #[test]
    fn a_point_after_a_blank_line_opens_the_next_block() {
        let old = "# Done\n\nCopy the message out.\n";
        let new = "# Done\n\nSend your finished text.\n";
        assert_eq!(
            readable_side(old, new, Side::After),
            "# Done\n\n[-|-]{+Send your finished text.+}\n"
        );
        assert_eq!(
            readable_side(old, new, Side::Before),
            "# Done\n\n[-Copy the message out.-]{+|+}\n"
        );
        // A removed last paragraph points from the end of the text before.
        assert_eq!(
            readable_side("Keep.\n\nDrop this.\n", "Keep.\n", Side::After),
            "Keep.[-|-]\n"
        );
    }

    #[test]
    fn before_side_points_to_added_text() {
        assert_eq!(
            readable_side("Keep this one.\n", "Keep this short one.\n", Side::Before),
            "Keep this {+|+}one.\n"
        );
        // A replacement shows its old text instead.
        assert_eq!(
            readable_side("Keep this one.\n", "Keep that one.\n", Side::Before),
            "Keep [-this-] one.\n"
        );
    }

    #[test]
    fn a_point_at_the_start_follows_the_block_marker() {
        assert_eq!(
            readable_side("# Draft\n\n# Title\n", "# Title\n", Side::After),
            "# [-|-]Title\n"
        );
    }

    #[test]
    fn finds_the_blocks_a_hunk_is_in() {
        let comparison = compare("# Title\n\nOld body.\n", "# Title\n\nNew body.\n");
        let doc = markup::github::parse(&marked(&comparison, Side::After, &[]));
        let found: Vec<bool> = doc
            .blocks
            .iter()
            .map(|block| block_has_hunk(block, 0))
            .collect();
        assert_eq!(found, [false, true]);
        assert_eq!(shown_hunks(&doc.blocks, 2), [true, false]);
    }

    /// Both sides, as `before / after`.
    fn readable(old: &str, new: &str) -> String {
        format!(
            "{}/ {}",
            readable_side(old, new, Side::Before),
            readable_side(old, new, Side::After)
        )
    }

    #[test]
    fn marks_open_after_block_markers() {
        assert_eq!(
            readable("- one\n", "- one\n- two\n"),
            "- one{+|+}\n/ - one\n- {+two+}\n"
        );
        assert_eq!(
            readable("# Title\n", "## Better title\n"),
            "# [-Title-]\n/ ## {+Better title+}\n"
        );
        assert_eq!(
            readable("> [!NOTE]\n> Old\n", "> [!NOTE]\n> New\n"),
            "> [!NOTE]\n> [-Old-]\n/ > [!NOTE]\n> {+New+}\n"
        );
        assert_eq!(
            readable("1. [ ] task\n", "1. [x] done\n"),
            "1. [ ] [-task-]{+|+}\n/ 1. [x] [-|-]{+done+}\n"
        );
    }

    #[test]
    fn leaves_fences_rules_and_blank_lines_bare() {
        assert_eq!(
            readable_side("Intro\n", "Intro\n\n```\ncode\n```\n\n---\n", Side::After),
            "Intro\n\n```\n{+code+}\n```\n\n---\n"
        );
        assert_eq!(
            readable_side("| a |\n", "| a |\n|---|\n| b |\n", Side::After),
            "| a |\n|---|\n| {+b+} |\n"
        );
    }

    #[test]
    fn keeps_frontmatter_keys() {
        assert_eq!(
            readable(
                "---\ntitle: Old\n---\nBody\n",
                "---\ntitle: New\n---\nBody\n"
            ),
            "---\ntitle: [-Old-]\n---\nBody\n/ ---\ntitle: {+New+}\n---\nBody\n"
        );
    }

    #[test]
    fn split_marks_carries_the_open_mark_across_calls() {
        let marked = marked(&compare("a b\n", "a cd\n"), Side::After, &[]);
        let cut = marked.find('d').unwrap();
        let mut open = None;
        let first = split_marks(&marked[..cut], &mut open);
        let second = split_marks(&marked[cut..], &mut open);
        let mark = Some(Mark {
            hunk: 0,
            deleted: false,
        });
        assert_eq!(first.last().unwrap(), &(mark, Run::Text("c")));
        assert_eq!(second.first().unwrap(), &(mark, Run::Text("d")));
        assert_eq!(strip_marks(&marked), "a cd\n");
    }

    /// Each side's text, with its marks taken out by [`strip_marks`] and by
    /// [`split_marks`].
    fn unmarked_sides(old: &str, new: &str) -> [(String, String); 2] {
        let comparison = compare(old, new);
        [Side::Before, Side::After].map(|side| {
            let marked = marked(&comparison, side, &[]);
            let runs = split_marks(&marked, &mut None)
                .into_iter()
                .filter_map(|(_, run)| match run {
                    Run::Text(text) => Some(text),
                    Run::Point => None,
                })
                .collect();
            (strip_marks(&marked), runs)
        })
    }

    #[test]
    fn keeps_private_use_characters() {
        let cases = [
            (
                "```\n\u{F113} main\n```\n",
                "```\n\u{F113} main branch\n```\n",
            ),
            ("The \u{F8FF} logo\n", "The \u{F8FF} new logo\n"),
            ("\u{E100} one\n", "\u{E100} two\n"),
            ("\u{F0001} icon\n", "\u{F0001} icons\n"),
            ("a \u{E000} b \u{E001}\n", "a \u{E000} c \u{E001}\n"),
            ("a\u{200A}b c\n", "a\u{200A}b d\n"),
        ];
        for (old, new) in cases {
            let [(before, before_runs), (after, after_runs)] = unmarked_sides(old, new);
            assert_eq!((before.as_str(), before_runs.as_str()), (old, old));
            assert_eq!((after.as_str(), after_runs.as_str()), (new, new));
        }
    }

    #[test]
    fn a_hair_space_in_a_change_is_text_not_a_point() {
        // The parser splits the hair space into a span of its own.
        let text = marked(&compare("a\n", "a **x**\u{200A}*y*\n"), Side::After, &[]);
        let cut = text.find('\u{200A}').unwrap();
        let mut open = None;
        split_marks(&text[..cut], &mut open);
        let space = split_marks(&text[cut..cut + POINT.len()], &mut open);
        assert_eq!(space, [(open, Run::Text(POINT))]);
        assert!(open.is_some());
        // A removal shows a point on the side without its text.
        let text = marked(&compare("a b c\n", "a c\n"), Side::After, &[]);
        let runs = split_marks(&text, &mut None);
        assert!(
            runs.iter()
                .any(|(mark, run)| mark.is_some() && *run == Run::Point)
        );
    }

    #[test]
    fn neutralizes_reserved_characters_and_references_to_them() {
        let cases = [
            ("a \u{FDD0} b \u{FDEF}", "a \u{FFFD} b \u{FFFD}"),
            ("&#xFDD0; &#XfdD4; &#64976;", "&#xFFFD; &#XFFFD; &#65533;"),
            ("&#x00FDD0; &#0064976;", "&#x00FFFD; &#0065533;"),
            // Not references CommonMark decodes, or not to reserved ones.
            (
                "&#x0000FDD0; &#xFDD0 &#xE001; &#xF8FF; &#;",
                "&#x0000FDD0; &#xFDD0 &#xE001; &#xF8FF; &#;",
            ),
            ("\u{F113} \u{F8FF} \u{200A}", "\u{F113} \u{F8FF} \u{200A}"),
        ];
        for (text, neutral) in cases {
            assert_eq!(neutralize(text), neutral);
            assert_eq!(neutralize(text).len(), text.len());
        }
        assert!(matches!(neutralize("plain \u{F8FF}"), Cow::Borrowed(_)));
    }

    #[test]
    fn reserved_characters_in_a_draft_open_no_marks() {
        let old = "One \u{FDD0}\u{FDE0} two &#xFDD1; three\n";
        let new = "One \u{FDD0}\u{FDE0} two &#xFDD1; four\n";
        let [(before, _), (after, _)] = unmarked_sides(old, new);
        assert_eq!(before, neutralize(old));
        assert_eq!(after, neutralize(new));
        let text = marked(&compare(old, new), Side::After, &[]);
        let marks: Vec<_> = split_marks(&text, &mut None)
            .into_iter()
            .filter_map(|(mark, _)| mark)
            .collect();
        assert!(!marks.is_empty());
        assert!(marks.iter().all(|mark| mark.hunk == 0));
    }

    #[test]
    fn a_reference_split_between_ops_is_neutralized() {
        let old = "x &#xFDD0; y\n";
        let new = "x &#xFDD1; y\n";
        let [(before, _), (after, _)] = unmarked_sides(old, new);
        assert_eq!(before, neutralize(old));
        assert_eq!(after, neutralize(new));
    }

    #[test]
    fn tags_tell_every_hunk_apart() {
        for hunk in [0, 1, 0xF8FF, 70_000, u32::MAX as usize] {
            assert_eq!(read_tag(&tag(hunk)), Some(hunk));
            assert_eq!(tag(hunk).chars().count(), TAG_DIGITS);
        }
        let mut open = None;
        let text = format!("{INS_OPEN}{}x{INS_CLOSE}", tag(70_000));
        assert_eq!(
            split_marks(&text, &mut open),
            [(
                Some(Mark {
                    hunk: 70_000,
                    deleted: false
                }),
                Run::Text("x")
            )]
        );
    }

    #[test]
    fn an_opener_without_a_whole_tag_opens_no_mark() {
        let short: String = tag(5).chars().skip(1).collect();
        let text = format!("{DEL_OPEN}{short}x");
        assert_eq!(split_marks(&text, &mut None), [(None, Run::Text("x"))]);
    }

    #[test]
    fn block_prefixes() {
        assert_eq!(block_prefix("- [x] done"), 6);
        assert_eq!(block_prefix("> - quoted item"), 4);
        assert_eq!(block_prefix("12) item"), 4);
        assert_eq!(block_prefix("-not a list"), 0);
        assert_eq!(block_prefix("####### seven"), 0);
        assert_eq!(frontmatter_key("title: Hi"), 7);
        assert!(is_raw_line("* * *"));
        assert!(is_raw_line("| :-- | --: |"));
        assert!(!is_raw_line("- item"));
    }
}

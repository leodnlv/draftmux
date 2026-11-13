//! Compares two versions of a draft for review: a line-then-word diff
//! grouped into hunks, and the redline text the preview renders, with each
//! insertion and deletion marked inline by private-use sentinels that pass
//! through the markdown parsers as ordinary text.

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffOp, capture_diff_deadline};

/// How long each diff may run before it settles for a coarser answer.
const DIFF_TIMEOUT: Duration = Duration::from_millis(50);

/// Opens inserted text; the next character carries the hunk, see [`tag`].
const INS_OPEN: char = '\u{E000}';
const INS_CLOSE: char = '\u{E001}';
/// Opens deleted text, like [`INS_OPEN`].
const DEL_OPEN: char = '\u{E002}';
const DEL_CLOSE: char = '\u{E003}';
/// Where one version has text the other lacks, shown in the other: a hair
/// space marked as that change, drawn as a bar.
pub const POINT: &str = "\u{200A}";
/// The hunk tags take the rest of the private use area.
const TAG_BASE: u32 = 0xE100;
const TAG_LAST: u32 = 0xF8FF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Equal,
    Delete,
    Insert,
}

/// A run of text the two versions share, or that only one of them has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub kind: Kind,
    pub text: String,
    /// The hunk a deletion or insertion belongs to.
    pub hunk: Option<usize>,
}

/// One change: neighbouring deletions and insertions, with where they sit
/// in each version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Byte range in the old text.
    pub old: Range<usize>,
    /// Byte range in the new text.
    pub new: Range<usize>,
    pub removed: String,
    pub added: String,
}

impl Hunk {
    /// Names the change by its text, so it is recognised again whichever
    /// versions it is compared across.
    pub fn key(&self) -> String {
        format!("{}\0{}", self.removed, self.added)
    }

    /// Whether it only removes text.
    pub fn is_removal(&self) -> bool {
        self.added.trim().is_empty()
    }

    /// Whether it only adds text.
    pub fn is_addition(&self) -> bool {
        self.removed.trim().is_empty() && !self.added.trim().is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comparison {
    pub ops: Vec<Op>,
    pub hunks: Vec<Hunk>,
}

/// Compares `old` with `new`: lines first, then words within replaced
/// lines, grouped into hunks with their offsets in both texts.
pub fn compare(old: &str, new: &str) -> Comparison {
    let mut ops = diff_ops(old, new);
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut at_old, mut at_new) = (0, 0);
    for k in 0..ops.len() {
        let len = ops[k].text.len();
        if ops[k].kind == Kind::Equal {
            at_old += len;
            at_new += len;
            continue;
        }
        let joins = k > 0 && ops[k - 1].kind != Kind::Equal;
        if !joins {
            hunks.push(Hunk {
                old: at_old..at_old,
                new: at_new..at_new,
                removed: String::new(),
                added: String::new(),
            });
        }
        let index = hunks.len() - 1;
        let hunk = &mut hunks[index];
        ops[k].hunk = Some(index);
        if ops[k].kind == Kind::Delete {
            hunk.removed.push_str(&ops[k].text);
            at_old += len;
            hunk.old.end = at_old;
        } else {
            hunk.added.push_str(&ops[k].text);
            at_new += len;
            hunk.new.end = at_new;
        }
    }
    Comparison { ops, hunks }
}

/// The edit script from `old` to `new`, with each run of changes collapsed
/// into one deletion followed by one insertion. Unchanged stretches of at
/// most two words inside a line are folded into the surrounding change, so a
/// rewritten clause reads as one replacement instead of confetti.
fn diff_ops(old: &str, new: &str) -> Vec<Op> {
    let deadline = Instant::now() + DIFF_TIMEOUT;
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let lines = seq_ops(&old_lines, &new_lines, deadline);
    let mut ops: Vec<(Kind, String)> = Vec::new();
    let mut k = 0;
    while k < lines.len() {
        if lines[k].0 == Kind::Equal {
            ops.push((Kind::Equal, lines[k].1.to_owned()));
            k += 1;
            continue;
        }
        let (mut removed, mut added) = (String::new(), String::new());
        while k < lines.len() && lines[k].0 != Kind::Equal {
            match lines[k].0 {
                Kind::Delete => removed.push_str(lines[k].1),
                _ => added.push_str(lines[k].1),
            }
            k += 1;
        }
        if !removed.is_empty() && !added.is_empty() && related(&removed, &added, deadline) {
            let words = seq_ops(&words(&removed), &words(&added), deadline);
            ops.extend(
                words
                    .into_iter()
                    .map(|(kind, text)| (kind, text.to_owned())),
            );
        } else if !removed.is_empty() && !added.is_empty() {
            // Unrelated lines stay a separate removal and insertion; the
            // empty unchanged run between them keeps them in separate hunks.
            ops.push((Kind::Delete, removed));
            ops.push((Kind::Equal, String::new()));
            ops.push((Kind::Insert, added));
        } else if !removed.is_empty() {
            ops.push((Kind::Delete, removed));
        } else {
            ops.push((Kind::Insert, added));
        }
    }
    let ops = normalize(ops);
    // Pure insertions around a kept word stay separate: "up to 5 attempts,
    // with".
    let removes_before = |k: usize| {
        ops[k - 1].0 == Kind::Delete
            || (ops[k - 1].0 == Kind::Insert && k >= 2 && ops[k - 2].0 == Kind::Delete)
    };
    let mut folded = Vec::with_capacity(ops.len());
    for (k, op) in ops.iter().enumerate() {
        let inner = k > 0
            && k + 1 < ops.len()
            && op.0 == Kind::Equal
            && removes_before(k)
            && ops[k + 1].0 == Kind::Delete;
        if inner && !op.1.contains('\n') && op.1.split_whitespace().count() <= 2 {
            folded.push((Kind::Delete, op.1.clone()));
            folded.push((Kind::Insert, op.1.clone()));
        } else {
            folded.push(op.clone());
        }
    }
    normalize(folded)
        .into_iter()
        .map(|(kind, text)| Op {
            kind,
            text,
            hunk: None,
        })
        .collect()
}

/// The edit script between token lists, deletions before insertions.
fn seq_ops<'a>(old: &[&'a str], new: &[&'a str], deadline: Instant) -> Vec<(Kind, &'a str)> {
    let mut out = Vec::new();
    for op in capture_diff_deadline(
        Algorithm::Myers,
        old,
        0..old.len(),
        new,
        0..new.len(),
        Some(deadline),
    ) {
        let (old_range, new_range) = (op.old_range(), op.new_range());
        match op {
            DiffOp::Equal { .. } => {
                out.extend(old[old_range].iter().map(|text| (Kind::Equal, *text)));
            }
            DiffOp::Delete { .. } | DiffOp::Insert { .. } | DiffOp::Replace { .. } => {
                out.extend(old[old_range].iter().map(|text| (Kind::Delete, *text)));
                out.extend(new[new_range].iter().map(|text| (Kind::Insert, *text)));
            }
        }
    }
    out
}

/// Runs of whitespace and of everything else.
fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (at, c) in text.char_indices() {
        let is_space = c.is_whitespace();
        if space.is_some_and(|space| space != is_space) {
            out.push(&text[start..at]);
            start = at;
        }
        space = Some(is_space);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Whether replaced text shares at least half its words with its
/// replacement, so a word diff reads as an edit rather than as a line swap.
fn related(old: &str, new: &str, deadline: Instant) -> bool {
    // Words match whatever their case or the punctuation around them.
    let normal = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .map(|word| {
                word.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .collect()
    };
    let (old, new) = (normal(old), normal(new));
    let old: Vec<&str> = old.iter().map(String::as_str).collect();
    let new: Vec<&str> = new.iter().map(String::as_str).collect();
    if old.is_empty() && new.is_empty() {
        return false;
    }
    let shared = seq_ops(&old, &new, deadline)
        .iter()
        .filter(|(kind, _)| *kind == Kind::Equal)
        .count();
    4 * shared >= old.len() + new.len()
}

/// Collapses each run of changes into one deletion followed by one
/// insertion, and joins neighbouring unchanged runs.
fn normalize(ops: Vec<(Kind, String)>) -> Vec<(Kind, String)> {
    let mut out: Vec<(Kind, String)> = Vec::with_capacity(ops.len());
    let mut ops = ops.into_iter().peekable();
    while let Some((kind, text)) = ops.next() {
        if kind == Kind::Equal {
            match out.last_mut() {
                Some((Kind::Equal, last)) => last.push_str(&text),
                _ => out.push((kind, text)),
            }
            continue;
        }
        let (mut removed, mut added) = (String::new(), String::new());
        let mut take = |kind: Kind, text: String| match kind {
            Kind::Delete => removed.push_str(&text),
            _ => added.push_str(&text),
        };
        take(kind, text);
        while let Some((kind, text)) = ops.next_if(|(kind, _)| *kind != Kind::Equal) {
            take(kind, text);
        }
        if !removed.is_empty() {
            out.push((Kind::Delete, removed));
        }
        if !added.is_empty() {
            out.push((Kind::Insert, added));
        }
    }
    out
}

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
    let mut at = 0;
    let mut out = String::new();
    // Points wait here for a line that can hold them.
    let mut pending: Vec<(usize, (char, char))> = Vec::new();
    for op in &comparison.ops {
        let hunk = op.hunk.unwrap_or_default();
        let mut queue = None;
        if op.kind == Kind::Equal || op.kind == shown {
            let start = flush(&mut out, &mut pending, &op.text);
            let rest = &op.text[start..];
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
    out.push(tag(hunk));
    out.push_str(POINT);
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
    use markup::Block;
    let tag = tag(hunk);
    let spans = |spans: &[markup::Span]| spans.iter().any(|span| span.text.contains(tag));
    let blocks = |blocks: &[Block]| blocks.iter().any(|block| block_has_hunk(block, hunk));
    match block {
        Block::Paragraph(text) | Block::Heading { spans: text, .. } => spans(text),
        Block::List(list) => list.items.iter().any(|item| blocks(&item.blocks)),
        Block::Quote(inner) | Block::Alert { blocks: inner, .. } => blocks(inner),
        Block::Code { text, .. } | Block::Html(text) => text.contains(tag),
        Block::Table(table) => {
            table.header.iter().any(|cell| spans(cell))
                || table.rows.iter().flatten().any(|cell| spans(cell))
        }
        Block::Rule => false,
    }
}

fn tag(hunk: usize) -> char {
    let code = (TAG_BASE + hunk as u32).min(TAG_LAST);
    char::from_u32(code).expect("private use code points are chars")
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
        out.push(tag(hunk));
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

/// Splits `text` from a parsed redline into runs with their marks, dropping
/// the sentinels. `open` carries a mark across calls for text split into
/// several spans.
pub fn split_marks<'a>(text: &'a str, open: &mut Option<Mark>) -> Vec<(Option<Mark>, &'a str)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let deleted = match c {
            INS_OPEN | DEL_OPEN => Some(c == DEL_OPEN),
            INS_CLOSE | DEL_CLOSE => None,
            _ => continue,
        };
        if start < at {
            out.push((*open, &text[start..at]));
        }
        start = at + c.len_utf8();
        *open = deleted.map(|deleted| {
            let hunk = match chars.peek() {
                Some(&(at, tag)) if (TAG_BASE..=TAG_LAST).contains(&(tag as u32)) => {
                    chars.next();
                    start = at + tag.len_utf8();
                    (tag as u32 - TAG_BASE) as usize
                }
                _ => 0,
            };
            Mark { hunk, deleted }
        });
    }
    if start < text.len() {
        out.push((*open, &text[start..]));
    }
    out
}

/// `text` without redline sentinels.
pub fn strip_marks(text: &str) -> String {
    text.chars().filter(|&c| !is_sentinel(c)).collect()
}

fn is_sentinel(c: char) -> bool {
    matches!(c, INS_OPEN | INS_CLOSE | DEL_OPEN | DEL_CLOSE)
        || (TAG_BASE..=TAG_LAST).contains(&(c as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each hunk as `[removed|added]`, in the new text's order.
    fn hunks(old: &str, new: &str) -> Vec<String> {
        compare(old, new)
            .hunks
            .iter()
            .map(|hunk| format!("[{}|{}]", hunk.removed, hunk.added))
            .collect()
    }

    /// One side with marks written as `{+…+}` and `[-…-]`, and a point as
    /// `|`.
    fn readable_side(old: &str, new: &str, side: Side) -> String {
        let marked = marked(&compare(old, new), side, &[]).replace(POINT, "|");
        let mut open = None;
        let mut out = String::new();
        let mut last: Option<Mark> = None;
        for (mark, text) in split_marks(&marked, &mut open) {
            if let Some(last) = last.filter(|_| mark != last) {
                out.push_str(if last.deleted { "-]" } else { "+}" });
            }
            if mark.is_some() && mark != last {
                out.push_str(if mark.unwrap().deleted { "[-" } else { "{+" });
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
    fn identical_texts_have_no_hunks() {
        assert!(compare("one\ntwo\n", "one\ntwo\n").hunks.is_empty());
        assert!(compare("", "").hunks.is_empty());
    }

    #[test]
    fn diffs_words_within_a_changed_line() {
        assert_eq!(
            hunks(
                "The quick brown fox jumps over the dog.\n",
                "The quick brown fox leaps over the dog.\n"
            ),
            ["[jumps|leaps]"]
        );
    }

    #[test]
    fn folds_short_unchanged_runs_between_changes() {
        // "fox" sits between two replacements, so the clause reads as one.
        assert_eq!(
            hunks(
                "The quick brown fox jumps over the lazy dog today.\n",
                "The quick red fox leaps over the lazy dog today.\n"
            ),
            ["[brown fox jumps|red fox leaps]"]
        );
    }

    #[test]
    fn keeps_long_unchanged_runs_between_changes() {
        assert_eq!(
            hunks(
                "Alpha one two three four omega.\n",
                "Beta one two three four sigma.\n"
            ),
            ["[Alpha|Beta]", "[omega.|sigma.]"]
        );
    }

    #[test]
    fn keeps_insertions_around_a_kept_word_apart() {
        assert_eq!(
            hunks("a b c d e f\n", "a X b Y c d e f\n"),
            ["[|X ]", "[|Y ]"]
        );
    }

    #[test]
    fn unrelated_replacements_stay_a_removal_and_an_insertion() {
        let comparison = compare(
            "Intro\nThe cat sat on the mat.\nEnd\n",
            "Intro\nDeploys now retry on failure.\nEnd\n",
        );
        let pairs: Vec<_> = comparison
            .hunks
            .iter()
            .map(|hunk| (hunk.removed.as_str(), hunk.added.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("The cat sat on the mat.\n", ""),
                ("", "Deploys now retry on failure.\n")
            ]
        );
    }

    #[test]
    fn hunks_know_their_offsets_in_both_texts() {
        let (old, new) = ("one\ntwo\nthree\n", "one\n2\nthree\nfour\n");
        let comparison = compare(old, new);
        for hunk in &comparison.hunks {
            assert_eq!(&old[hunk.old.clone()], hunk.removed);
            assert_eq!(&new[hunk.new.clone()], hunk.added);
        }
        assert_eq!(comparison.hunks.len(), 3);
        assert!(comparison.hunks[0].is_removal());
        assert!(comparison.hunks[1].is_addition());
        assert_eq!(comparison.hunks[2].added, "four\n");
    }

    #[test]
    fn every_change_belongs_to_its_hunk() {
        let comparison = compare("a b c\nkeep\nx\n", "a B c\nkeep\n");
        for op in &comparison.ops {
            assert_eq!(op.hunk.is_some(), op.kind != Kind::Equal);
        }
        assert_eq!(
            comparison.ops.iter().filter_map(|op| op.hunk).max(),
            Some(1)
        );
    }

    #[test]
    fn pure_additions_and_removals() {
        assert_eq!(hunks("one\nthree\n", "one\ntwo\nthree\n"), ["[|two\n]"]);
        assert_eq!(hunks("one\ntwo\nthree\n", "one\nthree\n"), ["[two\n|]"]);
        assert_eq!(hunks("", "Hello\n"), ["[|Hello\n]"]);
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
        assert_eq!(first.last().unwrap(), &(mark, "c"));
        assert_eq!(second.first().unwrap(), &(mark, "d"));
        assert_eq!(strip_marks(&marked), "a cd\n");
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

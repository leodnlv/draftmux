//! Source-preserving GFM printer. Comrak determines structure, while source
//! slices retain escapes, entities, references, fences and opaque payloads.
//! Only prose whitespace, structural markers and table padding are rewritten.

use comrak::nodes::{AstNode, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, format_html, parse_document};
use unicode_width::UnicodeWidthStr;

pub const WRAP_WIDTH: usize = 80;
const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    /// Preserve soft line breaks and inline whitespace.
    Keep,
    /// Fill prose to 80 display columns without splitting atomic inlines.
    Always,
    /// Join soft breaks, retaining hard breaks and opaque inline content.
    Never,
}

/// A declined rewrite is distinct from an already-formatted document.
#[derive(Debug)]
pub struct Report {
    pub text: String,
    pub declined: bool,
}

fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.table = true;
    o.extension.tasklist = true;
    o.extension.strikethrough = true;
    o.extension.autolink = true;
    o.extension.alerts = true;
    o.extension.footnotes = true;
    o.extension.math_dollars = true;
    o.extension.math_code = true;
    o.render.r#unsafe = true;
    o
}

pub fn format(input: &str, wrap: Wrap) -> String {
    format_report(input, wrap).text
}

pub fn format_report(input: &str, wrap: Wrap) -> Report {
    let Some((front, body)) = frontmatter(input) else {
        return Report {
            text: input.to_owned(),
            declined: true,
        };
    };
    // Comrak accepts CR line endings, but their source columns cannot index
    // an LF-based source map. The editor normalizes these before formatting.
    if body.contains('\r') && body.replace("\r\n", "").contains('\r') {
        return Report {
            text: input.to_owned(),
            declined: true,
        };
    }
    let has_frontmatter = !front.trim_start_matches('\u{feff}').is_empty();
    let (output, declined) = print(body, wrap, has_frontmatter);
    if equivalent(body, &output) && print(&output, wrap, has_frontmatter).0 == output {
        Report {
            text: format!("{front}{output}"),
            declined,
        }
    } else {
        Report {
            text: input.to_owned(),
            declined: true,
        }
    }
}

// Invalid YAML/TOML is still opaque. An unfinished leading envelope is not
// rewritten into a rule followed by Markdown that could later become fields.
fn frontmatter(input: &str) -> Option<(&str, &str)> {
    let bom = if input.starts_with('\u{feff}') { 3 } else { 0 };
    let mut lines = input[bom..].split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Some(("", input));
    };
    let delimiter = first.trim_end_matches(['\r', '\n']);
    if !matches!(delimiter.trim_end(), "---" | "+++") {
        return Some(input.split_at(bom));
    }
    if delimiter != delimiter.trim_end() {
        return None;
    }
    let mut end = bom + first.len();
    for line in lines {
        end += line.len();
        let close = line.trim_end_matches(['\r', '\n']);
        if close == delimiter || (delimiter == "---" && close == "...") {
            for blank in input[end..].split_inclusive('\n') {
                if !blank.trim().is_empty() {
                    break;
                }
                end += blank.len();
            }
            return Some(input.split_at(end));
        }
    }
    None
}

struct Source<'a> {
    text: &'a str,
    lines: Vec<usize>,
}

impl<'a> Source<'a> {
    fn new(text: &'a str) -> Self {
        let mut lines = vec![0];
        lines.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { text, lines }
    }

    fn offset(&self, line: usize) -> usize {
        self.lines
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(self.text.len())
    }

    fn line(&self, line: usize) -> &str {
        &self.text[self.offset(line)..self.offset(line + 1)]
    }

    fn inline<'n>(&self, node: &'n AstNode<'n>) -> String {
        let pos = node.data.borrow().sourcepos;
        if pos.start.line == 0 || pos.start.column == 0 {
            return String::new();
        }
        let mut start = self.offset(pos.start.line) + pos.start.column - 1;
        // Comrak's first escaped Text character starts after its backslash.
        if matches!(node.data.borrow().value, NodeValue::Text(_)) {
            let slashes = self.text[..start]
                .bytes()
                .rev()
                .take_while(|b| *b == b'\\')
                .count();
            if slashes % 2 == 1 {
                start -= 1;
            }
        }
        let end = self.offset(pos.end.line) + pos.end.column;
        let Some(raw) = self.text.get(start..end) else {
            return String::new();
        };
        let mut out = String::new();
        for (i, line) in raw.split_inclusive('\n').enumerate() {
            if i == 0 {
                out.push_str(line);
            } else {
                out.push_str(&self.unprefix(line, pos.start.line + i, node));
            }
        }
        out
    }

    fn raw<'n>(&self, node: &'n AstNode<'n>) -> String {
        let pos = node.data.borrow().sourcepos;
        let mut raw = self.range(
            pos.start.line,
            pos.end.line + 1,
            node.parent().unwrap_or(node),
        );
        if matches!(node.data.borrow().value, NodeValue::Paragraph)
            && node.parent().is_some_and(|parent| {
                matches!(parent.data.borrow().value, NodeValue::TaskItem(_))
                    && parent.data.borrow().sourcepos.start.line == pos.start.line
            })
            && (raw.starts_with("[ ] ") || raw.starts_with("[x] ") || raw.starts_with("[X] "))
        {
            raw.drain(..4);
        }
        raw
    }

    fn range<'n>(&self, start: usize, end: usize, node: &'n AstNode<'n>) -> String {
        (start..end)
            .map(|line| self.unprefix(self.line(line), line, node))
            .collect()
    }

    fn unprefix<'n>(&self, line: &str, number: usize, node: &'n AstNode<'n>) -> String {
        let mut ancestors = node.ancestors().collect::<Vec<_>>();
        ancestors.reverse();
        let mut text = line.to_owned();
        for ancestor in ancestors {
            let data = ancestor.data.borrow();
            match &data.value {
                NodeValue::BlockQuote | NodeValue::Alert(_) => {
                    let trimmed = text.trim_start_matches([' ', '\t']);
                    if let Some(rest) = trimmed.strip_prefix('>') {
                        text = rest
                            .strip_prefix(' ')
                            .or_else(|| rest.strip_prefix('\t'))
                            .unwrap_or(rest)
                            .to_owned();
                    }
                }
                NodeValue::Item(list) => {
                    text = strip_item(
                        &text,
                        list.padding + list.marker_offset,
                        number == data.sourcepos.start.line,
                    );
                }
                NodeValue::TaskItem(_) => {
                    // Task items carry no list-padding metadata. Derive it
                    // from their marker, not the checkbox's content column.
                    let first = self.line(data.sourcepos.start.line);
                    let marker = first
                        .get(data.sourcepos.start.column.saturating_sub(1)..)
                        .unwrap_or(first);
                    let width = marker_width(marker).unwrap_or(2);
                    text = strip_item(&text, width, number == data.sourcepos.start.line);
                }
                _ => {}
            }
        }
        text
    }
}

fn marker_width(line: &str) -> Option<usize> {
    let start = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[start..];
    let marker = if rest.starts_with(['-', '+', '*']) {
        1
    } else {
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || !matches!(rest.as_bytes().get(n), Some(b'.' | b')')) {
            return None;
        }
        n + 1
    };
    let mut width = start + marker;
    for c in rest[marker..].chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    Some(width)
}

fn strip_item(line: &str, width: usize, first: bool) -> String {
    if first {
        let trimmed = line.trim_start_matches([' ', '\t']);
        let marker = if trimmed.starts_with(['-', '+', '*']) {
            1
        } else {
            trimmed.bytes().take_while(u8::is_ascii_digit).count() + 1
        };
        return trimmed
            .get(marker..)
            .unwrap_or("")
            .trim_start_matches([' ', '\t'])
            .to_owned();
    }
    let mut column = 0;
    let mut end = 0;
    for (i, c) in line.char_indices() {
        if column >= width {
            break;
        }
        match c {
            ' ' => column += 1,
            '\t' => column += 4 - column % 4,
            _ => break,
        }
        end = i + c.len_utf8();
    }
    format!(
        "{}{}",
        " ".repeat(column.saturating_sub(width)),
        &line[end..]
    )
}

struct Printer<'a> {
    source: Source<'a>,
    wrap: Wrap,
    declined: bool,
    has_frontmatter: bool,
}

fn print(input: &str, wrap: Wrap, has_frontmatter: bool) -> (String, bool) {
    print_at(input, wrap, has_frontmatter, 0, 0)
}

fn print_at(
    input: &str,
    wrap: Wrap,
    has_frontmatter: bool,
    indent: usize,
    depth: usize,
) -> (String, bool) {
    let arena = Arena::new();
    let root = parse_document(&arena, input, &options());
    let mut printer = Printer {
        source: Source::new(input),
        wrap,
        declined: false,
        has_frontmatter,
    };
    let out = printer.children(root, indent, false, depth);
    (out, printer.declined)
}

impl Printer<'_> {
    fn children<'a>(
        &mut self,
        node: &'a AstNode<'a>,
        indent: usize,
        tight: bool,
        depth: usize,
    ) -> String {
        let pos = node.data.borrow().sourcepos;
        let mut line = pos.start.line.max(1);
        if matches!(node.data.borrow().value, NodeValue::Alert(_)) {
            line += 1;
        }
        let mut out = String::new();
        let mut ignore_next = false;
        let mut ignore_start = None;
        // Footnote nodes may be ordered by reference traversal rather than
        // source position. Definitions must stay where the author put them.
        let mut children = node.children().collect::<Vec<_>>();
        children.sort_by_key(|child| {
            let start = child.data.borrow().sourcepos.start;
            (start.line, start.column)
        });
        for child in children {
            let span = child.data.borrow().sourcepos;
            let raw = self.source.raw(child);
            if let Some(start) = ignore_start {
                if raw.trim() == "<!-- prettier-ignore-end -->" {
                    append(
                        &mut out,
                        &self.source.range(start, span.end.line + 1, node),
                        false,
                    );
                    line = span.end.line + 1;
                    ignore_start = None;
                }
                continue;
            }
            if span.start.line > line {
                let gap = self.source.range(line, span.start.line, node);
                append(&mut out, &gap, false);
            }
            if raw.trim() == "<!-- prettier-ignore-start -->" {
                ignore_start = Some(span.start.line);
                line = span.start.line;
                continue;
            }
            let candidate = if ignore_next {
                raw.clone()
            } else {
                self.block(child, indent, depth + 1)
            };
            ignore_next = raw.trim() == "<!-- prettier-ignore -->";
            // Keep an unsafe block locally, so unrelated supported blocks can
            // still format. The final check also verifies cross-block links.
            let rendered = if equivalent(&raw, &candidate) {
                candidate
            } else {
                self.declined = true;
                raw
            };
            append(&mut out, &rendered, tight);
            line = span.end.line + 1;
        }
        let last = if matches!(node.data.borrow().value, NodeValue::Document) {
            self.source.lines.len() + 1
        } else {
            pos.end.line + 1
        };
        if line < last {
            let gap = self.source.range(line, last, node);
            append(&mut out, &gap, false);
        }
        out
    }

    fn block<'a>(&mut self, node: &'a AstNode<'a>, indent: usize, depth: usize) -> String {
        if depth > MAX_DEPTH {
            self.declined = true;
            return self.source.raw(node);
        }
        match &node.data.borrow().value {
            NodeValue::Paragraph => {
                let raw = self.source.raw(node);
                let definitions = definition_prefix(&raw);
                if definitions > 0 {
                    // Comrak removes leading definitions from paragraph text
                    // without advancing inline source positions. A separating
                    // blank line gives the prose accurate positions while
                    // retaining reference resolution in the same document.
                    let separated = format!("{}\n{}", &raw[..definitions], &raw[definitions..]);
                    let (body, declined) = print_at(&separated, self.wrap, false, indent, depth);
                    self.declined |= declined;
                    return body;
                }
                // Dollar dialects disagree on unmatched/doubled delimiters.
                // Retain paragraphs containing dollar text the parser did not
                // classify as math, rather than editing a possible payload.
                if node.descendants().any(
                    |n| matches!(&n.data.borrow().value, NodeValue::Text(t) if t.contains('$')),
                ) {
                    return self.source.raw(node);
                }
                let pieces = self.inlines(node, depth);
                let width = WRAP_WIDTH.saturating_sub(indent).max(1);
                let task = node.previous_sibling().is_none()
                    && node
                        .parent()
                        .is_some_and(|p| matches!(p.data.borrow().value, NodeValue::TaskItem(_)));
                let text = layout_first(
                    &pieces,
                    self.wrap,
                    width,
                    if task {
                        width.saturating_sub(4).max(1)
                    } else {
                        width
                    },
                );
                format!("{}\n", text.trim_end_matches([' ', '\t', '\r', '\n']))
            }
            NodeValue::Heading(heading) => {
                let text =
                    layout(&self.inlines(node, depth), Wrap::Keep, usize::MAX).replace('\n', " ");
                format!(
                    "{}{}{}\n",
                    "#".repeat(heading.level as usize),
                    if text.is_empty() { "" } else { " " },
                    text.trim()
                )
            }
            NodeValue::List(list) => {
                let mut out = String::new();
                // Adjacent distinct lists cannot both use the same delimiter.
                let preceding = node.preceding_siblings().skip(1).take_while(|n| matches!(&n.data.borrow().value, NodeValue::List(other) if other.list_type == list.list_type)).count();
                for (index, item) in node.children().enumerate() {
                    let marker = if list.list_type == ListType::Bullet {
                        if preceding % 2 == 0 {
                            "- ".to_owned()
                        } else {
                            "* ".to_owned()
                        }
                    } else {
                        format!(
                            "{}{} ",
                            list.start + index,
                            if preceding % 2 == 0 { "." } else { ")" }
                        )
                    };
                    let empty_task = matches!(item.data.borrow().value, NodeValue::TaskItem(_))
                        && item.first_child().is_none();
                    let mut body = if empty_task {
                        String::new()
                    } else {
                        self.children(item, indent + marker.len(), list.tight, depth)
                    };
                    if let NodeValue::TaskItem(task) = &item.data.borrow().value {
                        body = format!(
                            "[{}]{}",
                            if task.symbol.is_some() { "x" } else { " " },
                            if body.is_empty() {
                                "\n".to_owned()
                            } else {
                                format!(" {body}")
                            }
                        );
                    }
                    let text = prefix(&body, &marker, &" ".repeat(marker.len()));
                    append(&mut out, &text, list.tight);
                }
                out
            }
            NodeValue::BlockQuote | NodeValue::Alert(_) => {
                let mut body = self.children(node, indent + 2, false, depth);
                if let NodeValue::Alert(alert) = &node.data.borrow().value {
                    if alert.title.is_some() {
                        return self.source.raw(node);
                    }
                    body = format!(
                        "[!{}]\n{}",
                        format!("{:?}", alert.alert_type).to_uppercase(),
                        body
                    );
                }
                prefix(&body, "> ", "> ")
            }
            NodeValue::Table(table) => {
                let raw = self.source.raw(node);
                if raw.contains('\t') {
                    return raw;
                }
                let mut rows = Vec::new();
                for (i, line) in raw.lines().enumerate() {
                    if i == 1 {
                        continue;
                    }
                    let mut cells = cells(line);
                    cells.resize(table.num_columns, String::new());
                    cells.truncate(table.num_columns);
                    let cells = cells
                        .into_iter()
                        .map(|cell| {
                            let arena = Arena::new();
                            let mut o = options();
                            o.extension.table = false;
                            let root = parse_document(&arena, &cell, &o);
                            let printer = Printer {
                                source: Source::new(&cell),
                                wrap: Wrap::Keep,
                                declined: false,
                                has_frontmatter: false,
                            };
                            root.first_child()
                                .map(|p| layout(&printer.inlines(p, depth), Wrap::Keep, usize::MAX))
                                .unwrap_or_default()
                        })
                        .collect();
                    rows.push(cells);
                }
                align_table(&rows, &table.alignments)
            }
            NodeValue::ThematicBreak => {
                let in_item = node.ancestors().any(|n| {
                    matches!(
                        n.data.borrow().value,
                        NodeValue::Item(_) | NodeValue::TaskItem(_)
                    )
                });
                let first = node.previous_sibling().is_none()
                    && node
                        .parent()
                        .is_some_and(|p| matches!(p.data.borrow().value, NodeValue::Document));
                if in_item || (first && !self.has_frontmatter) {
                    "***\n".into()
                } else {
                    "---\n".into()
                }
            }
            // These syntax families are opaque, including footnote bodies.
            _ => self.source.raw(node),
        }
    }

    fn inlines<'a>(&self, node: &'a AstNode<'a>, depth: usize) -> Vec<Piece> {
        let mut out = Vec::new();
        for child in node.children() {
            self.inline(child, depth + 1, &mut out);
        }
        out
    }

    fn inline<'a>(&self, node: &'a AstNode<'a>, depth: usize, out: &mut Vec<Piece>) {
        if depth > MAX_DEPTH {
            out.push(Piece::Atom(self.source.inline(node)));
            return;
        }
        match &node.data.borrow().value {
            NodeValue::Text(_) => out.push(Piece::Text(self.source.inline(node))),
            NodeValue::SoftBreak => out.push(Piece::Soft),
            NodeValue::LineBreak => out.push(Piece::Hard),
            NodeValue::Emph | NodeValue::Strong | NodeValue::Strikethrough => {
                let marker = match node.data.borrow().value {
                    NodeValue::Emph => "*",
                    NodeValue::Strong => "**",
                    _ => "~~",
                };
                out.push(Piece::Atom(marker.into()));
                out.extend(self.inlines(node, depth));
                out.push(Piece::Atom(marker.into()));
            }
            NodeValue::Link(_) | NodeValue::Image(_) => {
                let mut raw = self.source.inline(node);
                // Only the label changes; reference spelling, destinations
                // and titles keep their exact bytes, including title newlines.
                if let Some((start, end)) = label_range(&raw) {
                    let wrap = if self.wrap == Wrap::Keep {
                        Wrap::Keep
                    } else {
                        Wrap::Never
                    };
                    let label = layout(&self.inlines(node, depth), wrap, usize::MAX);
                    raw.replace_range(start..end, &label);
                }
                out.push(Piece::Atom(raw));
            }
            NodeValue::Code(_) => {
                let mut raw = self.source.inline(node);
                if self.wrap != Wrap::Keep {
                    raw = raw.replace("\r\n", " ").replace('\n', " ");
                }
                out.push(Piece::Atom(raw));
            }
            _ => out.push(Piece::Atom(self.source.inline(node))),
        }
    }
}

fn definition_prefix(raw: &str) -> usize {
    if !raw.trim_start().starts_with('[') || !raw.contains("]:") {
        return 0;
    }
    let mut boundary = 0;
    let mut end = 0;
    // Bound lookahead for malformed labels; larger ambiguous blocks remain
    // protected by the semantic check rather than quadratic prefix parsing.
    for line in raw.split_inclusive('\n').take(MAX_DEPTH) {
        end += line.len();
        let arena = Arena::new();
        let root = parse_document(&arena, &raw[..end], &options());
        if root.first_child().is_none() {
            boundary = end;
        } else if boundary > 0 {
            break;
        }
    }
    boundary
}

fn label_range(raw: &str) -> Option<(usize, usize)> {
    let start = if raw.starts_with("![") {
        2
    } else if raw.starts_with('[') {
        1
    } else {
        return None;
    };
    let mut depth = 1;
    let mut escaped = false;
    let mut code = 0;
    let bytes = raw.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == b'\\' && code == 0 {
            escaped = true;
            i += 1;
            continue;
        }
        if c == b'`' {
            let ticks = bytes[i..].iter().take_while(|c| **c == b'`').count();
            if code == 0 {
                code = ticks;
            } else if code == ticks {
                code = 0;
            }
            i += ticks;
            continue;
        }
        if code == 0 {
            if c == b'[' {
                depth += 1;
            }
            if c == b']' {
                depth -= 1;
                if depth == 0 {
                    return Some((start, i));
                }
            }
        }
        i += 1;
    }
    None
}

#[derive(Debug)]
enum Piece {
    Text(String),
    Atom(String),
    Soft,
    Hard,
}

fn layout(pieces: &[Piece], wrap: Wrap, width: usize) -> String {
    layout_first(pieces, wrap, width, width)
}

fn layout_first(pieces: &[Piece], wrap: Wrap, width: usize, first_width: usize) -> String {
    if wrap == Wrap::Keep {
        let mut text = String::new();
        for piece in pieces {
            match piece {
                Piece::Text(s) | Piece::Atom(s) => text.push_str(s),
                Piece::Soft => text.push('\n'),
                Piece::Hard => {
                    text.truncate(text.trim_end_matches([' ', '\t']).len());
                    text.push_str("\\\n");
                }
            }
        }
        return text;
    }
    let mut runs = vec![vec![String::new()]];
    for piece in pieces {
        match piece {
            Piece::Hard => {
                runs.push(vec![String::new()]);
            }
            Piece::Soft => runs.last_mut().unwrap().push(String::new()),
            Piece::Atom(s) => runs.last_mut().unwrap().last_mut().unwrap().push_str(s),
            Piece::Text(s) => {
                for c in s.chars() {
                    let words = runs.last_mut().unwrap();
                    if matches!(c, ' ' | '\t' | '\r' | '\n') {
                        if !words.last().unwrap().is_empty() {
                            words.push(String::new());
                        }
                    } else {
                        words.last_mut().unwrap().push(c);
                    }
                }
            }
        }
    }
    runs.into_iter()
        .enumerate()
        .map(|(run, words)| {
            let words = words
                .into_iter()
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>();
            if wrap == Wrap::Never {
                return words.join(" ");
            }
            let mut out = Vec::new();
            let mut start = 0;
            while start < words.len() {
                let mut end = start + 1;
                let mut used = words[start].width();
                let limit = if run == 0 && out.is_empty() {
                    first_width
                } else {
                    width
                };
                while end < words.len() && used + 1 + words[end].width() <= limit {
                    used += 1 + words[end].width();
                    end += 1;
                }
                let original = end;
                while end > start + 1 && end < words.len() && risky_start(&words[end..]) {
                    end -= 1;
                }
                if end < words.len() && risky_start(&words[end..]) {
                    end = original;
                    while end < words.len() && risky_start(&words[end..]) {
                        end += 1;
                    }
                }
                out.push(words[start..end].join(" "));
                start = end;
            }
            out.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\\\n")
}

fn risky_start(words: &[String]) -> bool {
    let Some(first) = words.first() else {
        return false;
    };
    first.starts_with(['<', '|', '>'])
        || first.contains("]:")
        || first.starts_with("```")
        || first.starts_with("~~~")
        || first
            .chars()
            .all(|c| matches!(c, '#' | '-' | '+' | '*' | '_' | '='))
        || first
            .trim_end_matches(['.', ')'])
            .bytes()
            .all(|b| b.is_ascii_digit())
}

fn append(out: &mut String, block: &str, tight: bool) {
    let block = block.trim_start_matches(['\n', '\r']);
    if block.trim().is_empty() {
        return;
    }
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if !tight && !out.ends_with("\n\n") && !out.ends_with("\r\n\r\n") {
            out.push('\n');
        }
    }
    out.push_str(block);
}

fn prefix(body: &str, first: &str, rest: &str) -> String {
    if body.is_empty() {
        return format!("{}\n", first.trim_end());
    }
    let mut out = String::new();
    for (i, line) in body.split_inclusive('\n').enumerate() {
        let p = if i == 0 { first } else { rest };
        if line == "\n" {
            out.push_str(p.trim_end());
            out.push('\n');
        } else {
            out.push_str(p);
            out.push_str(line);
        }
    }
    out
}

fn cells(line: &str) -> Vec<String> {
    let line = line.trim().strip_prefix('|').unwrap_or(line.trim());
    let mut cells = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if c == '|' && !escaped {
            cells.push(line[start..i].trim().to_owned());
            start = i + 1;
        }
        escaped = c == '\\' && !escaped;
    }
    if start < line.len() {
        cells.push(line[start..].trim().to_owned());
    }
    cells
}

fn align_table(rows: &[Vec<String>], alignments: &[TableAlignment]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let widths = (0..alignments.len())
        .map(|i| rows.iter().map(|r| r[i].width()).max().unwrap_or(0).max(3))
        .collect::<Vec<_>>();
    let row = |cells: &[String]| {
        let mut s = "|".to_owned();
        for (cell, width) in cells.iter().zip(&widths) {
            s.push_str(&format!(
                " {cell}{} |",
                " ".repeat(width.saturating_sub(cell.width()))
            ));
        }
        s.push('\n');
        s
    };
    let mut out = row(&rows[0]);
    let separators = alignments
        .iter()
        .zip(&widths)
        .map(|(a, width)| {
            let left = matches!(a, TableAlignment::Left | TableAlignment::Center);
            let right = matches!(a, TableAlignment::Right | TableAlignment::Center);
            format!(
                "{}{}{}",
                if left { ":" } else { "" },
                "-".repeat(width - usize::from(left) - usize::from(right)),
                if right { ":" } else { "" }
            )
        })
        .collect::<Vec<_>>();
    out.push_str(&row(&separators));
    for cells in &rows[1..] {
        out.push_str(&row(cells));
    }
    out
}

/// Rendered semantics include code, HTML, hard breaks, math and footnotes.
/// Whitespace normalization applies only to prose text, never literal payloads.
fn equivalent(a: &str, b: &str) -> bool {
    fn canonical(input: &str) -> String {
        let arena = Arena::new();
        let options = options();
        let root = parse_document(&arena, input, &options);
        for node in root.descendants() {
            let prose = node
                .ancestors()
                .skip(1)
                .any(|parent| matches!(parent.data.borrow().value, NodeValue::Paragraph));
            let mut data = node.data.borrow_mut();
            match &mut data.value {
                NodeValue::SoftBreak => data.value = NodeValue::Text(" ".into()),
                NodeValue::Text(text) if prose => {
                    let mut normalized = String::new();
                    for c in text.chars() {
                        if matches!(c, ' ' | '\t' | '\r' | '\n') {
                            if !normalized.ends_with(' ') {
                                normalized.push(' ');
                            }
                        } else {
                            normalized.push(c);
                        }
                    }
                    *text = normalized.into();
                }
                _ => {}
            }
        }
        let mut html = String::new();
        format_html(root, &options, &mut html).expect("writing to String");
        html
    }
    canonical(a) == canonical(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_guard_detects_changes_to_non_prose_payloads() {
        for (a, b) in [
            ("a  \nb", "a b"),
            ("[a](/u)", "[a](/v)"),
            ("`a  b`", "`a b`"),
            ("<i>a</i>", "<b>a</b>"),
            ("$a_b$", "$a\\_b$"),
            ("3. a", "1. a"),
            ("- [x] a", "- [ ] a"),
            ("x[^1]\n\n[^1]: note", "x\\[^1\\]\n\n[^1]: note"),
        ] {
            assert!(!equivalent(a, b), "{a:?} vs {b:?}");
        }
        assert!(equivalent("a\nb", "a b"));
    }
}

//! GitHub-flavored markdown parser.
//!
//! Follows GitHub's comment rendering: single newlines inside a paragraph are
//! line breaks, HTML comments are hidden, and other HTML is kept literally.
//! The `file` parsers render a file in a repository, such as README.md, where
//! a single newline is a space.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;

use comrak::nodes::{AstNode, ListType, NodeValue, Sourcepos, TableAlignment};
use comrak::{Arena, Options, parse_document};

use crate::source_map::{Lex, Node, Piece, Pieces, SourceMap};
use crate::{
    AlertKind, Alignment, Block, Document, List, ListItem, Span, SpanStyle, Table, push_span,
};

pub fn parse(markdown: &str) -> Document {
    Walk::new(markdown, false, "\n").document().0
}

/// [`parse`], with where each block and run of text came from in
/// `markdown`.
pub fn parse_mapped(markdown: &str) -> (Document, SourceMap) {
    Walk::new(markdown, true, "\n").document()
}

/// [`parse`] as GitHub renders a file in a repository: a single newline
/// inside a paragraph is a space. Hard breaks still break the line.
pub fn parse_file(markdown: &str) -> Document {
    Walk::new(markdown, false, " ").document().0
}

/// [`parse_file`], with where each block and run of text came from in
/// `markdown`.
pub fn parse_file_mapped(markdown: &str) -> (Document, SourceMap) {
    Walk::new(markdown, true, " ").document()
}

/// The edit that checks or unchecks the task list item at `path` in
/// `markdown`'s [`SourceMap`]: the range of the character between the
/// checkbox's brackets and what replaces it. `None` when `path` isn't a
/// task list item.
pub fn toggle_task(markdown: &str, path: &[usize]) -> Option<(Range<usize>, &'static str)> {
    let (_, map) = parse_mapped(markdown);
    let range = map.get(path)?.task.clone()?;
    let replacement = match markdown.get(range.clone())? {
        " " => "x",
        "x" | "X" => " ",
        _ => return None,
    };
    Some((range, replacement))
}

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.alerts = true;
    options
}

/// One conversion of comrak's tree. Without `mapped`, every [`Node`] is
/// empty and no source is read.
struct Walk<'s> {
    source: &'s str,
    mapped: bool,
    /// What a single newline inside a paragraph renders as.
    soft_break: &'static str,
    /// Byte offset of each line's start.
    lines: Vec<usize>,
    /// Character references decoded so far.
    entities: RefCell<HashMap<String, Option<String>>>,
}

/// Inline content: its spans, and the pieces of their text when mapped.
#[derive(Default)]
struct Inline {
    spans: Vec<Span>,
    pieces: Pieces,
}

impl<'s> Walk<'s> {
    fn new(source: &'s str, mapped: bool, soft_break: &'static str) -> Self {
        let lines = if mapped {
            std::iter::once(0)
                .chain(source.match_indices('\n').map(|(ix, _)| ix + 1))
                .collect()
        } else {
            Vec::new()
        };
        Walk {
            source,
            mapped,
            soft_break,
            lines,
            entities: RefCell::default(),
        }
    }

    fn document(&self) -> (Document, SourceMap) {
        let arena = Arena::new();
        let root = parse_document(&arena, self.source, &options());
        let (blocks, nodes) = self.blocks(root);
        (Document { blocks }, SourceMap { blocks: nodes })
    }

    fn blocks<'a>(&self, parent: &'a AstNode<'a>) -> (Vec<Block>, Vec<Node>) {
        parent
            .children()
            .filter_map(|node| self.block(node))
            .unzip()
    }

    fn block<'a>(&self, node: &'a AstNode<'a>) -> Option<(Block, Node)> {
        let mut map = Node {
            source: self.range(node),
            ..Node::default()
        };
        let block = match &node.data().value {
            NodeValue::Paragraph => {
                let inline = self.spans(node, None);
                map.pieces = inline.pieces.pieces;
                Block::Paragraph(inline.spans)
            }
            NodeValue::Heading(heading) => {
                let inline = self.spans(node, None);
                map.pieces = inline.pieces.pieces;
                Block::Heading {
                    level: heading.level,
                    spans: inline.spans,
                }
            }
            NodeValue::List(list) => {
                let (items, nodes) = node.children().map(|item| self.list_item(item)).unzip();
                map.children = nodes;
                Block::List(List {
                    start: (list.list_type == ListType::Ordered).then_some(list.start as u64),
                    tight: list.tight,
                    items,
                })
            }
            NodeValue::BlockQuote => {
                let (blocks, nodes) = self.blocks(node);
                map.children = nodes;
                Block::Quote(blocks)
            }
            NodeValue::Alert(alert) => {
                let kind = match alert.alert_type {
                    comrak::nodes::AlertType::Note => AlertKind::Note,
                    comrak::nodes::AlertType::Tip => AlertKind::Tip,
                    comrak::nodes::AlertType::Important => AlertKind::Important,
                    comrak::nodes::AlertType::Warning => AlertKind::Warning,
                    comrak::nodes::AlertType::Caution => AlertKind::Caution,
                };
                let (blocks, nodes) = self.blocks(node);
                match &alert.title {
                    None => {
                        map.children = nodes;
                        Block::Alert { kind, blocks }
                    }
                    // GitHub only recognizes the marker alone on its line; with
                    // text after it the quote stays a plain quote.
                    Some(title) => {
                        let marker = format!("[!{}] {title}", kind.label().to_uppercase());
                        map.children = std::iter::once(self.marker(&marker, &map.source))
                            .chain(nodes)
                            .collect();
                        let mut quoted = vec![Block::Paragraph(vec![plain(marker)])];
                        quoted.extend(blocks);
                        Block::Quote(quoted)
                    }
                }
            }
            NodeValue::CodeBlock(code) => {
                let text = code.literal.strip_suffix('\n').unwrap_or(&code.literal);
                let first = node.data().sourcepos.start.line + usize::from(code.fenced);
                map.pieces = self.lines(&code.literal, first, &map.source, text.len());
                Block::Code {
                    lang: code.info.split_whitespace().next().map(str::to_owned),
                    text: text.to_owned(),
                }
            }
            NodeValue::Table(table) => {
                let mut header = None;
                let mut rows = Vec::new();
                let mut row_nodes = Vec::new();
                for row in node.children() {
                    let (cells, cell_nodes) = row.children().map(|cell| self.cell(cell)).unzip();
                    let row_node = Node {
                        source: self.range(row),
                        children: cell_nodes,
                        ..Node::default()
                    };
                    match row.data().value {
                        NodeValue::TableRow(true) => header = Some((cells, row_node)),
                        _ => {
                            rows.push(cells);
                            row_nodes.push(row_node);
                        }
                    }
                }
                let (header, header_node) = header.unwrap_or_default();
                map.children = std::iter::once(header_node).chain(row_nodes).collect();
                Block::Table(Table {
                    alignments: table.alignments.iter().map(alignment).collect(),
                    header,
                    rows,
                })
            }
            NodeValue::ThematicBreak => Block::Rule,
            NodeValue::HtmlBlock(html) if is_html_comment(&html.literal) => return None,
            NodeValue::HtmlBlock(html) => {
                let text = html.literal.trim_end();
                let first = node.data().sourcepos.start.line;
                map.pieces = self.lines(&html.literal, first, &map.source, text.len());
                Block::Html(text.to_owned())
            }
            _ => return None,
        };
        Some((block, map))
    }

    fn list_item<'a>(&self, node: &'a AstNode<'a>) -> (ListItem, Node) {
        let task = match &node.data().value {
            NodeValue::TaskItem(task) => Some((task.symbol.is_some(), task.symbol_sourcepos)),
            _ => None,
        };
        let (blocks, nodes) = self.blocks(node);
        let map = Node {
            source: self.range(node),
            children: nodes,
            task: task
                .filter(|_| self.mapped)
                .map(|(_, symbol)| self.position_range(symbol)),
            ..Node::default()
        };
        let task = task.map(|(checked, _)| checked);
        (ListItem { task, blocks }, map)
    }

    fn cell<'a>(&self, node: &'a AstNode<'a>) -> (Vec<Span>, Node) {
        let source = self.range(node);
        let cell = self
            .mapped
            .then(|| Cell::new(self.source, &source))
            .flatten();
        let inline = self.spans(node, cell.as_ref());
        let map = Node {
            source,
            pieces: inline.pieces.pieces,
            ..Node::default()
        };
        (inline.spans, map)
    }

    fn spans<'a>(&self, node: &'a AstNode<'a>, cell: Option<&Cell>) -> Inline {
        let mut out = Inline::default();
        self.collect_spans(node, SpanStyle::default(), None, cell, &mut out);
        out
    }

    fn collect_spans<'a>(
        &self,
        parent: &'a AstNode<'a>,
        style: SpanStyle,
        link: Option<&str>,
        cell: Option<&Cell>,
        out: &mut Inline,
    ) {
        for node in parent.children() {
            match &node.data().value {
                NodeValue::Text(text) => {
                    push_span(&mut out.spans, text, style, link);
                    if self.mapped {
                        let decode = |reference: &str| self.decode(reference);
                        let lex = Lex {
                            escapes: true,
                            entity: Some(&decode),
                            ..Lex::EXACT
                        };
                        let source = self.inline_range(node, cell);
                        out.pieces.lexed(text, source, self.source, lex);
                    }
                }
                NodeValue::SoftBreak | NodeValue::LineBreak => {
                    let text = match node.data().value {
                        NodeValue::SoftBreak => self.soft_break,
                        _ => "\n",
                    };
                    push_span(&mut out.spans, text, style, link);
                    if self.mapped {
                        let source = self.inline_range(node, cell);
                        out.pieces.whole(text, source, self.source);
                    }
                }
                NodeValue::Code(code) => {
                    let code_style = SpanStyle {
                        code: true,
                        ..style
                    };
                    push_span(&mut out.spans, &code.literal, code_style, link);
                    if self.mapped {
                        let source = self.inline_range(node, cell);
                        self.code_span(&code.literal, code.num_backticks, source, cell, out);
                    }
                }
                NodeValue::HtmlInline(html) if is_html_comment(html) => {}
                NodeValue::HtmlInline(html) => {
                    push_span(&mut out.spans, html, style, link);
                    if self.mapped {
                        let source = self.inline_range(node, cell);
                        out.pieces.lexed(html, source, self.source, Lex::EXACT);
                    }
                }
                NodeValue::Emph => {
                    let style = SpanStyle {
                        emph: true,
                        ..style
                    };
                    self.collect_spans(node, style, link, cell, out)
                }
                NodeValue::Strong => {
                    let style = SpanStyle {
                        strong: true,
                        ..style
                    };
                    self.collect_spans(node, style, link, cell, out)
                }
                NodeValue::Strikethrough => {
                    let style = SpanStyle {
                        strike: true,
                        ..style
                    };
                    self.collect_spans(node, style, link, cell, out)
                }
                // Images are not loaded; the alt text links to the image unless
                // an enclosing link already applies, as with linked badges.
                NodeValue::Link(target) | NodeValue::Image(target) => {
                    let link = Some(link.unwrap_or(&target.url));
                    self.collect_spans(node, style, link, cell, out)
                }
                _ => self.collect_spans(node, style, link, cell, out),
            }
        }
    }

    /// Maps a code span's text to the source between its backticks, with
    /// the space comrak strips from each end and line endings read as
    /// spaces.
    fn code_span(
        &self,
        literal: &str,
        backticks: usize,
        source: Range<usize>,
        cell: Option<&Cell>,
        out: &mut Inline,
    ) {
        let lex = Lex {
            pipes: cell.is_some(),
            newline_space: true,
            ..Lex::EXACT
        };
        let inner = source.start + backticks..source.end.saturating_sub(backticks);
        let padded = self.source.get(inner.clone()).is_some_and(|inner| {
            inner.len() >= 2
                && [inner.as_bytes()[0], inner.as_bytes()[inner.len() - 1]]
                    .iter()
                    .all(|c| matches!(c, b' ' | b'\n'))
        });
        let stripped = inner.start + 1..inner.end - 1;
        let followed = out.pieces.follows(literal, inner, self.source, lex)
            || padded && out.pieces.follows(literal, stripped, self.source, lex);
        if !followed {
            out.pieces.whole(literal, source, self.source);
        }
    }

    /// The pieces of a code or HTML block's `literal`, whose lines are the
    /// ends of the source's lines from the 1-based `first`, kept to the
    /// block's `len` text bytes. A line that isn't the end of its source
    /// line, such as one with a tab expanded to spaces, maps whole.
    fn lines(&self, literal: &str, first: usize, block: &Range<usize>, len: usize) -> Vec<Piece> {
        if !self.mapped {
            return Vec::new();
        }
        let mut pieces = Pieces::default();
        for (ix, line) in literal.split_inclusive('\n').enumerate() {
            let content = line.strip_suffix('\n').unwrap_or(line);
            let Some(start) = (first + ix)
                .checked_sub(1)
                .and_then(|ix| self.lines.get(ix))
            else {
                return vec![Piece {
                    text: 0..len,
                    source: block.clone(),
                    exact: false,
                }];
            };
            let ending = self
                .lines
                .get(first + ix)
                .copied()
                .unwrap_or(self.source.len());
            let end = if self.source[..ending].ends_with('\n') {
                ending - 1
            } else {
                ending
            };
            if self.source[*start..end].ends_with(content) {
                pieces.push(content.len(), end - content.len()..end, true);
            } else {
                let start = (*start).max(block.start).min(end);
                pieces.push(content.len(), start..end, false);
            }
            if line.ends_with('\n') {
                pieces.whole("\n", end..ending, self.source);
            }
        }
        pieces.clip(len);
        pieces.pieces
    }

    /// The synthesized paragraph for an alert marker with a title, mapped
    /// to the marker's line from its `[!`.
    fn marker(&self, marker: &str, alert: &Range<usize>) -> Node {
        if !self.mapped {
            return Node::default();
        }
        let line_end = self.source[alert.start..]
            .find('\n')
            .map_or(self.source.len(), |end| alert.start + end);
        let line = &self.source[alert.start..line_end];
        let start = alert.start + line.find("[!").unwrap_or(0);
        let source = start..alert.start + line.trim_end().len();
        let mut pieces = Pieces::default();
        pieces.lexed(marker, source.clone(), self.source, Lex::EXACT);
        Node {
            source,
            pieces: pieces.pieces,
            ..Node::default()
        }
    }

    /// The byte range of `node` in the source.
    fn range<'a>(&self, node: &'a AstNode<'a>) -> Range<usize> {
        if !self.mapped {
            return 0..0;
        }
        self.position_range(node.data().sourcepos)
    }

    /// The byte range of `position` in the source.
    fn position_range(&self, position: Sourcepos) -> Range<usize> {
        let offset = |line: usize, column: usize| {
            line.checked_sub(1)
                .and_then(|ix| self.lines.get(ix))
                .map_or(self.source.len(), |start| start + column)
                .min(self.source.len())
        };
        let start = offset(position.start.line, position.start.column.saturating_sub(1));
        // The end column is the last byte of the node's last character.
        let mut end = offset(position.end.line, position.end.column).max(start);
        while !self.source.is_char_boundary(end) {
            end += 1;
        }
        start..end
    }

    /// The byte range of an inline node, past the escaped pipes that
    /// comrak's inline positions in a table cell don't count.
    fn inline_range<'a>(&self, node: &'a AstNode<'a>, cell: Option<&Cell>) -> Range<usize> {
        let range = self.range(node);
        match cell {
            Some(cell) => cell.start(range.start)..cell.end(range.end),
            None => range,
        }
    }

    /// What a character reference such as `&amp;` stands for, as comrak
    /// decodes it.
    fn decode(&self, reference: &str) -> Option<String> {
        self.entities
            .borrow_mut()
            .entry(reference.to_owned())
            .or_insert_with(|| {
                let arena = Arena::new();
                let root = parse_document(&arena, reference, &Options::default());
                let paragraph = root.first_child()?;
                let text = paragraph.first_child()?;
                if text.next_sibling().is_some() {
                    return None;
                }
                match &text.data().value {
                    NodeValue::Text(text) => Some(text.to_string()),
                    _ => None,
                }
            })
            .clone()
    }
}

/// A table cell's content with escaped pipes. Comrak removes the `\` of
/// each `\|` before parsing the cell's inlines, and their positions count
/// from the content's start without it.
struct Cell {
    /// Where the content starts in the source.
    content: usize,
    /// Source offset of each content byte that comrak keeps, then the
    /// content's end.
    kept: Vec<usize>,
}

impl Cell {
    fn new(source: &str, cell: &Range<usize>) -> Option<Cell> {
        let text = source.get(cell.clone())?;
        if !text.contains("\\|") {
            return None;
        }
        let content = cell.start + (text.len() - text.trim_start_matches([' ', '\t']).len());
        let bytes = source.as_bytes();
        let mut kept = Vec::new();
        let mut ix = content;
        while ix < cell.end {
            if bytes[ix] == b'\\' && ix + 1 < cell.end {
                if bytes[ix + 1] != b'|' {
                    kept.push(ix);
                }
                kept.push(ix + 1);
                ix += 2;
            } else {
                kept.push(ix);
                ix += 1;
            }
        }
        kept.push(cell.end);
        Some(Cell { content, kept })
    }

    fn start(&self, reported: usize) -> usize {
        match reported.checked_sub(self.content) {
            Some(ix) => self.kept[ix.min(self.kept.len() - 1)],
            None => reported,
        }
    }

    fn end(&self, reported: usize) -> usize {
        match reported.checked_sub(self.content) {
            Some(0) | None => reported,
            Some(ix) if ix < self.kept.len() => self.kept[ix - 1] + 1,
            Some(_) => self.kept[self.kept.len() - 1],
        }
    }
}

fn alignment(alignment: &TableAlignment) -> Alignment {
    match alignment {
        TableAlignment::None => Alignment::None,
        TableAlignment::Left => Alignment::Left,
        TableAlignment::Center => Alignment::Center,
        TableAlignment::Right => Alignment::Right,
    }
}

fn is_html_comment(html: &str) -> bool {
    html.trim_start().starts_with("<!--")
}

fn plain(text: String) -> Span {
    Span {
        text,
        style: SpanStyle::default(),
        link: None,
    }
}

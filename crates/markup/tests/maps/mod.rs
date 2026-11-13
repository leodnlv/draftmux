//! Reading source maps in tests: each target's mapped parse, the text of
//! every node, and the check that every piece is the source it claims.

#![allow(dead_code)]

use std::ops::Range;
use std::path::{Path, PathBuf};

use markup::source_map::{Fragment, Node, SourceMap};
use markup::{Block, Document, Span, frontmatter, github, plain, rich_text, slack};

/// What a mapped parse is for: a preview target, or what Slack posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Github,
    Slack,
    /// [`slack::posted_mapped`] of the Slack parse.
    Posted,
    Rich,
    Plain,
}

pub const TARGETS: [Target; 5] = [
    Target::Github,
    Target::Slack,
    Target::Posted,
    Target::Rich,
    Target::Plain,
];

impl Target {
    /// `source` parsed and mapped for this target.
    pub fn mapped(self, source: &str) -> (Document, SourceMap) {
        match self {
            Target::Github => github::parse_mapped(source),
            Target::Slack => slack::parse_mapped(source),
            Target::Posted => {
                let (doc, map) = slack::parse_mapped(source);
                let (posted, map) = slack::posted_mapped(&doc, &map);
                let blocks = posted.into_iter().map(|(block, _)| block).collect();
                (Document { blocks }, map)
            }
            Target::Rich => {
                let (doc, map) = github::parse_mapped(source);
                rich_text::pasted_mapped(&doc, &map)
            }
            Target::Plain => plain::parse_mapped(source),
        }
    }

    /// `source` parsed for this target without a map.
    pub fn parse(self, source: &str) -> Document {
        match self {
            Target::Github => github::parse(source),
            Target::Slack => slack::parse(source),
            Target::Posted => Document {
                blocks: slack::posted(&slack::parse(source))
                    .into_iter()
                    .map(|(block, _)| block)
                    .collect(),
            },
            Target::Rich => rich_text::pasted(&github::parse(source)),
            Target::Plain => plain::parse(source),
        }
    }

    /// Whether the target adds text with no source: an alert's label, a
    /// task's box, list markers or a table's layout.
    fn adds_text(self) -> bool {
        matches!(self, Target::Rich | Target::Posted)
    }
}

/// The text of a node: its spans' text, or a code or HTML block's string.
pub fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// Each block of `blocks` beside its node, and the text of every node in
/// the tree by path; fails where the map's shape isn't the document's.
pub fn texts(
    blocks: &[Block],
    nodes: &[Node],
    path: &mut Vec<usize>,
    out: &mut Vec<(Vec<usize>, String)>,
) {
    assert_eq!(blocks.len(), nodes.len(), "blocks at {path:?}");
    for (ix, (block, node)) in blocks.iter().zip(nodes).enumerate() {
        path.push(ix);
        let leaf = |text: String, out: &mut Vec<_>| {
            assert!(node.children.is_empty(), "children at {path:?}");
            out.push((path.clone(), text));
        };
        match block {
            Block::Paragraph(spans) | Block::Heading { spans, .. } => leaf(spans_text(spans), out),
            Block::Code { text, .. } | Block::Html(text) => leaf(text.clone(), out),
            Block::Rule => leaf(String::new(), out),
            Block::Quote(blocks) | Block::Alert { blocks, .. } => {
                assert!(node.pieces.is_empty());
                texts(blocks, &node.children, path, out);
            }
            Block::List(list) => {
                assert_eq!(list.items.len(), node.children.len(), "items at {path:?}");
                for (ix, (item, child)) in list.items.iter().zip(&node.children).enumerate() {
                    path.push(ix);
                    texts(&item.blocks, &child.children, path, out);
                    path.pop();
                }
            }
            Block::Table(table) => {
                let rows: Vec<_> = std::iter::once(&table.header).chain(&table.rows).collect();
                assert_eq!(rows.len(), node.children.len(), "rows at {path:?}");
                for (ix, (row, row_node)) in rows.iter().zip(&node.children).enumerate() {
                    assert_eq!(row.len(), row_node.children.len(), "cells at {path:?}/{ix}");
                    for (cell, (col, cell_node)) in
                        row.iter().zip(row_node.children.iter().enumerate())
                    {
                        assert!(cell_node.children.is_empty());
                        out.push(([path.as_slice(), &[ix, col]].concat(), spans_text(cell)));
                    }
                }
            }
        }
        path.pop();
    }
}

pub fn text_at(doc: &Document, map: &SourceMap, path: &[usize]) -> String {
    let mut all = Vec::new();
    texts(&doc.blocks, &map.blocks, &mut Vec::new(), &mut all);
    all.into_iter()
        .find(|(at, _)| at == path)
        .map(|(_, text)| text)
        .unwrap_or_else(|| panic!("no text at {path:?}"))
}

/// What `locate` finds for `range` on `target`, as each fragment's path
/// and text.
pub fn shown(target: Target, source: &str, range: Range<usize>) -> Vec<(Vec<usize>, String)> {
    let (doc, map) = target.mapped(source);
    map.locate(range)
        .into_iter()
        .map(|Fragment { path, text }| {
            let shown = text_at(&doc, &map, &path)[text].to_owned();
            (path, shown)
        })
        .collect()
}

pub fn range_of(source: &str, needle: &str) -> Range<usize> {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} in {source:?}"));
    start..start + needle.len()
}

/// Every pair of review fixtures in `dir`, by name.
pub fn review_pairs(dir: &Path) -> Vec<(String, String, String)> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".before.md").map(str::to_owned)
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let read = |side: &str| {
                std::fs::read_to_string(dir.join(format!("{name}.{side}.md"))).unwrap()
            };
            let (before, after) = (read("before"), read("after"));
            (name, before, after)
        })
        .collect()
}

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
}

/// The markdown fixtures of this crate and the review fixtures, by name:
/// each file whole, and its body after any frontmatter as the preview
/// parses it.
pub fn corpus() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    markdown_files(&root.join("tests/fixtures"), &mut files);
    markdown_files(&root.join("../draftmux/fixtures/review"), &mut files);
    files.sort();
    assert!(files.len() > 100, "{} fixtures", files.len());
    let mut out = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        let name = file.strip_prefix(root).unwrap().display().to_string();
        let body = frontmatter::parse(&text).body.to_owned();
        out.push((format!("{name} (body)"), body));
        out.push((name, text));
    }
    out
}

/// Checks the map of `source` on `target`: the mapped document is the
/// unmapped one, pieces follow each other inside their node's text and
/// block, and every exact piece's source is its text. Pieces cover each
/// node's text, except text the target adds, which must be one of the
/// texts it can add. Returns how many pieces were checked.
pub fn check(target: Target, name: &str, source: &str, failures: &mut Vec<String>) -> usize {
    let (doc, map) = target.mapped(source);
    assert_eq!(doc, target.parse(source), "{target:?} {name}");
    let mut nodes = Vec::new();
    texts(&doc.blocks, &map.blocks, &mut Vec::new(), &mut nodes);
    let mut checked = 0;
    for (path, text) in nodes {
        let node = map.node(&path);
        let at = format!("{target:?} {name} {path:?}");
        let mut gaps = Vec::new();
        let mut covered = 0;
        for piece in &node.pieces {
            checked += 1;
            let at = format!("{at} {piece:?}");
            if piece.text.is_empty() || piece.text.start < covered {
                failures.push(format!("{at}: doesn't follow {covered}"));
            }
            if piece.text.start > covered {
                gaps.push(
                    text.get(covered..piece.text.start)
                        .unwrap_or("?")
                        .to_owned(),
                );
            }
            covered = covered.max(piece.text.end);
            let Some(slice) = source.get(piece.source.clone()) else {
                failures.push(format!("{at}: not in the source"));
                continue;
            };
            if piece.source.start < node.source.start || piece.source.end > node.source.end {
                failures.push(format!("{at}: outside its block {:?}", node.source));
            }
            if piece.exact && text.get(piece.text.clone()) != Some(slice) {
                failures.push(format!(
                    "{at}: {slice:?} isn't {:?}",
                    text.get(piece.text.clone())
                ));
            }
            if !piece.exact && slice.is_empty() {
                failures.push(format!("{at}: no source"));
            }
        }
        if covered < text.len() {
            gaps.push(text[covered..].to_owned());
        }
        for gap in gaps {
            if !target.adds_text() || !added(target, &gap) {
                failures.push(format!("{at}: {gap:?} of {text:?} has no piece"));
            }
        }
    }
    checked
}

/// Whether `gap` is text `target` adds.
fn added(target: Target, gap: &str) -> bool {
    match target {
        Target::Rich => {
            ["Note", "Tip", "Important", "Warning", "Caution", "☐ ", "☑ "].contains(&gap)
        }
        // List markers and their indentation, line ends, and a table's
        // padding, separators and rule.
        Target::Posted => gap
            .chars()
            .all(|c| c.is_ascii_digit() || " \n•.-+|".contains(c)),
        _ => false,
    }
}

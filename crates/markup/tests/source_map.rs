//! Source maps of the GitHub parse: every exact piece is its source, and
//! a change in the source finds the rendered text that shows it.

use std::ops::Range;
use std::path::{Path, PathBuf};

use markup::source_map::{Fragment, Node, SourceMap};
use markup::{Block, Document, Span, frontmatter, github};

/// The text of a node: its spans' text, or a code or HTML block's string.
fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// Each block of `blocks` beside its node, and the text of every node in
/// the tree by path; fails where the map's shape isn't the document's.
fn texts(
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

fn text_at(doc: &Document, map: &SourceMap, path: &[usize]) -> String {
    let mut all = Vec::new();
    texts(&doc.blocks, &map.blocks, &mut Vec::new(), &mut all);
    all.into_iter()
        .find(|(at, _)| at == path)
        .map(|(_, text)| text)
        .unwrap_or_else(|| panic!("no text at {path:?}"))
}

/// What `locate` finds for `range`, as each fragment's path and text.
fn shown(source: &str, range: Range<usize>) -> Vec<(Vec<usize>, String)> {
    let (doc, map) = github::parse_mapped(source);
    map.locate(range)
        .into_iter()
        .map(|Fragment { path, text }| {
            let shown = text_at(&doc, &map, &path)[text].to_owned();
            (path, shown)
        })
        .collect()
}

fn range_of(source: &str, needle: &str) -> Range<usize> {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} in {source:?}"));
    start..start + needle.len()
}

/// Every pair of review fixtures in `dir`, by name.
fn review_pairs(dir: &Path) -> Vec<(String, String, String)> {
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

const SYNTAX_EDITS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/review");

#[test]
fn every_text_piece_reproduces_its_literal() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    markdown_files(&root.join("tests/fixtures"), &mut files);
    markdown_files(&root.join("../draftmux/fixtures/review"), &mut files);
    files.sort();
    assert!(files.len() > 100, "{} fixtures", files.len());
    let mut failures = Vec::new();
    let mut checked = 0;
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        // The preview parses the body after any frontmatter.
        let body = frontmatter::parse(&text).body;
        for source in [text.as_str(), body] {
            let name = file.strip_prefix(root).unwrap().display();
            let (doc, map) = github::parse_mapped(source);
            assert_eq!(doc, github::parse(source), "{name}");
            let mut nodes = Vec::new();
            texts(&doc.blocks, &map.blocks, &mut Vec::new(), &mut nodes);
            for (path, text) in nodes {
                let node = map.node(&path);
                let mut covered = 0;
                for piece in &node.pieces {
                    checked += 1;
                    let at = format!("{name} {path:?} {piece:?}");
                    if piece.text.start != covered || piece.text.is_empty() {
                        failures.push(format!("{at}: doesn't follow {covered}"));
                    }
                    covered = piece.text.end;
                    let Some(slice) = source.get(piece.source.clone()) else {
                        failures.push(format!("{at}: not in the source"));
                        continue;
                    };
                    if piece.source.start < node.source.start || piece.source.end > node.source.end
                    {
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
                if covered != text.len() {
                    failures.push(format!("{name} {path:?}: covers {covered} of {:?}", text));
                }
            }
        }
    }
    assert!(checked > 1000, "{checked} pieces");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn plain_paragraph_ranges_map_one_to_one() {
    let source = "First paragraph.\n\nSecond one\nwraps here.\n";
    let (doc, map) = github::parse_mapped(source);
    assert_eq!(doc, github::parse(source));
    for (needle, path) in [("paragraph", vec![0]), ("one\nwraps", vec![1])] {
        assert_eq!(
            shown(source, range_of(source, needle)),
            [(path, needle.to_owned())]
        );
    }
    assert_eq!(
        map.blocks[1].source,
        range_of(source, "Second one\nwraps here.")
    );
}

#[test]
fn entity_and_escape_ranges_map_to_their_rendered_characters() {
    let source = "Tom &amp; Jerry &copy; \\* end\n";
    assert_eq!(
        shown(source, range_of(source, "Jerry")),
        [(vec![0], "Jerry".into())]
    );
    // A reference or escape maps whole, even from part of it.
    assert_eq!(
        shown(source, range_of(source, "amp")),
        [(vec![0], "&".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "&copy;")),
        [(vec![0], "©".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "\\*")),
        [(vec![0], "*".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "y &copy; \\*")),
        [(vec![0], "y © *".into())]
    );
}

#[test]
fn syntax_only_changes_map_to_a_zero_width_anchor() {
    let source = "## Title\n\nSee [the docs](https://example.com/one) now.\n";
    let anchor = |range: Range<usize>| {
        let (_, map) = github::parse_mapped(source);
        map.locate(range)
    };
    // A heading's marker shows before the heading's text.
    assert_eq!(
        anchor(range_of(source, "#")),
        [Fragment {
            path: vec![0],
            text: 0..0
        }]
    );
    // A link's URL shows after the text before it in the paragraph.
    let url = range_of(source, "one");
    let at = "See the docs".len();
    assert_eq!(
        anchor(url.clone()),
        [Fragment {
            path: vec![1],
            text: at..at
        }]
    );
    // So does a point there.
    assert_eq!(
        anchor(url.start..url.start),
        [Fragment {
            path: vec![1],
            text: at..at
        }]
    );
    // A point at a word's start shows before it.
    let now = range_of(source, "now").start;
    let at = "See the docs ".len();
    assert_eq!(
        anchor(now..now),
        [Fragment {
            path: vec![1],
            text: at..at
        }]
    );
    // A point between blocks shows after the text before it.
    let gap = range_of(source, "\n\n").start + 1;
    assert_eq!(
        anchor(gap..gap),
        [Fragment {
            path: vec![0],
            text: 5..5
        }]
    );
    // An empty document has nowhere to show one.
    assert_eq!(github::parse_mapped("").1.locate(0..0), []);
}

#[test]
fn code_block_ranges_map_inside_the_block() {
    let source = "> ```rust\n> fn main() {\n>     run();\n> }\n> ```\n\n    indented\n";
    assert_eq!(
        shown(source, range_of(source, "run")),
        [(vec![0, 0], "run".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "{\n>     run")),
        [(vec![0, 0], "{\n    run".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "indented")),
        [(vec![1], "indented".into())]
    );
    // The fence's info string renders no text.
    assert_eq!(
        github::parse_mapped(source)
            .1
            .locate(range_of(source, "rust")),
        [Fragment {
            path: vec![0, 0],
            text: 0..0
        }]
    );
    // A tab partly used as indentation renders as spaces that aren't in the
    // source, so its line maps whole.
    let source = ">\t\tfoo\n";
    assert_eq!(
        shown(source, range_of(source, "fo")),
        [(vec![0, 0], "  foo".into())]
    );
}

#[test]
fn table_cell_with_escaped_pipe_maps_exactly() {
    let source = "| a | b |\n|---|---|\n| y \\| z *w* \\| `p \\| q` | after |\n";
    let cell = vec![0, 1, 0];
    assert_eq!(
        shown(source, range_of(source, "z")),
        [(cell.clone(), "z".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "w")),
        [(cell.clone(), "w".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "q")),
        [(cell.clone(), "q".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "\\|")),
        [(cell.clone(), "|".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "after")),
        [(vec![0, 1, 1], "after".into())]
    );
    let (_, map) = github::parse_mapped(source);
    assert!(
        map.node(&cell)
            .pieces
            .iter()
            .filter(|piece| piece.exact)
            .count()
            >= 4
    );
}

#[test]
fn autolink_and_www_link_map_to_their_source() {
    let source = "Go to https://example.com/one, www.example.org or <https://b.example>.\n";
    for needle in ["one", "www.example.org", "b.example"] {
        assert_eq!(
            shown(source, range_of(source, needle)),
            [(vec![0], needle.to_owned())]
        );
    }
    // The angle brackets render nothing; their point is the link's start.
    let bracket = range_of(source, "<");
    let (_, map) = github::parse_mapped(source);
    let at = "Go to https://example.com/one, www.example.org or ".len();
    assert_eq!(
        map.locate(bracket),
        [Fragment {
            path: vec![0],
            text: at..at
        }]
    );
}

/// The one change between two texts, as a range on each side.
fn change(before: &str, after: &str) -> (Range<usize>, Range<usize>) {
    let prefix = before
        .char_indices()
        .zip(after.chars())
        .find(|((_, a), b)| a != b)
        .map_or(before.len().min(after.len()), |((ix, _), _)| ix);
    let suffix = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    (prefix..before.len() - suffix, prefix..after.len() - suffix)
}

/// Review pairs whose change is in syntax that a mark written into the
/// text would break: an autolink's URL, a hard break's spaces, a link
/// reference definition and a setext underline.
#[test]
fn syntax_edits_show_their_change_on_both_sides() {
    let pairs = review_pairs(Path::new(SYNTAX_EDITS));
    let names: Vec<_> = pairs.iter().map(|(name, ..)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "autolink-edited",
            "hard-break-removed",
            "reference-definition",
            "setext-underline"
        ]
    );
    let mut seen = Vec::new();
    for (name, before, after) in &pairs {
        let (from, to) = change(before, after);
        seen.push((
            name.as_str(),
            shown(before, from)
                .into_iter()
                .map(|(_, text)| text)
                .collect::<Vec<_>>(),
            shown(after, to)
                .into_iter()
                .map(|(_, text)| text)
                .collect::<Vec<_>>(),
        ));
    }
    let both = |text: &str| vec![text.to_owned()];
    assert_eq!(
        seen,
        [
            // The edited URL is the link's text.
            ("autolink-edited", both("one"), both("two")),
            // The removed spaces are the hard break, shown as the line end
            // it was; the soft break left behind gets the point.
            ("hard-break-removed", both("\n"), both("")),
            // The changed definition renders nothing; its point follows
            // the paragraph before it.
            ("reference-definition", both(""), both("")),
            // The changed underline renders nothing; its point follows the
            // heading's text.
            ("setext-underline", both(""), both("")),
        ]
    );
    // Each shows in the first block, the only one with text.
    for (name, before, after) in &pairs {
        let (from, to) = change(before, after);
        for (side, source, range) in [("before", before, from), ("after", after, to)] {
            let found = github::parse_mapped(source).1.locate(range);
            assert_eq!(found.len(), 1, "{name} {side}");
            assert_eq!(found[0].path, [0], "{name} {side}");
        }
    }
}

#[test]
fn syntax_only_hunk_anchors_to_its_block() {
    let dir = Path::new(SYNTAX_EDITS);
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
    // The underline is in the heading's source; its point ends the heading.
    let setext = read("setext-underline.before.md");
    let (_, map) = github::parse_mapped(&setext);
    let underline = range_of(&setext, "=====");
    assert!(map.blocks[0].source.contains(&underline.start));
    assert_eq!(
        map.locate(underline),
        [Fragment {
            path: vec![0],
            text: 5..5
        }]
    );
    // A definition isn't a block; its point ends the block before it.
    let definition = read("reference-definition.before.md");
    let (_, map) = github::parse_mapped(&definition);
    assert_eq!(map.blocks.len(), 1);
    let end = "Read the guide first.".len();
    assert_eq!(
        map.locate(range_of(&definition, "one")),
        [Fragment {
            path: vec![0],
            text: end..end
        }]
    );
    // Before any text, it starts the first block.
    let first = "[guide]: https://example.com/one\n\nRead [the guide][guide] first.\n";
    let (_, map) = github::parse_mapped(first);
    assert_eq!(
        map.locate(range_of(first, "one")),
        [Fragment {
            path: vec![0],
            text: 0..0
        }]
    );
}

#[test]
fn a_wrapped_code_span_maps_exactly_around_its_line_end() {
    let source = "- Run `cargo test\n  --lib` now\n";
    let item = vec![0, 0, 0];
    assert_eq!(
        shown(source, range_of(source, "test")),
        [(item.clone(), "test".into())]
    );
    assert_eq!(
        shown(source, range_of(source, "lib")),
        [(item.clone(), "lib".into())]
    );
    // The line end and the next line's indentation render as one space.
    assert_eq!(shown(source, range_of(source, "\n ")), [(item, " ".into())]);
}

//! Source maps of the GitHub parse: every exact piece is its source, and
//! a change in the source finds the rendered text that shows it. The
//! corpus check covers every target.

use std::ops::Range;
use std::path::Path;

use markup::github;
use markup::source_map::Fragment;

mod maps;

use maps::{Target, check, corpus, range_of, review_pairs};

/// What `locate` finds for `range` in the GitHub parse.
fn shown(source: &str, range: Range<usize>) -> Vec<(Vec<usize>, String)> {
    maps::shown(Target::Github, source, range)
}

const SYNTAX_EDITS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/review");

#[test]
fn every_text_piece_reproduces_its_literal() {
    let mut failures = Vec::new();
    let mut checked = Vec::new();
    for target in maps::TARGETS {
        let mut count = 0;
        for (name, source) in corpus() {
            count += check(target, &name, &source, &mut failures);
            // Slack and plain text read `\r\n` as `\n`; their maps point
            // back at both bytes.
            if matches!(target, Target::Slack | Target::Posted | Target::Plain) {
                let crlf = source.replace('\n', "\r\n");
                count += check(target, &format!("{name} (crlf)"), &crlf, &mut failures);
            }
        }
        checked.push((target, count));
    }
    for (target, count) in checked {
        assert!(count > 1000, "{target:?}: {count} pieces");
    }
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

#[test]
fn frontmatter_ranges_map_to_fields() {
    let text =
        "---\nto: a@example.com\n# who else\ncc:\nsubject: |-\n  First\n\n  second\n---\nBody\n";
    let (draft, map) = markup::frontmatter::parse_mapped(text);
    let keys: Vec<_> = draft.fields.keys().map(String::as_str).collect();
    assert_eq!(keys, ["to", "cc", "subject"]);
    let field = |needle: &str| map.locate(range_of(text, needle));
    assert_eq!(field("example"), [0]);
    // A block scalar's lines, a blank one included, belong to its key.
    assert_eq!(field("First"), [2]);
    assert_eq!(field("second"), [2]);
    assert_eq!(field("cc:\nsubject"), [1, 2]);
    // Between fields, a change shows on the field before it.
    assert_eq!(field("# who"), [0]);
    // A point at a line's start belongs to the field ending before it.
    let cc = range_of(text, "cc").start;
    assert_eq!(map.locate(cc..cc), [1]);
    // The delimiters show on the nearest field.
    assert_eq!(map.locate(0..3), [0]);
    assert_eq!(field("---\nBody"), [2]);
    // The body isn't in a field.
    assert_eq!(map.end, draft.body_offset);
    assert_eq!(field("Body"), []);
    // Lines that aren't one per field map every field to the whole block.
    let flow = "---\n{a: 1, b: 2}\n---\n";
    let (draft, map) = markup::frontmatter::parse_mapped(flow);
    assert_eq!(draft.fields.len(), 2);
    assert_eq!(map.locate(range_of(flow, "2")), [0, 1]);
    // Without fields there is nothing to show.
    assert_eq!(markup::frontmatter::parse_mapped("Body").1.locate(0..4), []);
}

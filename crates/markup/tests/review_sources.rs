//! Source maps of the Slack, rich-text and plain-text targets: a change in
//! a draft finds the text each target shows for it.

use markup::source_map::Fragment;

mod maps;

use maps::{Target, range_of, shown};

/// The text `target` shows for `needle`, the first in `source`.
fn texts(target: Target, source: &str, needle: &str) -> Vec<String> {
    shown(target, source, range_of(source, needle))
        .into_iter()
        .map(|(_, text)| text)
        .collect()
}

#[test]
fn slack_ranges_map_through_tokens_and_lines() {
    let source = concat!(
        "Fish &amp; chips \\* :tada: for *you*\n",
        "see <https://x.example|the docs> or [site](https://y.example)\n",
        "\n",
        "> quoted\n",
        "> twice `a &lt; b`\n",
        "- item one\n",
        "    continued\n",
        "```\n",
        "code &gt; here\n",
        "```\n",
    );
    let slack = |needle: &str| shown(Target::Slack, source, range_of(source, needle));
    assert_eq!(slack("chips"), [(vec![0], "chips".into())]);
    // An entity, an escape and a shortcode map whole.
    assert_eq!(slack("amp"), [(vec![0], "&".into())]);
    assert_eq!(slack("\\*"), [(vec![0], "*".into())]);
    assert_eq!(slack("tad"), [(vec![0], "🎉".into())]);
    // Emphasis delimiters render nothing; the text inside maps exactly.
    assert_eq!(slack("you"), [(vec![0], "you".into())]);
    // Link labels map; their URLs are syntax.
    assert_eq!(slack("the docs"), [(vec![0], "the docs".into())]);
    assert_eq!(slack("site"), [(vec![0], "site".into())]);
    // The URL comes before the label it links.
    let label = "Fish & chips * 🎉 for you\nsee ".len();
    assert_eq!(
        Target::Slack
            .mapped(source)
            .1
            .locate(range_of(source, "x.example")),
        [Fragment {
            path: vec![0],
            text: label..label
        }]
    );
    // A paragraph's lines join with a `\n` from the source; the closing
    // `*` before it renders nothing.
    assert_eq!(slack("*\nsee"), [(vec![0], "\nsee".into())]);
    // A quote's next line joins past its `>`.
    assert_eq!(slack("\n> twice"), [(vec![1, 0], "\ntwice".into())]);
    assert_eq!(slack("lt;"), [(vec![1, 0], "<".into())]);
    // A list item's continuation joins past its indentation.
    assert_eq!(
        slack("one\n    cont"),
        [(vec![2, 0, 0], "one\ncont".into())]
    );
    // Code maps inside the block, its entities whole.
    assert_eq!(slack("here"), [(vec![3], "here".into())]);
    assert_eq!(slack("&gt;"), [(vec![3], ">".into())]);
}

#[test]
fn slack_tables_map_each_cell() {
    let source = "| Name | Count |\n| --- | ---: |\n| apples | 3 |\n| pears |\n";
    let slack = |needle: &str| shown(Target::Slack, source, range_of(source, needle));
    assert_eq!(slack("Count"), [(vec![0, 0, 1], "Count".into())]);
    assert_eq!(slack("apples"), [(vec![0, 1, 0], "apples".into())]);
    // A row missing a cell gets an empty one at its end.
    let (_, map) = Target::Slack.mapped(source);
    let end = range_of(source, "| pears |").end;
    assert_eq!(map.node(&[0, 2, 1]).source, end..end);
}

#[test]
fn slack_crlf_maps_back_to_both_bytes() {
    let source = "One &amp; two\r\nthree\r\n\r\n```\r\nfour\r\nfive\r\n```\r\n";
    for target in [Target::Slack, Target::Plain] {
        assert_eq!(texts(target, source, "three"), ["three"], "{target:?}");
        assert_eq!(texts(target, source, "\r\n"), ["\n"], "{target:?}");
    }
    assert_eq!(texts(Target::Slack, source, "four\r\nfive"), ["four\nfive"]);
    // Half of a `\r\n` is all of its `\n`.
    let cr = range_of(source, "\r").start;
    assert_eq!(
        shown(Target::Slack, source, cr..cr + 1),
        [(vec![0], "\n".into())]
    );
}

#[test]
fn posted_slack_maps_tables_and_list_lines() {
    let source = "# Plan\n\n| Name | Count |\n| --- | ---: |\n| apples | 3 |\n\n- first\n  wrapped\n    - nested\n";
    let posted = |needle: &str| shown(Target::Posted, source, range_of(source, needle));
    // A heading posts as a bold line with the same text.
    assert_eq!(posted("Plan"), [(vec![0], "Plan".into())]);
    // Slack keeps blank lines between blocks as a paragraph, so the table
    // is the third block. It posts as code, each cell where the layout
    // puts it.
    assert_eq!(posted("apples"), [(vec![2], "apples".into())]);
    assert_eq!(posted("3"), [(vec![2], "3".into())]);
    let (doc, map) = Target::Posted.mapped(source);
    assert_eq!(
        maps::text_at(&doc, &map, &[2]),
        "Name   | Count\n-------+------\napples |     3"
    );
    // A list posts as text lines after their markers.
    assert_eq!(posted("first"), [(vec![4], "first".into())]);
    assert_eq!(posted("nested"), [(vec![4], "nested".into())]);
    let text = maps::text_at(&doc, &map, &[4]);
    assert_eq!(text, "• first\nwrapped\n    • nested");
    // A marker's change points at the item's text.
    let marker = range_of(source, "- first").start;
    assert_eq!(
        map.locate(marker..marker + 1),
        [Fragment {
            path: vec![4],
            text: 4..4
        }]
    );
}

#[test]
fn rich_text_maps_through_pasting() {
    let source = "> [!NOTE]\n> Mind the gap.\n\n- [ ] task one\n- [x] done\n\n<div>raw</div>\n";
    let rich = |needle: &str| shown(Target::Rich, source, range_of(source, needle));
    // The alert's text follows its label, which has no source.
    assert_eq!(rich("gap"), [(vec![0, 1], "gap".into())]);
    assert_eq!(
        Target::Rich
            .mapped(source)
            .1
            .locate(range_of(source, "NOTE")),
        [Fragment {
            path: vec![0, 1],
            text: 0..0
        }]
    );
    // A task's text follows its box.
    assert_eq!(rich("task"), [(vec![1, 0, 0], "task".into())]);
    let (doc, map) = Target::Rich.mapped(source);
    assert_eq!(maps::text_at(&doc, &map, &[1, 1, 0]), "☑ done");
    assert_eq!(rich("done"), [(vec![1, 1, 0], "done".into())]);
    // Raw HTML pastes as its own text.
    assert_eq!(rich("raw"), [(vec![2], "raw".into())]);
}

#[test]
fn plain_text_maps_one_to_one() {
    let source = "Hi **Sam**,\n\n&amp; more\n\n\n";
    assert_eq!(texts(Target::Plain, source, "**Sam**"), ["**Sam**"]);
    assert_eq!(texts(Target::Plain, source, "&amp;"), ["&amp;"]);
    // The trailing line breaks don't show; a change there points at the
    // end of the text.
    let (_, map) = Target::Plain.mapped(source);
    let end = source.trim_end().len();
    assert_eq!(
        map.locate(end + 1..end + 2),
        [Fragment {
            path: vec![0],
            text: end..end
        }]
    );
}

/// Noncharacters in a draft are its own text: every target shows them as
/// written, mapped exactly.
#[test]
fn literal_noncharacters_render_as_written() {
    let source = "Keep \u{FDD0}this\u{FDD3} and `\u{FDEF}`\n";
    for target in maps::TARGETS {
        let (doc, map) = target.mapped(source);
        let text = maps::text_at(&doc, &map, &[0]);
        assert!(
            text.contains("\u{FDD0}this\u{FDD3}"),
            "{target:?}: {text:?}"
        );
        assert!(text.contains('\u{FDEF}'), "{target:?}: {text:?}");
        for needle in ["\u{FDD0}", "this\u{FDD3}", "\u{FDEF}"] {
            assert_eq!(texts(target, source, needle), [needle], "{target:?}");
        }
    }
}

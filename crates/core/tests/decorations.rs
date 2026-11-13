//! Decorations from a comparison, found in what each target renders: every
//! change shows on both sides.

use std::ops::Range;
use std::path::Path;

use draftmux_core::decorations::{Decoration, DecorationKind, Side, decorations};
use draftmux_core::diff::compare;
use markup::source_map::{Fragment, SourceMap};
use markup::{frontmatter, github, plain, rich_text, slack};

#[derive(Debug, Clone, Copy)]
enum Target {
    Github,
    /// What Slack posts, which its preview shows.
    Slack,
    Rich,
    Plain,
}

/// The body's map for `target`.
fn body_map(target: Target, body: &str) -> SourceMap {
    match target {
        Target::Github => github::parse_mapped(body).1,
        Target::Slack => {
            let (doc, map) = slack::parse_mapped(body);
            slack::posted_mapped(&doc, &map).1
        }
        Target::Rich => {
            let (doc, map) = github::parse_mapped(body);
            rich_text::pasted_mapped(&doc, &map).1
        }
        Target::Plain => plain::parse_mapped(body).1,
    }
}

/// Where a decoration shows in one side's preview.
#[derive(Debug, PartialEq, Eq)]
enum Shown {
    /// A field of the frontmatter, by index.
    Field(usize),
    Body(Fragment),
}

/// What `text`, previewed on `target`, shows for `range`: fields for the
/// part in frontmatter, fragments for the part in the body.
fn shown(target: Target, text: &str, range: Range<usize>) -> Vec<Shown> {
    let (draft, fields) = frontmatter::parse_mapped(text);
    let mut out: Vec<Shown> = fields
        .locate(range.clone())
        .into_iter()
        .map(Shown::Field)
        .collect();
    let offset = draft.body_offset;
    if range.end > offset || range.is_empty() && range.start >= offset {
        let body = range.start.max(offset) - offset..range.end.max(offset) - offset;
        let map = body_map(target, draft.body);
        out.extend(map.locate(body).into_iter().map(Shown::Body));
    }
    out
}

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

#[test]
fn decorations_cover_every_hunk_on_both_sides() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pairs = review_pairs(&root.join("../draftmux/fixtures/review"));
    assert!(pairs.len() > 40, "{} pairs", pairs.len());
    let mut failures = Vec::new();
    for (name, before, after) in &pairs {
        let comparison = compare(before, after);
        let all = decorations(&comparison);
        for target in [Target::Github, Target::Slack, Target::Rich, Target::Plain] {
            for (side, text) in [(Side::Before, before), (Side::After, after)] {
                let side_has_text = !shown(target, text, 0..text.len()).is_empty();
                for hunk in 0..comparison.hunks.len() {
                    let mine: Vec<&Decoration> = all
                        .iter()
                        .filter(|decoration| decoration.side == side && decoration.hunk == hunk)
                        .collect();
                    let at = format!("{name} {target:?} {side:?} hunk {hunk}");
                    if mine.is_empty() {
                        failures.push(format!("{at}: no decoration"));
                        continue;
                    }
                    let found = mine.iter().any(|decoration| {
                        !shown(target, text, decoration.range.clone()).is_empty()
                    });
                    // A side with no text shows a point on its own.
                    let standalone = !side_has_text
                        && mine
                            .iter()
                            .any(|decoration| decoration.kind == DecorationKind::Point);
                    if !found && !standalone {
                        failures.push(format!("{at}: shows nowhere"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn an_empty_side_gets_a_point_anchor() {
    let comparison = compare("", "Hello\n");
    let before: Vec<_> = decorations(&comparison)
        .into_iter()
        .filter(|decoration| decoration.side == Side::Before)
        .collect();
    assert_eq!(
        before,
        [Decoration {
            side: Side::Before,
            hunk: 0,
            range: 0..0,
            kind: DecorationKind::Point
        }]
    );
    // The empty side renders no text to anchor it to, so it stands alone.
    for target in [Target::Github, Target::Slack, Target::Rich, Target::Plain] {
        assert_eq!(shown(target, "", 0..0), [], "{target:?}");
    }
    // Removing everything leaves a point on the new side.
    let comparison = compare("Hello\n", "");
    let after: Vec<_> = decorations(&comparison)
        .into_iter()
        .filter(|decoration| decoration.side == Side::After)
        .map(|decoration| (decoration.range, decoration.kind))
        .collect();
    assert_eq!(after, [(0..0, DecorationKind::Point)]);
}

#[test]
fn a_frontmatter_change_shows_on_its_field() {
    let before = "---\ntitle: Old plan\nto: team\n---\nBody\n";
    let after = "---\ntitle: New plan\nto: team\n---\nBody\n";
    let comparison = compare(before, after);
    for decoration in decorations(&comparison) {
        let text = match decoration.side {
            Side::Before => before,
            Side::After => after,
        };
        assert_eq!(
            shown(Target::Github, text, decoration.range),
            [Shown::Field(0)]
        );
    }
}

//! Structural snapshots of the block tree produced by the GitHub parser.

mod common;

use common::outline;
use markup::github;

fn snapshot(markdown: &str) -> String {
    outline(&github::parse(markdown))
}

#[test]
fn headings() {
    insta::assert_snapshot!(snapshot(
        "# One\n## Two\n### Three\n#### Four\n##### Five\n###### Six\n\nSetext\n===\n"
    ));
}

#[test]
fn inline_styles() {
    insta::assert_snapshot!(snapshot(
        "plain **bold** *italic* ***both*** ~~gone~~ `code` **bold `code` inside**\n"
    ));
}

#[test]
fn line_breaks() {
    insta::assert_snapshot!(snapshot(
        "soft\nbreak\n\nhard  \nbreak\n\nbackslash\\\nbreak\n"
    ));
}

#[test]
fn file_line_breaks() {
    assert_eq!(
        github::parse_file("soft\nbreak\n\n- wrapped\n  item\n"),
        github::parse("soft break\n\n- wrapped item\n")
    );
    let hard = "hard  \nbreak\n\nbackslash\\\nbreak\n";
    assert_eq!(github::parse_file(hard), github::parse(hard));
    assert_eq!(
        github::parse_file_mapped("soft\nbreak\n").0,
        github::parse_file("soft\nbreak\n")
    );
}

#[test]
fn links() {
    insta::assert_snapshot!(snapshot(
        "[**bold** label](https://a.example) and [ref][r] and <https://b.example>\n\n\
         [r]: https://ref.example\n"
    ));
}

#[test]
fn autolinks() {
    insta::assert_snapshot!(snapshot(
        "see https://example.com/path. or www.example.com, mail a@example.com\n"
    ));
}

#[test]
fn images() {
    insta::assert_snapshot!(snapshot(
        "![alt text](https://img.example/a.png)\n\n\
         [![CI](https://img.example/badge.svg)](https://ci.example)\n"
    ));
}

#[test]
fn entities_and_unicode() {
    insta::assert_snapshot!(snapshot("&amp; &copy; \\*not em\\* 日本語 — ünïcode\n"));
}

#[test]
fn bullet_and_ordered_lists() {
    insta::assert_snapshot!(snapshot(
        "- one\n- two\n  - nested\n    1. deep\n\n3. three\n4. four\n"
    ));
}

#[test]
fn loose_list() {
    insta::assert_snapshot!(snapshot(
        "- first\n\n  second paragraph\n\n- item\n\n  ```\n  code in item\n  ```\n"
    ));
}

#[test]
fn task_lists() {
    insta::assert_snapshot!(snapshot(
        "- [x] done\n- [ ] todo\n- plain\n  - [X] nested done\n\n> - [ ] in quote\n"
    ));
}

/// The text after toggling the task item at `path`, if it is one.
fn toggled(markdown: &str, path: &[usize]) -> Option<String> {
    let (range, with) = github::toggle_task(markdown, path)?;
    let mut out = markdown.to_owned();
    out.replace_range(range, with);
    Some(out)
}

#[test]
fn toggling_a_task_edits_only_its_own_checkbox() {
    let source = "- [ ] same\n- [ ] same\n- [x] same\n";
    assert_eq!(
        toggled(source, &[0, 1]).as_deref(),
        Some("- [ ] same\n- [x] same\n- [x] same\n")
    );
    assert_eq!(
        toggled(source, &[0, 2]).as_deref(),
        Some("- [ ] same\n- [ ] same\n- [ ] same\n")
    );
}

#[test]
fn toggling_reaches_nested_quoted_and_uppercase_tasks() {
    let source = "Intro\n\n- plain\n  - [X] nested\n\n> 1. [ ] quoted\n";
    assert_eq!(
        toggled(source, &[1, 0, 1, 0]).as_deref(),
        Some("Intro\n\n- plain\n  - [ ] nested\n\n> 1. [ ] quoted\n")
    );
    assert_eq!(
        toggled(source, &[2, 0, 0]).as_deref(),
        Some("Intro\n\n- plain\n  - [X] nested\n\n> 1. [x] quoted\n")
    );
}

#[test]
fn toggling_finds_checkboxes_after_tabs_and_crlf() {
    assert_eq!(
        toggled("-\t[ ] tab\r\n-\t[x] tab\r\n", &[0, 1]).as_deref(),
        Some("-\t[ ] tab\r\n-\t[ ] tab\r\n")
    );
    assert_eq!(
        toggled("* é [ ]\n* [ ] émoji 🎉\n", &[0, 1]).as_deref(),
        Some("* é [ ]\n* [x] émoji 🎉\n")
    );
}

#[test]
fn only_task_items_toggle() {
    let source = "- plain\n- [ ] task\n\n```\n- [ ] code\n```\n\n[ ] text\n";
    assert_eq!(toggled(source, &[0, 0]), None);
    assert_eq!(toggled(source, &[0, 1, 0]), None);
    assert_eq!(toggled(source, &[1]), None);
    assert_eq!(toggled(source, &[2]), None);
    assert_eq!(toggled(source, &[9]), None);
    assert!(toggled(source, &[0, 1]).is_some());
}

#[test]
fn quotes() {
    insta::assert_snapshot!(snapshot("> quoted\nlazy continuation\n>\n> > nested\n"));
}

#[test]
fn alerts() {
    insta::assert_snapshot!(snapshot(
        "> [!NOTE]\n> Note body\n\n\
         > [!TIP]\n> Tip body\n\n\
         > [!IMPORTANT]\n> Important body\n\n\
         > [!WARNING]\n> Warning body\n\n\
         > [!CAUTION]\n> Caution **body**\n> - list in alert\n"
    ));
}

#[test]
fn non_alert_markers_stay_quotes() {
    insta::assert_snapshot!(snapshot(
        "> [!FOO]\n> unknown kind\n\n> [!NOTE] Custom title\n> body\n"
    ));
}

#[test]
fn code_blocks() {
    insta::assert_snapshot!(snapshot(
        "```rust title=x\nfn main() {\n    println!(\"hi\");\n}\n```\n\n\
         ```\nno lang\n\n```\n\n    indented code\n\n```\nunclosed fence\n"
    ));
}

#[test]
fn tables() {
    insta::assert_snapshot!(snapshot(
        "| Left | Center | Right | None |\n|:-----|:------:|------:|------|\n\
         | `a\\|b` | **c** | 3 | |\n| ragged |\n"
    ));
}

#[test]
fn rule_and_html() {
    insta::assert_snapshot!(snapshot(
        "above\n\n---\n\n<details>\n<summary>More</summary>\n</details>\n\n\
         <!-- template comment -->\n\ninline <b>tag</b> and <!-- hidden --> comment\n"
    ));
}

#[test]
fn empty_document() {
    insta::assert_snapshot!(snapshot(""));
}

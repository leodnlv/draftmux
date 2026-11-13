//! Independently specified acceptance matrix; unchanged noncanonical input is
//! a failure in required-format cases, not evidence of a safe formatter.
use markup::format::{Wrap, format, format_report};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use unicode_width::UnicodeWidthStr;

const MODES: [Wrap; 3] = [Wrap::Keep, Wrap::Always, Wrap::Never];

fn semantics(input: &str) -> Vec<String> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let mut out = Vec::new();
    let mut text = String::new();
    let mut code = false;
    let flush = |out: &mut Vec<String>, text: &mut String| {
        if !text.is_empty() {
            out.push(format!("text:{text}"));
            text.clear();
        }
    };
    for event in Parser::new_ext(input, options) {
        match event.into_static() {
            Event::Text(s) if !code => {
                for c in s.chars() {
                    if matches!(c, ' ' | '\t' | '\r' | '\n') {
                        if !text.ends_with(' ') {
                            text.push(' ');
                        }
                    } else {
                        text.push(c);
                    }
                }
            }
            Event::SoftBreak => {
                if !text.ends_with(' ') {
                    text.push(' ');
                }
            }
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => {
                flush(&mut out, &mut text);
                out.push(format!("link:{:?}:{:?}", dest_url.as_ref(), title.as_ref()));
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                flush(&mut out, &mut text);
                out.push(format!(
                    "image:{:?}:{:?}",
                    dest_url.as_ref(),
                    title.as_ref()
                ));
            }
            Event::Code(s) => {
                flush(&mut out, &mut text);
                out.push(format!("code:{:?}", s.as_ref()));
            }
            Event::InlineMath(s) => {
                flush(&mut out, &mut text);
                out.push(format!("math:{:?}", s.as_ref()));
            }
            Event::DisplayMath(s) => {
                flush(&mut out, &mut text);
                out.push(format!("display-math:{:?}", s.as_ref()));
            }
            Event::Html(s) => {
                flush(&mut out, &mut text);
                out.push(format!("html:{:?}", s.as_ref()));
            }
            Event::InlineHtml(s) => {
                flush(&mut out, &mut text);
                out.push(format!("inline-html:{:?}", s.as_ref()));
            }
            Event::FootnoteReference(s) => {
                flush(&mut out, &mut text);
                out.push(format!("footnote:{:?}", s.as_ref()));
            }
            Event::Text(s) => {
                flush(&mut out, &mut text);
                out.push(format!("literal:{:?}", s.as_ref()));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut out, &mut text);
                code = true;
                out.push(match kind {
                    pulldown_cmark::CodeBlockKind::Indented => "indented-code".into(),
                    pulldown_cmark::CodeBlockKind::Fenced(info) => {
                        format!("fenced-code:{:?}", info.as_ref())
                    }
                });
            }
            Event::Start(Tag::FootnoteDefinition(s)) => {
                flush(&mut out, &mut text);
                out.push(format!("footnote-def:{:?}", s.as_ref()));
            }
            event => {
                flush(&mut out, &mut text);
                if matches!(event, Event::Start(Tag::CodeBlock(_))) {
                    code = true;
                }
                if matches!(event, Event::End(TagEnd::CodeBlock)) {
                    code = false;
                }
                out.push(format!("{event:?}"));
            }
        }
    }
    flush(&mut out, &mut text);
    out
}

fn invariant(input: &str, output: &str, wrap: Wrap) {
    assert_eq!(
        semantics(input),
        semantics(output),
        "semantics {wrap:?}\nINPUT:\n{input}\nOUTPUT:\n{output}"
    );
    assert_eq!(
        format(output, wrap),
        output,
        "idempotence {wrap:?}: {input:?}"
    );
}

fn golden(input: &str, expected: &str) {
    for wrap in MODES {
        let report = format_report(input, wrap);
        assert_eq!(report.text, expected, "{wrap:?}: {input:?}");
        assert!(!report.declined, "declined {wrap:?}: {input:?}");
        invariant(input, &report.text, wrap);
    }
}

#[test]
fn blocks_and_inline_normalization() {
    for (input, expected) in [
        (
            "Title\n=====\n\nSubtitle\n-----\n",
            "# Title\n\n## Subtitle\n",
        ),
        ("### title ###\n", "### title\n"),
        ("#\n", "#\n"),
        ("+ one\n+ two\n", "- one\n- two\n"),
        ("3) three\n9) four\n", "3. three\n4. four\n"),
        ("0) zero\n7) next\n", "0. zero\n1. next\n"),
        ("* [_label_](/u)\n", "- [*label*](/u)\n"),
        ("_one_ __two__ ***three***\n", "*one* **two** ***three***\n"),
        ("*__mixed__* ___both___\n", "***mixed*** ***both***\n"),
        ("* literal ]: text\n", "- literal ]: text\n"),
        (
            "* item `map[k]: v`\n* other\n",
            "- item `map[k]: v`\n- other\n",
        ),
        (
            "* outer\n  + inner\n    * third\n",
            "- outer\n  - inner\n    - third\n",
        ),
        ("* [X] done\n* [ ] open\n", "- [x] done\n- [ ] open\n"),
        ("- [ ]\n", "- [ ]\n"),
        ("-\n", "-\n"),
        ("- first\n\n- second\n", "- first\n\n- second\n"),
        ("- a\n* b\n\n__x__\n", "- a\n\n* b\n\n**x**\n"),
        ("1. a\n1) b\n\n__x__\n", "1. a\n\n1) b\n\n**x**\n"),
        ("body\n\n___\n", "body\n\n---\n"),
        ("___\n\nbody\n", "***\n\nbody\n"),
        ("- ***\n", "- ***\n"),
        ("\n\nbody\n\n\nnext\n\n", "body\n\nnext\n"),
        ("hello", "hello\n"),
        ("   \n", ""),
        ("", ""),
    ] {
        golden(input, expected);
    }
}

#[test]
fn prose_bytes_do_not_gain_escapes_or_decode_entities() {
    for input in [
        "foo_bar_baz #123 @user! 2 * 3 a<b x>y [ ] -> ~ ^ | = + !\n",
        "\\* \\_ \\# \\. \\! \\|\n",
        "&amp; &nbsp; &copy; &#123; &#x2014;\n",
        "a\u{a0}b c\u{202f}d e\u{200b}f g\u{feff}h 日本\u{3000}語\n",
        ":shipit: @org/team GH-12 owner/repo#1\n",
        "`code with  spaces` and ``a ` b``\n",
        "<kbd>Ctrl</kbd> <br> <!-- c -->\n",
        "$a_b * c$ and $`x`$\n",
        "x[^1]\n\n[^1]: A footnote.\n",
        "$$\nx_y * z\n$$\n",
        "[missing] and [x][missing]\n",
    ] {
        golden(input, input);
    }
    golden("_a_b_\n", "*a_b*\n");
    assert_eq!(format("a\tb  c\nd\n", Wrap::Keep), "a\tb  c\nd\n");
    for wrap in [Wrap::Always, Wrap::Never] {
        assert_eq!(format("a\tb  c\nd\n", wrap), "a b c d\n");
    }
}

#[test]
fn width_boundaries_use_display_columns_and_fill_greedily() {
    for (word, count, fit) in [
        ("word", 24, 16),
        ("é", 45, 40),
        ("日本語の文章", 9, 6),
        ("e\u{301}", 45, 40),
        ("👩‍💻", 35, 27),
    ] {
        let input = format!("{}\n", vec![word; count].join(" "));
        let out = format(&input, Wrap::Always);
        assert_eq!(
            out.lines().next().unwrap(),
            vec![word; fit].join(" "),
            "{input}"
        );
        assert!(out.lines().all(|line| line.width() <= 80));
        invariant(&input, &out, Wrap::Always);
        assert_eq!(format(&out, Wrap::Never), input);
    }
    for (prefix, word, count, should_wrap) in [
        ("", "a", 78, false),
        ("", "a", 79, true),
        ("", "界", 39, false),
        ("", "界", 40, true),
        ("- ", "界", 38, false),
        ("- ", "界", 39, true),
    ] {
        let input = format!("{prefix}{} b\n", word.repeat(count));
        let out = format(&input, Wrap::Always);
        assert_eq!(out.lines().count() > 1, should_wrap, "{input:?} => {out:?}");
        invariant(&input, &out, Wrap::Always);
    }
    for marker in ["- ", "9. ", "99. ", "> ", "> - ", "> - 1. "] {
        let input = format!("{marker}{}\n", "word ".repeat(24).trim_end());
        let out = format(&input, Wrap::Always);
        assert_ne!(out, input);
        assert!(out.lines().all(|line| line.width() <= 80), "{out}");
        invariant(&input, &out, Wrap::Always);
    }
}

#[test]
fn hard_breaks_and_cross_mode_laws() {
    for input in [
        "a  \nb\n",
        "a\\\nb\n",
        "a   \nb\n",
        "- a  \n  b\n",
        "> a  \n> b\n",
        "*a  \nb*\n",
    ] {
        for wrap in MODES {
            let out = format(input, wrap);
            invariant(input, &out, wrap);
            assert!(out.contains("\\\n"));
        }
    }
    for input in [
        "a\nb\n",
        "a  b\tc\n\nd e\n",
        "- a\n  b\n- c\n",
        "> a\n> b\n",
    ] {
        let always = format(input, Wrap::Always);
        let never = format(input, Wrap::Never);
        assert_eq!(format(&always, Wrap::Never), never);
        assert_eq!(format(&never, Wrap::Always), always);
        assert_eq!(format(&always, Wrap::Keep), always);
        assert_eq!(format(&never, Wrap::Keep), never);
        for wrap in MODES {
            invariant(input, &format(input, wrap), wrap);
        }
    }
}

#[test]
fn atoms_and_wrap_hazards() {
    for atom in [
        "`code with spaces`",
        "``a ` b``",
        "[link text](https://e.test/a_(b) \"a title\")",
        "![alt text](a.png \"T\")",
        "[![badge](a.png)](/u)",
        "<https://e.test/a_b>",
        "https://e.test/a_b?x=1&y=2",
        "www.example.com/a_b",
        "user@example.com",
    ] {
        let input = format!("{} {atom} tail words\n", "é ".repeat(36).trim_end());
        let out = format(&input, Wrap::Always);
        assert!(out.lines().count() > 1, "{out}");
        assert!(out.contains(atom), "{out}");
        invariant(&input, &out, Wrap::Always);
    }
    for hazard in [
        "# heading",
        "> quote",
        "- item",
        "+ item",
        "* item",
        "1. item",
        "1) item",
        "---",
        "===",
        "***",
        "___",
        "```",
        "~~~",
        "<!-- comment -->",
        "<div>x</div>",
        "[ref]: /u",
        "| a | b |",
    ] {
        for padding in [65, 70, 75] {
            let input = format!("{} tail {hazard} remaining words\n", "a".repeat(padding));
            let out = format(&input, Wrap::Always);
            assert_ne!(out, input, "did not wrap {input:?}");
            invariant(&input, &out, Wrap::Always);
            assert!(!out.contains('\\'), "added escaping: {out}");
        }
    }
}

#[test]
fn opaque_blocks_do_not_freeze_containers_or_neighbors() {
    for (input, expected) in [
        (
            "* a\n\n  ~~~~js extra\n  const  x = 1\n  ~~~~\n",
            "- a\n\n  ~~~~js extra\n  const  x = 1\n  ~~~~\n",
        ),
        (
            "*   item\n\n    ```js x\n    a  b\n    ```\n",
            "- item\n\n  ```js x\n  a  b\n  ```\n",
        ),
        ("* item\n\n      a  b\n", "- item\n\n      a  b\n"),
        (
            "* item\n\n  <div>\n   raw  HTML\n  </div>\n",
            "- item\n\n  <div>\n   raw  HTML\n  </div>\n",
        ),
        (
            "* x [r]\n\n  [r]: /u\n* y\n",
            "- x [r]\n\n  [r]: /u\n\n- y\n",
        ),
        (
            "<details>\n<summary>S</summary>\n\n* inner\n\n</details>\n",
            "<details>\n<summary>S</summary>\n\n- inner\n\n</details>\n",
        ),
        (
            "<!-- prettier-ignore -->\n\n* _keep_\n\n__format__\n",
            "<!-- prettier-ignore -->\n\n* _keep_\n\n**format**\n",
        ),
        (
            "<!-- prettier-ignore-start -->\n\n* _keep_\n\n\n__keep__\n\n<!-- prettier-ignore-end -->\n\n__format__\n",
            "<!-- prettier-ignore-start -->\n\n* _keep_\n\n\n__keep__\n\n<!-- prettier-ignore-end -->\n\n**format**\n",
        ),
    ] {
        golden(input, expected);
    }
    for opaque in [
        "```js\r\nconst  x = 1\r\n```\r\n",
        "```js extra\nconst  x = 1\n```\n",
        "~~~~x\na\n~~~~~\n",
        "```\nunclosed",
        "    code  \n",
        "<!-- raw\n\n comment -->\n",
        "<script>\na < b;\n</script>\n",
        "<?xml version='1'?>\n",
        "<!DOCTYPE html>\n",
        "<![CDATA[ a < b ]]>\n",
        "<custom-tag>\n",
        "<div>\n raw\n</div>",
    ] {
        golden(opaque, opaque);
        if opaque.ends_with('\n') && !opaque.contains("unclosed") {
            let input = format!("Title\n=====\n\n{opaque}\n* body\n");
            let expected = format!("# Title\n\n{opaque}\n- body\n");
            golden(&input, &expected);
        }
    }
}

#[test]
fn references_are_preserved_with_resolution_and_unused_definitions() {
    for definition in [
        "[Unused]: <https://e.test/a> 'Title'\n",
        "[r]: /one\n[r]: /two\n",
        "[r]: /u\n  \"A title\"\n",
        "[multi\n line]: /u\n",
        "[R]: <u> (title)\n",
    ] {
        let input = format!("* [r] [r][] [label][r]\n\n{definition}\n__end__\n");
        let expected = format!("- [r] [r][] [label][r]\n\n{definition}\n**end**\n");
        golden(&input, &expected);
    }
    golden("[r]: /u\n* [r]\n", "[r]: /u\n\n- [r]\n");
    golden("[unused]: /u\ntext\n", "[unused]: /u\n\ntext\n");
    golden("* [X] budget $5\n", "- [x] budget $5\n");
    golden("*\t[x] done\n", "- [x] done\n");
    golden(
        "[^1]: footnote\n\n* text[^1]\n",
        "[^1]: footnote\n\n- text[^1]\n",
    );
    golden(
        "* text[^b] and text[^a]\n\n[^a]: first\n\n[^b]: second\n",
        "- text[^b] and text[^a]\n\n[^a]: first\n\n[^b]: second\n",
    );
    golden("> [r]: /u\n>\n> * [r]\n", "> [r]: /u\n>\n> - [r]\n");
}

#[test]
fn frontmatter_bytes_and_unfinished_envelopes() {
    for front in [
        "---\r\nvalues: [a, b]\r\n---\r\n\n",
        "+++\ninvalid = [\n+++\n\n",
        "---\na: b\n...\n\n",
        "\u{feff}---\ninvalid YAML\n---\n",
        "---\ntext\n---\n\n\n",
    ] {
        for wrap in MODES {
            let input = format!("{front}* body\n");
            assert_eq!(format(&input, wrap), format!("{front}- body\n"));
            assert_eq!(format(front, wrap), front);
        }
    }
    for input in [
        "---\nx: y\n\n* b\n",
        "+++\na\n...\n",
        "--- \nx: y\n---\n* b\n",
    ] {
        for wrap in MODES {
            let report = format_report(input, wrap);
            assert_eq!(report.text, input);
            assert!(report.declined);
        }
    }
    golden("\n---\na\n---\n", "***\n\n## a\n");
    for wrap in MODES {
        assert_eq!(
            format("\u{feff}___\n\nbody\n", wrap),
            "\u{feff}***\n\nbody\n"
        );
    }
}

#[test]
fn tables_align_without_changing_cells() {
    golden(
        "a | b\n--- | ---\nx | y\n",
        "| a   | b   |\n| --- | --- |\n| x   | y   |\n",
    );
    golden(
        "| 日本 | b |\n|---|---|\n| x | 😀😀 |\n",
        "| 日本 | b    |\n| ---- | ---- |\n| x    | 😀😀 |\n",
    );
    golden(
        "| a | b |\n|---|---|\n| `c\\|d` | x_y |\n",
        "| a      | b   |\n| ------ | --- |\n| `c\\|d` | x_y |\n",
    );
    golden(
        "| a | b |\n|---|---|\n| c |\n",
        "| a   | b   |\n| --- | --- |\n| c   |     |\n",
    );
    for input in [
        "| a |\n|---|\n",
        "| | |\n|-|-|\n",
        "| left | center | right | none |\n| :--- | :---: | ---: | --- |\n| a \\| b | `x\\|y` | [z](/u \"t\") | |\n",
        "| a | b |\n|-|-|\n| x | y | ignored |\n",
    ] {
        for wrap in MODES {
            let out = format(input, wrap);
            invariant(input, &out, wrap);
            assert!(out.starts_with("| "));
        }
    }
    for prefix in ["> ", "  "] {
        let table = "| a | long |\n|---|---|\n| c | d |\n";
        let input = format!(
            "{}{}",
            if prefix == "  " { "- item\n\n" } else { "" },
            table
                .lines()
                .map(|l| format!("{prefix}{l}\n"))
                .collect::<String>()
        );
        let out = format(&input, Wrap::Keep);
        assert!(out.contains("| a   | long |"), "{out}");
        assert!(!out.lines().any(|l| l.ends_with(' ') || l.ends_with('\t')));
        invariant(&input, &out, Wrap::Keep);
    }
}

#[test]
fn independent_oracle_rejects_semantic_mutations() {
    for (a, b) in [
        ("# a", "## a"),
        ("3. a", "1. a"),
        ("- [x] a", "- [ ] a"),
        ("a  \nb", "a b"),
        ("[a](/u \"t\")", "[a](/v \"t\")"),
        ("[a](/u \"t\")", "[a](/u \"s\")"),
        ("![a](/u)", "![b](/u)"),
        ("`a  b`", "`a b`"),
        ("<i x='a'>", "<i x='b'>"),
        ("$a_b$", "$a\\_b$"),
        ("x[^1]\n\n[^1]: foot", "x\\[^1\\]\n\n[^1]: foot"),
        ("|a|\n|:-|", "|a|\n|-:|"),
        ("- a\n- b", "- a\n\n- b"),
    ] {
        assert_ne!(semantics(a), semantics(b), "oracle failed: {a:?} vs {b:?}");
    }
}

#[test]
fn generated_supported_documents_must_format() {
    for n in 0..100 {
        let input = format!(
            "Title {n}\n=======\n\n* _item {n}_\n* [x] `code {n}`\n\nParagraph with [link {n}](/u) and words.\n"
        );
        let expected = format!(
            "# Title {n}\n\n- *item {n}*\n- [x] `code {n}`\n\nParagraph with [link {n}](/u) and words.\n"
        );
        golden(&input, &expected);
    }
}

#[test]
fn arbitrary_source_is_safe_and_stable() {
    let tokens = [
        "*",
        "**",
        "_",
        "`",
        "[",
        "]",
        "(",
        ")",
        "<",
        ">",
        "#",
        "-",
        "1.",
        "\\",
        "&amp;",
        "é",
        "日本",
        "a",
        " ",
        "  ",
        "\t",
        "\n",
        "\n\n",
        "  \n",
        "[x]: /u",
        "[x]",
        "<div>",
        "</div>",
        "~~",
        "| a | b |",
        "$",
        "[^1]",
        "&nbsp;",
        "```",
        "~~~",
    ];
    let mut seed = 12345u64;
    for _ in 0..500 {
        let mut input = String::new();
        for _ in 0..25 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            input.push_str(tokens[(seed >> 32) as usize % tokens.len()]);
        }
        for wrap in MODES {
            invariant(&input, &format(&input, wrap), wrap);
        }
    }
}

#[test]
fn multiline_atoms_and_numbered_continuations() {
    for input in ["[link\nacross](u)\n", "`code\nspan`\n"] {
        assert_eq!(format(input, Wrap::Keep), input);
        for wrap in [Wrap::Always, Wrap::Never] {
            let out = format(input, wrap);
            assert_eq!(out, input.trim_end().replace('\n', " ") + "\n");
            invariant(input, &out, wrap);
        }
    }
    for start in [9, 99] {
        let input = format!(
            "{start}) {}\n1) {}\n",
            "word ".repeat(24).trim_end(),
            "word ".repeat(24).trim_end()
        );
        let out = format(&input, Wrap::Always);
        assert!(out.contains(&format!("\n{}. ", start + 1)), "{out}");
        assert!(
            out.contains(&format!(
                "\n{}word",
                " ".repeat((start + 1).to_string().len() + 2)
            )),
            "{out}"
        );
        invariant(&input, &out, Wrap::Always);
    }
    let input = format!("- [X] {}\n", "word ".repeat(24).trim_end());
    let out = format(&input, Wrap::Always);
    assert!(out.lines().all(|line| line.width() <= 80), "{out}");
    invariant(&input, &out, Wrap::Always);
}

#[test]
fn alerts_quotes_and_rich_copy_keep_document_semantics() {
    for kind in ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"] {
        let input = format!("> [!{kind}]\n> {}\n", "word ".repeat(24).trim_end());
        for wrap in MODES {
            let out = format(&input, wrap);
            assert!(out.starts_with(&format!("> [!{kind}]\n")));
            invariant(&input, &out, wrap);
            if wrap == Wrap::Always {
                assert!(out.lines().all(|line| line.width() <= 80));
            }
        }
    }
    golden(
        "> [!NOTE] Custom title\n> text\n",
        "> [!NOTE] Custom title\n> text\n",
    );
    let unknown = "> [!FOO]\n> text\n";
    assert_eq!(format(unknown, Wrap::Keep), unknown);
    for wrap in MODES {
        invariant(unknown, &format(unknown, wrap), wrap);
    }
    for (input, expected) in [
        (">a\n>b\n", "> a\n> b\n"),
        ("> a\nb\n", "> a\n> b\n"),
        ("> > * item\n", "> > - item\n"),
        (">code:\n>\n>     x  y\n", "> code:\n>\n>     x  y\n"),
    ] {
        let out = format(input, Wrap::Keep);
        assert_eq!(out, expected);
        invariant(input, &out, Wrap::Keep);
    }
    for input in [
        "Title\n=====\n\n* _one_\n* [x] two\n",
        "> [!TIP]\n> A **tip** with [link](https://example.com).\n",
        "| a | b |\n|:-|-:|\n| `x` | _y_ |\n",
        "a  \nb\n\n```js\nconst  x = 1\n```\n",
        "<div>raw HTML</div>\n",
    ] {
        let out = format(input, Wrap::Keep);
        let before = markup::github::parse(input);
        let after = markup::github::parse(&out);
        assert_eq!(before, after);
        assert_eq!(
            markup::rich_text::pasted(&before),
            markup::rich_text::pasted(&after)
        );
        assert_eq!(
            markup::rich_text::html(&before),
            markup::rich_text::html(&after)
        );
        assert_eq!(
            markup::rich_text::rtf(&before),
            markup::rich_text::rtf(&after)
        );
    }
}

#[test]
fn realistic_already_formatted_drafts_and_unsupported_regions() {
    for n in 0..24 {
        let input = format!(
            "# Fix issue {n}\n\nThanks @user! See #{n} and `snake_case`.\n\n- [x] Add tests\n- [ ] Review [the patch](https://example.com/{n})\n\n```rust\nlet  x = {n};\n```\n\n<details>\n<summary>Notes</summary>\n\nKeep &nbsp; and &#123; as written.\n\n</details>\n"
        );
        assert_eq!(format(&input, Wrap::Keep), input);
        invariant(&input, &format(&input, Wrap::Always), Wrap::Always);
    }
    for input in [
        ":::note\nSome content\n:::\n",
        "{% if value %}\nText\n{% endif %}\n",
        "<Component value={x_y} />\n",
        "Title\r\n=====\r\n\r\n* item\r\n",
        "text\rnext\r",
        "| a\tb | c |\n| --- | --- |\n",
    ] {
        for wrap in MODES {
            let out = format(input, wrap);
            invariant(input, &out, wrap);
        }
    }
    for atom in [
        "x".repeat(200),
        format!("`{}`", "x ".repeat(100)),
        format!("[{}](/u)", "x ".repeat(100)),
    ] {
        let input = format!("- {atom} tail\n");
        let out = format(&input, Wrap::Always);
        assert!(out.starts_with(&format!("- {atom}")), "{out}");
        invariant(&input, &out, Wrap::Always);
    }
}

#[test]
fn escaped_pipe_parity_unicode_and_inline_table_content() {
    for slash_count in 1..=4 {
        let input = format!(
            "| a | b |\n| --- | --- |\n| x{}|y | z |\n",
            "\\".repeat(slash_count)
        );
        for wrap in MODES {
            invariant(&input, &format(&input, wrap), wrap);
        }
    }
    for input in [
        "| _a_ | ![x](/u) |\n|:-|:-:|\n| &amp; | <b>x</b> |\n",
        "| e\u{301} | 界 | 👩‍💻 |\n|---|---|---|\n| x | y | z |\n",
        "| a | b |\n| - | - | - |\n",
    ] {
        for wrap in MODES {
            invariant(input, &format(input, wrap), wrap);
        }
    }
}

/// Resource probes are isolated from ordinary correctness tests and report
/// measurements rather than asserting a flaky shared-CPU deadline.
#[test]
#[ignore]
fn resource_matrix() {
    for (name, input) in [
        (
            "1kb",
            "A paragraph with words and [a link](/u).\n\n".repeat(25),
        ),
        (
            "100kb",
            "A paragraph with words and [a link](/u).\n\n".repeat(2500),
        ),
        (
            "1mb",
            "A paragraph with words and [a link](/u).\n\n".repeat(25000),
        ),
        ("deep-quotes", format!("{}text\n", "> ".repeat(3000))),
        ("brackets", "[".repeat(20000)),
        ("delimiters", "*a ".repeat(20000)),
        (
            "table",
            format!("| a | b |\n|---|---|\n{}", "| x | y |\n".repeat(10000)),
        ),
    ] {
        let started = std::time::Instant::now();
        let out = format(&input, Wrap::Always);
        eprintln!("{name}: {} bytes in {:?}", input.len(), started.elapsed());
        assert_eq!(format(&out, Wrap::Always), out, "{name}");
    }
}

use markup::format::{Wrap, format};

#[test]
fn reference_fixtures() {
    for (name, input) in [
        (
            "link-text-wrap",
            include_str!("fixtures/format/link-text-wrap.md"),
        ),
        (
            "setext-heading-wrap",
            include_str!("fixtures/format/setext-heading-wrap.md"),
        ),
        (
            "wrapped-block-starts",
            include_str!("fixtures/format/wrapped-block-starts.md"),
        ),
    ] {
        for wrap in [Wrap::Keep, Wrap::Always, Wrap::Never] {
            let output = format(input, wrap);
            insta::assert_snapshot!(format!("{name}-{wrap:?}"), output);
            assert_eq!(format(&output, wrap), output);
        }
    }
}

#[test]
fn normalizes_blocks() {
    let input = "Title\n=====\n\n* _one_\n* __two__\n\n3) three\n9) four\n";
    assert_eq!(
        format(input, Wrap::Keep),
        "# Title\n\n- *one*\n- **two**\n\n3. three\n4. four\n"
    );
}

#[test]
fn wrap_modes_and_continuations() {
    let prose = "word ".repeat(24).trim_end().to_owned();
    let input = format!("{prose}\n\n- {prose}\n");
    assert_eq!(format(&input, Wrap::Keep), input);
    let wrapped = format(&input, Wrap::Always);
    assert_ne!(wrapped, input);
    assert!(wrapped.lines().all(|line| line.len() <= 80), "{wrapped}");
    assert!(wrapped.contains("\n  word"), "{wrapped}");
    assert_eq!(format(&wrapped, Wrap::Never), input);
    assert_eq!(format("first\nsecond\n", Wrap::Keep), "first\nsecond\n");
    assert_eq!(format("first\nsecond\n", Wrap::Never), "first second\n");
}

#[test]
fn atomic_inlines_and_hard_breaks() {
    let prefix = "words ".repeat(12);
    for atom in [
        "`inline code with spaces`",
        "[a link with spaces](https://example.com/a)",
        "https://example.com/very/long/path/without/spaces",
    ] {
        let input = format!("{prefix}{atom} words after it\n");
        let output = format(&input, Wrap::Always);
        assert!(output.contains(atom), "{output}");
        assert_ne!(output, input);
        assert!(output.lines().count() > 1);
        assert_eq!(format(&output, Wrap::Always), output);
    }
    for wrap in [Wrap::Keep, Wrap::Always, Wrap::Never] {
        assert_eq!(format("first  \nsecond\n", wrap), "first\\\nsecond\n");
    }
}

#[test]
fn aligns_tables() {
    let input = "| a | longer |\n| :- | -: |\n| longest | b |\n";
    let expected = "| a       | longer |\n| :------ | -----: |\n| longest | b      |\n";
    for wrap in [Wrap::Keep, Wrap::Always, Wrap::Never] {
        assert_eq!(format(input, wrap), expected);
        assert_eq!(format(expected, wrap), expected);
    }
}

#[test]
fn aligns_nested_tables() {
    let input = "> | a | longer |\n> | :- | -: |\n> | longest | b |\n";
    assert_eq!(
        format(input, Wrap::Keep),
        "> | a       | longer |\n> | :------ | -----: |\n> | longest | b      |\n"
    );
}

#[test]
fn preserves_opaque_source_and_references() {
    let input = "---\r\nvalues: [a, b]\r\n---\r\n\n```js extra\nconst  x =  1\n```\n\n<div class='a'>\n  raw   HTML\n</div>\n\n[Unused]: <https://example.com/a> 'Title'\n\n[used][ref]\n\n[ref]: /url\n  \"A title\"\n";
    for wrap in [Wrap::Keep, Wrap::Always, Wrap::Never] {
        assert_eq!(format(input, wrap), input);
    }
    let input = "* a\n\n    ~~~~js\n    const  x = 1\n    ~~~~\n";
    assert_eq!(
        format(input, Wrap::Keep),
        "- a\n\n    ~~~~js\n    const  x = 1\n    ~~~~\n"
    );
    let input = "- item\n\n  [multi\n  line]: /unused\n\n* another\n";
    assert_eq!(format(input, Wrap::Keep), input);
}

#[test]
fn edge_cases_are_idempotent() {
    for input in [
        "",
        "   \n",
        "hello",
        "é 日本語 😀\n",
        "a\u{a0}b\n",
        "- [x] task\n- [ ] undone\n",
        "> quoted\n> words\n",
        "> [!NOTE]\n> a note\n",
        "a &amp; b \\* c\n",
        "![alt text](image.png \"Title\")\n",
        "before <em>raw</em> after\n",
        "- outer\n  + inner\n\n- next\n",
        "a\n\n---\n",
        "| a \\| b | `x` |\n| --- | --- |\n| y | z |\n",
        "---\nfront: matter\n---\n\n* body\n",
    ] {
        for wrap in [Wrap::Keep, Wrap::Always, Wrap::Never] {
            let output = format(input, wrap);
            assert_eq!(format(&output, wrap), output, "{input:?}, {wrap:?}");
        }
    }
}

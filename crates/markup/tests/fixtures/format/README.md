# Markdown formatting fixtures

`link-text-wrap.md`, `setext-heading-wrap.md` and `wrapped-block-starts.md`
come from `oxc-project/oxc` at commit
`0d2e41a182ed8e93fce2f49477f5eb9e17ca1725`, under
`crates/oxc_formatter_markdown/tests/fixtures/markdown/prose-wrap/`.
They are MIT licensed; the upstream notice is in `LICENSE`.

These cover reference links, long inline links, headings, hard breaks and
paragraph text that could become block syntax when wrapped. The tests run
all three wrap modes. Snapshots describe Draftmux's output, not upstream's
expected output.

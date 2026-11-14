# Draftmux

Draftmux opens the message in its own window, with this conversation as
context, so the user can polish it there. Later, when the user says the draft
is done, read it back and use it.

The user runs this skill explicitly to hand a message off. To use the finished
draft, they either say so in the same conversation or run the skill again:

- **Hand off** when they ask for a message to be written, or give no
  instructions. Follow "Hand off" below.
- **Read back** when they say the draft is done or ready, or ask to use, post
  or send it, including after the conversation was compacted. Follow "Read it
  back" below.

## Locate this session

Every command below starts with these lines. Shell variables don't survive
between commands, so repeat them each time; never reuse a path from memory.

```sh
f=
if [ -n "${DRAFTMUX_DRAFT_PATH:-}" ]; then
  echo "draftmux: this session is already a Draftmux draft; edit $DRAFTMUX_DRAFT_PATH directly" >&2
  exit 1
elif [ -n "${PI_SESSION_ID:-}" ]; then
  f=${PI_SESSION_FILE:-}
elif [ -n "${CLAUDE_CODE_SESSION_ID:-}" ]; then
  f=$(find "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects" -mindepth 2 -maxdepth 2 \
    -name "$CLAUDE_CODE_SESSION_ID.jsonl" -print -quit)
elif [ "${PI_CODING_AGENT:-}" = true ]; then
  echo "draftmux: pi didn't set PI_SESSION_ID for bash; a pi extension that replaces the bash tool may not pass it through" >&2
fi
[ -f "$f" ] || { echo "draftmux: can't find this session's file" >&2; exit 1; }
```

If it fails, report the error to the user. Don't guess the session file by
other means.

## Hand off

Run the locator, then:

```sh
set -- "$f"
[ -n "${PI_CODING_AGENT_DIR:-}" ] && set -- "$@" --pi-agent-dir "$PI_CODING_AGENT_DIR"
draftmux open "$@" --target github --note 'PR description for the retry change'
```

- `--target`: `github` for GitHub markdown, `slack` for Slack messages,
  `rich` for emails and documents with formatting such as bold and lists,
  `plain` for plain text. Leave it out when the user hasn't said where the
  message goes; Draftmux then uses its default for the directory.
- `--backend`: leave it out unless the user names the agent the draft should
  run on, then pass `--backend claude` or `--backend pi`. Also pass
  `--backend pi` when `open` reports that Claude Code can't be used and the
  user still wants the draft.
- `--note`: one line saying what message to write, such as who it's for and
  what it should cover. Draftmux's agent writes the first draft from this
  conversation and the note, so include anything the conversation leaves
  open. Single-quote it; write `'` as `'\''`.

When the message is already written in a file, such as a draft the user asked
you to write earlier, pass `--draft-file <path>` instead of `--note`. The
draft starts with the file's text unchanged and Draftmux's agent doesn't write
a first draft. Pass the path as is; don't read, copy or rewrite the file
first.

It prints the draft ID. Tell the user the ID and that Draftmux is writing the
first draft, or with `--draft-file` that the draft is open in Draftmux, then
stop and wait. Don't write the message yourself.

If `open` exits 1 after printing an ID, the draft exists but the app couldn't
focus it: report the error and the ID, and don't run `open` again (that
creates a second draft). `draftmux focus <id>` retries showing it in the app.
If `open` exits 1 without an ID, nothing was created; report the error.

## Draft fields

In a file for `--draft-file`, put a `title:` field for PRs and issues, or
`subject:`, `to:` and `cc:` for emails, in YAML frontmatter at the top. Only
include known recipients. Use string values, quoting YAML punctuation, and
comma-separated strings rather than lists.

## Read it back

When the user says the draft is done, run the locator again, then read the
draft from Draftmux. Use the text exactly as it is: the user has already
polished it, so don't rewrite, reformat, shorten or fix it.

Use `--json` to read the ordered `fields` map and the `body` without
frontmatter. The top-level `title` is the stored metadata title, not the
frontmatter title; use `fields.title` for a PR or issue.

```sh
draft=$(draftmux show --source "$f" --json) || exit 1
title=$(printf '%s' "$draft" | jq -er '.fields.title | select(length > 0)') || exit 1
printf '%s' "$draft" | jq -j '.body' | gh pr create --title "$title" --body-file -
```

For email, read `fields.subject`, `fields.to` and `fields.cc` separately
from `body`. Other keys are returned unchanged. A Slack `body` is Markdown in
which every line break shows. Pass it as-is to a Slack tool that takes
Markdown; for one that takes Slack mrkdwn, change only `**bold**` to
`*bold*`, `*italic*` to `_italic_`, `~~strike~~` to `~strike~` and
`[text](url)` to `<url|text>`. Do not send or post until the user asks.

A GitHub draft keeps each paragraph on one line. When it goes into a Markdown
file in a repository instead, wrap paragraphs and list items to the
repository's Markdown line length, if it has one; change nothing else.

`draftmux show --path` returns the raw file's path, frontmatter included: use
it for editing, not as a `--body-file`.

If several drafts came from this session, `show` uses the most recently active
one and lists the others on stderr; mention them to the user. If it finds no
draft (for example the session was resumed under a new file) and you still
have the ID from `open`, use `draftmux show <id>` or
`draftmux show <id> --path` instead. Without the ID, list the drafts from
this directory, newest first:

```sh
draftmux list --cwd "$PWD"
```

Show the user the drafts that could be this one and ask which to use, then
read it with `draftmux show <id>`. Never run `open` to recover a draft.
`list --all` includes archived drafts, which `show` also reads;
`draftmux restore <id>` brings one back to the app.

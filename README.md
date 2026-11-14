# Draftmux

Polish the messages your coding agent writes before they go out.

Your agent knows the context, so it's the right one to write the PR description,
the issue or the Slack update. But polishing that message inside an agent
harness sucks.

Draftmux gives the message its own window. Hand it off from Claude Code or pi,
and an agent forked from the session writes the first draft beside a preview of
where it's going. Edit it by hand, rewrite a selection with one click, or ask
for changes in chat. When it's good enough, tell your agent it's done and it
uses the text as written.

https://github.com/user-attachments/assets/031ca42d-ae22-48cb-b2e9-9f94e93042c1

Status: early release; expect rough edges.

## Features

- **Hand-off from your session.** `/draftmux Slack update for the team` forks
  the conversation into a [pi](https://github.com/earendil-works/pi) agent,
  which writes the first draft from what you've already discussed.
- **Chat with the draft's agent.** Ask for changes in plain words. It runs with
  your pi profile, so your skills are available.
- **Edit beside a preview.** Write Markdown and see it as GitHub, Slack, rich
  text or plain text. `⌘+` and `⌘−` change the text size, and `⌘0` resets it.
- **Selection actions.** Select text for alternatives, Shorten or Tighten, or
  your own actions. With text selected, a chat message changes only that part. Hold `⌘` while dragging to select more passages; a
  chat message then edits each of them.
- **Copy or hand back.** Copy formatted for the target with `⌘⇧C`, or tell your
  agent "done, file the issue".

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/raine/draftmux/main/install.sh | sh
```

This installs the latest release and links the `draftmux` command in
`~/.local/bin`; make sure that directory is on your PATH. Run it again to
update.

### macOS

Requires an Apple silicon Mac with macOS 11 or later. The script installs
`Draftmux.app` in `/Applications`. With Homebrew, instead:

```sh
brew install --cask raine/draftmux/draftmux
```

Draftmux keeps itself up to date either way.

### Linux

Runs on x86_64 and arm64 with glibc 2.35 or later, such as Ubuntu 22.04 or
Debian 12 and newer. The script installs the app in `~/.local/lib/draftmux`.
Shortcuts use Ctrl where macOS uses Cmd.

### pi

By default, the agent features run [pi](https://github.com/earendil-works/pi):

```sh
npm install -g @earendil-works/pi-coding-agent
```

Without pi, editing, previews, copying and Markdown formatting of GitHub and
rich-text drafts still work. Drafts can also run on
[Claude Code](#drafting-with-claude-code).

## Quick start

1. Install the agent skill for Claude Code and pi:

   ```sh
   draftmux skill install
   ```

2. In an agent session, hand the message off: `/draftmux PR description` in
   Claude Code, or `/skill:draftmux PR description` in pi.

3. Polish the draft in Draftmux.

4. Back in the same session, say what to do with it, for example
   `done, open the PR`. From another session, run `/draftmux done, open the PR`.

## How it works with pi

- Each draft gets its own pi session, forked from your pi session or converted
  from a Claude Code transcript. Your original session is untouched.
- Draftmux drives pi through its RPC mode (`pi --mode rpc`), one process per
  draft, with your own config (`~/.pi/agent` or another profile), so your
  logins, models and skills apply.
- A bundled extension, passed with `-e`, limits edits to the draft file and
  tells the agent the target. Nothing is installed into your pi.
- Selection actions run in a throwaway fork of the draft's session with tools
  blocked.
- The original agent reads the finished draft with `draftmux show`.

## Drafting with Claude Code

Drafts run on pi unless you ask for Claude Code. From a Claude Code session,
ask the skill for it (`/draftmux PR description, on Claude Code`) or run
`draftmux open <transcript> --backend claude`. To make it the default, set
Backend to Same as source in Settings, globally or for one directory. Claude
Code sessions then continue on Claude Code, and `--backend pi` puts a single
draft on pi.

Needs Claude Code 2.1.293 or later. If Draftmux can't run Claude Code, `open`
reports an error instead of switching to pi.

- The draft continues your conversation in its own Claude Code session. Your
  original session is untouched.
- Claude Code may read files but edit only the draft. It runs without your
  Claude Code settings files, `CLAUDE.md`, hooks or MCP servers.
- New drafts use the session's model unless Settings sets a Claude Code
  drafting model such as `sonnet`. The model chip changes a draft's model, and
  the draft keeps the choice.
- Selection actions run in a throwaway copy of the conversation with no tools.
  They use the draft's model unless Settings sets a Claude Code selection
  model.
- A draft stays on the agent it was created with.

## Configuration

Settings opens with `⌘,` and saves plain JSON to `~/.config/draftmux`, which you
can also edit by hand. Changes apply without a restart. To add your own
selection actions:

```json
"selection_actions": [
  { "name": "More formal", "prompt": "Rewrite in a more formal tone." }
]
```

A draft can start with YAML frontmatter for fields kept apart from the body,
such as `title` for PRs and issues or `subject`, `to` and `cc` for emails.

## Troubleshooting

Errors and crash reports go to `draftmux.log` in
`~/Library/Application Support/Draftmux` on macOS and `~/.local/share/Draftmux`
on Linux. Once the log passes 1 MB, the app moves it to `draftmux.log.1` on its
next start.

## Development

Needs the Rust toolchain in `rust-toolchain.toml`, plus full Xcode on macOS.

```sh
cargo run -- app
```

This uses your real drafts. On macOS, `just dev-app` builds and opens a separate
`Draftmux Dev.app` with its own data directory.

## License

MIT. See [LICENSE](LICENSE).

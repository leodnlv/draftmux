# Changelog

## v0.3.1 (2026-11-16)

- Fix: Switching to another app no longer closes the selection popover or
  drops its suggestions.
- Fix: Clicking or moving the cursor away from a pending accent (a dead key
  such as `` ` ``) finishes it, so the next keys type at the new cursor.
- Fix: Swiping sideways across a wide table or code block in the preview no
  longer scrolls the page up or down.

Thanks [@erikjuhani](https://github.com/erikjuhani) and
[@leodnlv](https://github.com/leodnlv)!

## v0.3.0 (2026-11-15)

- New: Draftmux runs on Windows. See the README for installation.
- Fix: Processes that pi starts no longer keep running after pi exits.

## v0.2.0 (2026-11-14)

A big update, with new features and lots of polish throughout the app.

- New: Draft with Claude Code. `draftmux open --backend claude` continues a
  Claude Code session as the draft's own, leaving the original conversation
  as it was. Claude Code sessions draft on Claude Code by default; set
  Backend in Settings to use pi. Needs Claude Code 2.1.293 or later.
- New: `draftmux <FILE>` opens a file for editing, with an agent session of
  its own in the file's directory.
- New: `draftmux open --draft-file <path>` hands off a message your agent has
  already written.
- New: Save a draft opened from a file back to it with `⌘S` or
  `draftmux save`. It asks before replacing a file changed outside the draft.
- New: Preview a draft as a GitHub repository file, where a single line break
  shows as a space. Pick Repository File from the GitHub menu or pass
  `--target github-file`.
- New: Edit several passages with one chat message. Hold `⌘` (`Ctrl` on
  Linux) while dragging to add a selection.
- New: Draftmux on Linux updates itself like the macOS app.
- New: The update button shows what changed in every version since yours.
- New: Zoom the editor, preview and chat with `⌘+`, `⌘−` and `⌘0`.
- New: Switch a draft without an agent session between Claude Code and pi
  from the agent name above the chat.
- New: The sidebar has an Archived section, and `draftmux restore <id>`
  unarchives a draft.
- New: `draftmux focus <id>` shows a draft in the app, launching it if needed.
- New: Format has Wrap at 80 Columns and Unwrap Paragraphs.
- New: The GitHub preview highlights code blocks in GitHub's syntax colors.
- Changed: Format uses Draftmux's built-in Markdown formatter and no longer
  needs oxfmt.
- Changed: The agent writes GitHub drafts with each paragraph on one line.
- Changed: Options applied from the selection popover appear in Changes and
  can be reverted.
- Changed: The app logs to `draftmux.log` in the data directory, including
  crashes, however it was started.
- UX: Text in the review's columns is selectable.
- UX: Undo and redo reach the draft from anywhere in the window.
- UX: The editor collapses when the window is too narrow for it beside the
  preview.
- Fix: A handed-off draft whose first draft never started asks for it again
  the next time it opens.
- Fix: A `settings.json` that doesn't parse is no longer overwritten when you
  change a setting.
- Fix: When loading a draft's earlier conversation fails, the chat says so
  and offers Retry.
- Fix: While asking which version of a changed file to keep, the app stops
  saving and hides stale save errors.
- Fix: Changing a draft's details from the app and from `draftmux restore` at
  the same time no longer loses one of the changes.
- Fix: The install script checks it can replace both the app and the command
  before changing either.

Thanks [@erikjuhani](https://github.com/erikjuhani) for great feedback!

## v0.1.0 (2026-11-12)

Initial release.

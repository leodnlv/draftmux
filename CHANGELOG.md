# Changelog

## Unreleased

- A hand-edited `settings.json` that doesn't parse is no longer overwritten
  when you change a setting in the app. The change applies until Draftmux
  restarts, and the Settings window shows the file's error until you fix it.
- Change the text size of the editor, preview and chat with `⌘+` and `⌘−`
  (`Ctrl` on Linux), and reset it with `⌘0`. The size is remembered, and macOS
  gets a View menu with Zoom In, Zoom Out and Actual Size.
- Edit several passages with one chat message. Hold `⌘` (`Ctrl` on Linux)
  while dragging in the editor or the preview to add a selection; the
  selections attach to the chat as quotes, and pi edits each one in place.
- Format no longer needs oxfmt. Markdown formatting is built in for GitHub
  and rich-text drafts; code inside fenced blocks is left unchanged.
- Undo and redo (`⌘Z`/`⌘⇧Z`, `Ctrl+Z`/`Ctrl+Shift+Z` on Linux) now reach the
  draft from anywhere in the window, so an agent's change can be undone right
  after sending a chat message without clicking into the editor. macOS also
  gets an Edit menu with Undo and Redo.
- The agent writes GitHub drafts with each paragraph on one line. GitHub shows
  every line break in a comment, so hard-wrapped drafts came out ragged. When
  your agent writes a GitHub draft into a Markdown file in a repository, the
  skill has it wrap the paragraphs to the repository's line length.
- Format has a menu with Wrap at 80 Columns and Unwrap Paragraphs, for
  wrapping a draft that goes into a Markdown file or unwrapping one that was
  written wrapped.
- The Markdown editor collapses when the window is too narrow to show it
  beside the preview, and comes back when the window widens again. Showing or
  hiding a pane yourself holds until the window crosses that width again.
- `draftmux open --draft-file <path>` hands off a message your agent has
  already written. The draft starts with the file's text and keeps the
  conversation, and Draftmux's agent skips writing a first draft.

## v0.1.0 (2026-11-12)

Initial release.

# Changelog

## Unreleased

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

## v0.1.0 (2026-11-12)

Initial release.

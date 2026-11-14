# Changelog

## Unreleased

- Draft with Claude Code instead of pi: `draftmux open --backend claude`
  continues a Claude Code transcript as the draft's own Claude Code session,
  forked from a copy of the transcript, so the original conversation is left
  as it was. Claude Code may edit only the draft and runs without project
  settings, CLAUDE.md, hooks or MCP servers. The model chip switches the
  draft's model, and the choice is kept for later sessions. Selection actions
  work on these drafts too: each runs on a copy of the conversation, without
  tools and without saving a session, so the draft and its conversation stay
  as they were until you pick an option. They use the draft's model unless
  Settings gives a Claude Code selection model for its directory. The
  `draftmux` agent skill passes `--backend claude` when you ask for Claude
  Code. Needs Claude Code 2.1.293 or later. Drafts are still created with pi
  unless you ask for Claude Code.
- Changes to a draft's details from the app and from `draftmux restore` at
  the same moment no longer undo one another.
- A handed-off draft whose first draft was never started, because Draftmux
  quit or pi failed before taking the request, asks pi for it again the next
  time it opens, instead of staying empty. Once pi has taken the request it is
  never sent again. If pi refuses it, the chat shows the error with Retry.

- While Draftmux asks whether to keep your edits or a version of the draft
  changed outside it, the toolbar no longer shows an earlier "Couldn't save"
  error, and the app stops trying to save until you answer. If the question
  is closed without an answer, it comes back only once the file changes again
  or you edit the draft again.
- Clicking Update Available or Restart to Update in the toolbar shows what
  changed in every version since yours before you update. Linux and
  development builds get the button too when Check for Updates finds a newer
  release.
- The app logs to `draftmux.log` in the data directory however it was started,
  including from Finder, the Dock or Spotlight, and records crashes there with
  a backtrace. Each start is logged with the time and version, and the log is
  moved to `draftmux.log.1` once it passes 1 MB.
- A hand-edited `settings.json` that doesn't parse is no longer overwritten
  when you change a setting in the app. The change applies until Draftmux
  restarts, and the Settings window shows the file's error until you fix it.
- The install script checks that it may replace both the app and the
  `draftmux` command before changing either, and puts the installed app back if
  moving the new one into place fails.
- When a draft's earlier conversation fails to load, the chat says so and
  offers Retry instead of looking like an empty conversation.
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
- `draftmux focus <id>` shows a draft in the app, launching it if needed. When
  `open` creates a draft but can't show it, it now suggests this instead of
  leaving the draft to be found by hand.
- `draftmux restore <id>` unarchives a draft. If the app is running, the draft
  reappears in the sidebar and is selected.
- The sidebar has an Archived section at the bottom, collapsed by default. It
  lists the 20 most recently active archived drafts, with Show All for the
  rest, and each can be restored from its row or its right-click menu.

## v0.1.0 (2026-11-12)

Initial release.

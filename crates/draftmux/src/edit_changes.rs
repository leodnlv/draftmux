//! The changes an agent's edit made, word by word, as the chat's expanded
//! edit row lists them. Hard wraps inside a paragraph are layout, so
//! rewrapping a paragraph doesn't make every line of it a change.

use std::ops::Range;

/// How many words of unchanged text a change shows on either side.
const CONTEXT_WORDS: usize = 4;

/// One patch hunk's edited text and the changes in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditHunk {
    /// The hunk's text after the edit, with its line breaks, as it appears
    /// in the file.
    pub text: String,
    pub changes: Vec<EditChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditChange {
    /// Unchanged text leading up to the change, starting with "…" when it
    /// is cut short.
    pub before: String,
    pub removed: String,
    pub added: String,
    /// Unchanged text after the change, ending with "…" when it is cut
    /// short.
    pub after: String,
    /// What to point at in [`EditHunk::text`]: the added text, or the word
    /// before a removal.
    pub target: Range<usize>,
}

/// The changes in each hunk of a unified patch.
pub fn edit_hunks(patch: &str) -> Vec<EditHunk> {
    patch_sides(patch)
        .into_iter()
        .map(|(old, new)| {
            let changes = changes(&join_wrapped(&old), &join_wrapped(&new));
            EditHunk { text: new, changes }
        })
        .collect()
}

/// Each hunk's text before and after the edit.
fn patch_sides(patch: &str) -> Vec<(String, String)> {
    let mut hunks: Vec<(String, String)> = Vec::new();
    for line in patch.lines() {
        if line.starts_with("@@") {
            hunks.push(Default::default());
            continue;
        }
        let Some((old, new)) = hunks.last_mut() else {
            continue;
        };
        let (kind, rest) = line.split_at(line.len().min(1));
        match kind {
            "+" => push_line(new, rest),
            "-" => push_line(old, rest),
            " " | "" => {
                push_line(old, rest);
                push_line(new, rest);
            }
            // "\ No newline at end of file"
            _ => {}
        }
    }
    hunks
}

fn push_line(text: &mut String, line: &str) {
    text.push_str(line);
    text.push('\n');
}

fn changes(old: &str, new: &str) -> Vec<EditChange> {
    draftmux_core::diff::compare(old, new)
        .hunks
        .into_iter()
        .filter_map(|hunk| {
            let removed = hunk.removed.trim();
            let added = hunk.added.trim();
            if removed.is_empty() && added.is_empty() {
                return None;
            }
            // Whitespace at the change's edges belongs to the context, so
            // "with " → "in " reads as "with" → "in".
            let lead = hunk.added.len() - hunk.added.trim_start().len();
            let trail = hunk.added.len() - hunk.added.trim_end().len();
            let start = hunk.new.start + lead;
            let end = (hunk.new.end - trail).max(start);
            let edge = |text: &str, at_start: bool| {
                let side = if added.is_empty() {
                    &hunk.removed
                } else {
                    &hunk.added
                };
                let spaced = if at_start {
                    side.starts_with(char::is_whitespace)
                } else {
                    side.ends_with(char::is_whitespace)
                };
                spaced && !text.is_empty()
            };
            let mut before = leading_context(&new[..hunk.new.start]);
            if edge(&before, true) && !before.ends_with(char::is_whitespace) {
                before.push(' ');
            }
            let mut after = trailing_context(&new[hunk.new.end..]);
            if edge(&after, false) && !after.starts_with(char::is_whitespace) {
                after.insert(0, ' ');
            }
            let target = if start < end {
                start..end
            } else {
                last_word(new, start)
            };
            Some(EditChange {
                before,
                removed: removed.to_owned(),
                added: added.to_owned(),
                after,
                target,
            })
        })
        .collect()
}

/// The last few words of `text`'s last line.
fn leading_context(text: &str) -> String {
    let line = text.rsplit('\n').next().unwrap_or_default();
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    let kept = words[words.len().saturating_sub(CONTEXT_WORDS)..].join(" ");
    let cut = if words.len() > CONTEXT_WORDS {
        "…"
    } else {
        ""
    };
    let space = if line.ends_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    format!("{cut}{kept}{space}")
}

/// The first few words of `text`'s first line.
fn trailing_context(text: &str) -> String {
    let line = text.split('\n').next().unwrap_or_default();
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    let kept = words[..words.len().min(CONTEXT_WORDS)].join(" ");
    let cut = if words.len() > CONTEXT_WORDS {
        "…"
    } else {
        ""
    };
    let space = if line.starts_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    format!("{space}{kept}{cut}")
}

/// The word that ends at or before `at`, or `at..at` when there is none.
fn last_word(text: &str, at: usize) -> Range<usize> {
    let head = text[..at].trim_end();
    let start = head.rfind(char::is_whitespace).map_or(0, |space| space + 1);
    start..head.len()
}

/// `text` with the line breaks inside paragraphs turned into spaces, at the
/// same byte offsets. Breaks that start a list item, quote, heading, table
/// row or fence, or end a hard break, stay.
fn join_wrapped(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        if i + 1 == lines.len() {
            break;
        }
        if is_fence(line) {
            fenced = !fenced;
        }
        let next = lines[i + 1];
        let joins = !fenced
            && !line.trim().is_empty()
            && !next.trim().is_empty()
            && !line.ends_with("  ")
            && !line.ends_with('\\')
            && !ends_block(line)
            && !starts_block(next);
        out.push(if joins { ' ' } else { '\n' });
    }
    out
}

fn is_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// Lines that are a block of their own.
fn ends_block(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('#') || line.starts_with('|') || is_fence(line)
}

/// Lines that start a block rather than continue a paragraph.
fn starts_block(line: &str) -> bool {
    let line = line.trim_start();
    if ends_block(line) || line.starts_with('>') {
        return true;
    }
    if ["- ", "* ", "+ "]
        .iter()
        .any(|marker| line.starts_with(marker))
    {
        return true;
    }
    // A thematic break or setext underline.
    if line.len() >= 3 && line.chars().all(|c| c == '-' || c == '=' || c == '*') {
        return true;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && (line[digits..].starts_with(". ") || line[digits..].starts_with(") "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(old: &str, new: &str) -> String {
        let mut patch = String::from("--- a\n+++ b\n@@ -1 +1 @@\n");
        for line in old.lines() {
            patch.push_str(&format!("-{line}\n"));
        }
        for line in new.lines() {
            patch.push_str(&format!("+{line}\n"));
        }
        patch
    }

    fn shown(change: &EditChange) -> String {
        let arrow = if change.removed.is_empty() || change.added.is_empty() {
            ""
        } else {
            " → "
        };
        format!(
            "{}[{}]{arrow}{{{}}}{}",
            change.before, change.removed, change.added, change.after
        )
    }

    #[test]
    fn rewrapped_paragraph_is_one_word_change() {
        let hunks = edit_hunks(&patch(
            "Edit it by hand,\nrewrite a selection with one click, or ask\nfor changes in chat.",
            "Edit it by hand, rewrite a\nselection in one click, or ask for\nchanges in chat.",
        ));
        let changes: Vec<String> = hunks[0].changes.iter().map(shown).collect();
        assert_eq!(
            changes,
            ["…hand, rewrite a selection [with] → {in} one click, or ask…"]
        );
        let target = hunks[0].changes[0].target.clone();
        assert_eq!(&hunks[0].text[target], "in");
    }

    #[test]
    fn rewrap_alone_has_no_changes() {
        let hunks = edit_hunks(&patch("one two\nthree four", "one\ntwo three four"));
        assert!(hunks[0].changes.is_empty());
    }

    #[test]
    fn words_carry_their_punctuation() {
        let hunks = edit_hunks(&patch(
            "forked from your session for changes.",
            "forked from your session, so it knows.",
        ));
        assert_eq!(
            shown(&hunks[0].changes[0]),
            "forked from your [session for changes.] → {session, so it knows.}"
        );
    }

    #[test]
    fn removal_points_at_the_word_before() {
        let hunks = edit_hunks(&patch("a quick brown fox", "a brown fox"));
        let change = &hunks[0].changes[0];
        assert_eq!(shown(change), "a [quick]{} brown fox");
        assert_eq!(&hunks[0].text[change.target.clone()], "a");
    }

    #[test]
    fn list_items_stay_on_their_own_lines() {
        assert_eq!(
            join_wrapped("- one\n- two\ncontinued"),
            "- one\n- two continued"
        );
        assert_eq!(join_wrapped("# Title\nbody"), "# Title\nbody");
        assert_eq!(join_wrapped("```\na\nb\n```\nc"), "```\na\nb\n```\nc");
        assert_eq!(join_wrapped("1. first\n2. second"), "1. first\n2. second");
    }

    #[test]
    fn context_stops_at_line_breaks() {
        let hunks = edit_hunks(&patch("- one\n- two old", "- one\n- two new"));
        assert_eq!(shown(&hunks[0].changes[0]), "- two [old] → {new}");
    }
}

//! Where a parsed [`Document`](crate::Document)'s text came from in its
//! source, so a source range (a change in a draft) can be shown on the
//! rendered text without changing what is parsed.
//!
//! A [`SourceMap`] mirrors the document's block tree. Each node with text
//! carries [`Piece`]s that cover that text in order. An exact piece is
//! source copied verbatim; any other piece (an escape, an entity, a line
//! break, text whose source couldn't be followed) maps as a whole, which is
//! coarse but never points at the wrong text.

use std::ops::Range;

/// The source behind a document, one [`Node`] per block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    pub blocks: Vec<Node>,
}

/// A block, list item, table row or table cell.
///
/// Children follow the document's structure: a list's items, an item's,
/// quote's or alert's blocks, and a table's rows (the header row first)
/// with their cells. Paragraphs, headings, cells, code and HTML have text
/// and no children; their text is the concatenation of their spans' text,
/// or the block's own string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    /// The whole block in the source, its syntax included.
    pub source: Range<usize>,
    /// Pieces covering this node's text, in order.
    pub pieces: Vec<Piece>,
    pub children: Vec<Node>,
}

/// A run of rendered text and the source it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// Byte range in the node's text.
    pub text: Range<usize>,
    /// Byte range in the source.
    pub source: Range<usize>,
    /// The source bytes are the text bytes, so any part of one maps to the
    /// same part of the other. A piece that isn't exact maps only whole.
    pub exact: bool,
}

/// Rendered text in the node at `path` (indices into
/// [`SourceMap::blocks`], then into each node's children). A fragment with
/// an empty `text` is an anchor drawn between two characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub path: Vec<usize>,
    pub text: Range<usize>,
}

impl SourceMap {
    /// The rendered text that shows `source`, whose bounds are character
    /// boundaries in the source.
    ///
    /// A range with rendered text gets the parts of its exact pieces it
    /// covers and every other piece it touches whole. An empty range, or
    /// one only in syntax that renders no text (markers, URLs, setext
    /// underlines, link reference definitions), gets a single anchor:
    ///
    /// 1. at `source.start` in the innermost node with text that contains
    ///    it: inside the exact piece there, before the first piece after
    ///    it, or else after that node's last piece;
    /// 2. otherwise after the last piece that starts before it;
    /// 3. otherwise before the document's first piece.
    ///
    /// Text a target adds that isn't in the source, such as a rich-text
    /// alert's label, has no piece, so nothing here points at it.
    ///
    /// Only a document without text gets nothing.
    pub fn locate(&self, source: Range<usize>) -> Vec<Fragment> {
        let mut found: Vec<Fragment> = Vec::new();
        if !source.is_empty() {
            self.each_piece(|path, piece| {
                if piece.source.start >= source.end || source.start >= piece.source.end {
                    return;
                }
                let text = if piece.exact {
                    let start = source.start.max(piece.source.start) - piece.source.start;
                    let end = source.end.min(piece.source.end) - piece.source.start;
                    piece.text.start + start..piece.text.start + end
                } else {
                    piece.text.clone()
                };
                match found.last_mut() {
                    Some(last) if last.path == path && last.text.end == text.start => {
                        last.text.end = text.end;
                    }
                    _ => found.push(Fragment {
                        path: path.to_vec(),
                        text,
                    }),
                }
            });
        }
        if found.is_empty() {
            found.extend(self.anchor(source.start));
        }
        found
    }

    fn anchor(&self, offset: usize) -> Option<Fragment> {
        let at = |path: &[usize], position: usize| Fragment {
            path: path.to_vec(),
            text: position..position,
        };
        let mut path = innermost(&self.blocks, offset, &mut Vec::new()).unwrap_or_default();
        while !path.is_empty() {
            let node = self.node(&path);
            let mut first = None;
            let mut last = None;
            walk(node, &mut path.clone(), &mut |path, piece| {
                if first.is_none() && piece.source.end > offset {
                    let inside = offset.saturating_sub(piece.source.start);
                    let position = piece.text.start + if piece.exact { inside } else { 0 };
                    first = Some(at(path, position));
                }
                last = Some(at(path, piece.text.end));
            });
            if let Some(anchor) = first.or(last) {
                return Some(anchor);
            }
            path.pop();
        }
        let mut before = None;
        let mut first = None;
        self.each_piece(|path, piece| {
            if piece.source.start < offset {
                before = Some(at(path, piece.text.end));
            }
            if first.is_none() {
                first = Some(at(path, piece.text.start));
            }
        });
        before.or(first)
    }

    /// The node at `path`.
    pub fn node(&self, path: &[usize]) -> &Node {
        let (first, rest) = path.split_first().expect("a path names a block");
        rest.iter()
            .fold(&self.blocks[*first], |node, ix| &node.children[*ix])
    }

    /// Every piece in document order, with the path of its node.
    pub fn each_piece(&self, mut visit: impl FnMut(&[usize], &Piece)) {
        let mut path = Vec::new();
        for (ix, node) in self.blocks.iter().enumerate() {
            path.push(ix);
            walk(node, &mut path, &mut visit);
            path.pop();
        }
    }
}

fn walk(node: &Node, path: &mut Vec<usize>, visit: &mut impl FnMut(&[usize], &Piece)) {
    for piece in &node.pieces {
        visit(path, piece);
    }
    for (ix, child) in node.children.iter().enumerate() {
        path.push(ix);
        walk(child, path, visit);
        path.pop();
    }
}

impl Node {
    /// This node and its children with every piece's text moved `by` bytes
    /// later, as when text is put before it.
    pub(crate) fn shift_text(&mut self, by: usize) {
        for piece in &mut self.pieces {
            piece.text = piece.text.start + by..piece.text.end + by;
        }
    }
}

/// Moves a map of `text.replace("\r\n", "\n")` onto `text`: each `\n`
/// that was `\r\n` maps from both bytes, as a piece of its own.
pub(crate) fn restore_crlf(blocks: &mut [Node], text: &str) {
    // Where each `\n` that was a `\r\n` is in the normalized text.
    let crs: Vec<usize> = text
        .match_indices("\r\n")
        .enumerate()
        .map(|(removed, (ix, _))| ix - removed)
        .collect();
    if !crs.is_empty() {
        for node in blocks {
            restore_node(node, &crs);
        }
    }
}

fn restore_node(node: &mut Node, crs: &[usize]) {
    let at = |offset: usize| offset + crs.partition_point(|&cr| cr < offset);
    node.source = at(node.source.start)..at(node.source.end);
    let mut pieces = Vec::with_capacity(node.pieces.len());
    for piece in std::mem::take(&mut node.pieces) {
        let Piece {
            mut text,
            source,
            exact,
        } = piece;
        let mut start = source.start;
        if exact {
            let first = crs.partition_point(|&cr| cr < source.start);
            for &cr in crs[first..].iter().take_while(|&&cr| cr < source.end) {
                let len = cr - start;
                if len > 0 {
                    pieces.push(Piece {
                        text: text.start..text.start + len,
                        source: at(start)..at(cr),
                        exact: true,
                    });
                }
                pieces.push(Piece {
                    text: text.start + len..text.start + len + 1,
                    source: at(cr)..at(cr + 1),
                    exact: false,
                });
                text.start += len + 1;
                start = cr + 1;
            }
            if start == source.end {
                continue;
            }
        }
        pieces.push(Piece {
            text,
            source: at(start)..at(source.end),
            exact,
        });
    }
    node.pieces = pieces;
    for child in &mut node.children {
        restore_node(child, crs);
    }
}

/// The path of the deepest node whose source contains `offset`, its end
/// included.
fn innermost(nodes: &[Node], offset: usize, path: &mut Vec<usize>) -> Option<Vec<usize>> {
    let ix = nodes
        .iter()
        .position(|node| node.source.start <= offset && offset <= node.source.end)?;
    path.push(ix);
    Some(innermost(&nodes[ix].children, offset, path).unwrap_or_else(|| path.clone()))
}

/// Builds the pieces of one node's text.
#[derive(Default)]
pub(crate) struct Pieces {
    pub(crate) pieces: Vec<Piece>,
    /// Length of the node's text so far.
    len: usize,
}

impl Pieces {
    /// Appends `text` as one piece from `source`, exact when the two are
    /// the same bytes.
    pub(crate) fn whole(&mut self, text: &str, source: Range<usize>, src: &str) {
        let exact = src.get(source.clone()) == Some(text);
        self.push(text.len(), source, exact);
    }

    /// Appends `text`, read from `source` by `lex`; a source that `lex`
    /// can't follow to the end maps as one piece.
    pub(crate) fn lexed(&mut self, text: &str, source: Range<usize>, src: &str, lex: Lex) {
        if !self.follows(text, source.clone(), src, lex) {
            self.whole(text, source, src);
        }
    }

    /// Appends `text` read from `source` by `lex`, if `lex` can follow the
    /// source to its end; otherwise appends nothing.
    pub(crate) fn follows(
        &mut self,
        text: &str,
        source: Range<usize>,
        src: &str,
        lex: Lex,
    ) -> bool {
        let (count, len) = (self.pieces.len(), self.len);
        if let Some(slice) = src.get(source.clone())
            && self.follow(slice, source.start, text, lex)
        {
            return true;
        }
        self.pieces.truncate(count);
        self.len = len;
        false
    }

    /// Keeps only the pieces of the text's first `len` bytes.
    pub(crate) fn clip(&mut self, len: usize) {
        self.pieces.retain(|piece| piece.text.start < len);
        if let Some(last) = self.pieces.last_mut()
            && last.text.end > len
        {
            if last.exact {
                last.source.end -= last.text.end - len;
            }
            last.text.end = len;
        }
        self.len = self.len.min(len);
    }

    fn follow(&mut self, source: &str, base: usize, text: &str, lex: Lex) -> bool {
        let (mut s, mut t) = (0, 0);
        while s < source.len() && t < text.len() {
            let (rest, wanted) = (&source[s..], &text[t..]);
            let (taken, given, exact) = match lex.atomic(rest, wanted) {
                Some((taken, given)) => (taken, given, false),
                None => {
                    let c = rest.chars().next().unwrap();
                    if !wanted.starts_with(c) {
                        return false;
                    }
                    (c.len_utf8(), c.len_utf8(), true)
                }
            };
            self.push(given, base + s..base + s + taken, exact);
            s += taken;
            t += given;
        }
        s == source.len() && t == text.len()
    }

    /// Appends a piece of `len` text bytes, joining an exact piece to the
    /// exact piece before it when both texts and sources are contiguous.
    pub(crate) fn push(&mut self, len: usize, source: Range<usize>, exact: bool) {
        if len == 0 {
            return;
        }
        let text = self.len..self.len + len;
        self.len += len;
        if let Some(last) = self.pieces.last_mut()
            && exact
            && last.exact
            && last.source.end == source.start
            && last.text.end == text.start
        {
            last.source.end = source.end;
            last.text.end = text.end;
            return;
        }
        self.pieces.push(Piece {
            text,
            source,
            exact,
        });
    }
}

/// Decodes a character reference such as `&amp;`, when given one.
pub(crate) type Decode<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// What source syntax may stand for other text inside a node.
#[derive(Clone, Copy)]
pub(crate) struct Lex<'a> {
    /// `\` before ASCII punctuation stands for the punctuation.
    pub(crate) escapes: bool,
    /// `\|` stands for `|`, as in a table cell's code.
    pub(crate) pipes: bool,
    /// A line ending, with the next line's indentation and quote markers,
    /// stands for a space, as in code spans.
    pub(crate) newline_space: bool,
    /// Decodes character references.
    pub(crate) entity: Option<&'a Decode<'a>>,
}

impl Lex<'_> {
    /// Only text copied verbatim.
    pub(crate) const EXACT: Lex<'static> = Lex {
        escapes: false,
        pipes: false,
        newline_space: false,
        entity: None,
    };

    /// Source and text bytes of syntax at the start of `source` that stands
    /// for the start of `text`.
    fn atomic(&self, source: &str, text: &str) -> Option<(usize, usize)> {
        let bytes = source.as_bytes();
        let escaped = |c: u8| {
            (self.escapes && c.is_ascii_punctuation() || self.pipes && c == b'|')
                && text.as_bytes().first() == Some(&c)
        };
        if bytes.first() == Some(&b'\\') && bytes.get(1).is_some_and(|&c| escaped(c)) {
            return Some((2, 1));
        }
        if self.newline_space
            && text.starts_with(' ')
            && let Some(rest) = source.strip_prefix('\n').or(source.strip_prefix("\r\n"))
        {
            let prefix = rest.len() - rest.trim_start_matches([' ', '\t', '>']).len();
            return Some((source.len() - rest.len() + prefix, 1));
        }
        if bytes.first() == Some(&b'&')
            && let Some(decode) = self.entity
        {
            let end = source
                .bytes()
                .take(33)
                .position(|c| c == b';')
                .filter(|&end| end > 1)?;
            let reference = &source[..=end];
            if !reference[1..end]
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'#')
            {
                return None;
            }
            let decoded = decode(reference).filter(|decoded| decoded != reference)?;
            return text
                .starts_with(decoded.as_str())
                .then_some((reference.len(), decoded.len()));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexed(source: &str, text: &str, lex: Lex) -> Vec<Piece> {
        let mut pieces = Pieces::default();
        pieces.lexed(text, 0..source.len(), source, lex);
        pieces.pieces
    }

    const TEXT: Lex = Lex {
        escapes: true,
        ..Lex::EXACT
    };

    #[test]
    fn equal_text_is_one_exact_piece() {
        assert_eq!(
            lexed("héllo", "héllo", TEXT),
            [Piece {
                text: 0..6,
                source: 0..6,
                exact: true
            }]
        );
    }

    #[test]
    fn an_escape_maps_whole_between_exact_runs() {
        let pieces = lexed(r"a \* b", "a * b", TEXT);
        assert_eq!(
            pieces,
            [
                Piece {
                    text: 0..2,
                    source: 0..2,
                    exact: true
                },
                Piece {
                    text: 2..3,
                    source: 2..4,
                    exact: false
                },
                Piece {
                    text: 3..5,
                    source: 4..6,
                    exact: true
                },
            ]
        );
    }

    #[test]
    fn a_source_that_cannot_be_followed_maps_whole() {
        assert_eq!(
            lexed("a &amp; b", "a & b", TEXT),
            [Piece {
                text: 0..5,
                source: 0..9,
                exact: false
            }]
        );
    }
}

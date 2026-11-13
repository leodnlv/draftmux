//! Plain text, such as an email body.
//!
//! Nothing is interpreted: markdown markers, HTML, entities and URLs stay
//! literal. The whole text is one unstyled paragraph whose `\n`s are its line
//! breaks, blank lines included, so it reads the way a mail client shows it.

use crate::source_map::{Node, Pieces, SourceMap, restore_crlf};
use crate::{Block, Document, Span, SpanStyle};

pub fn parse(text: &str) -> Document {
    parse_mapped(text).0
}

/// [`parse`], with where its text came from in `text`: all of it is the
/// text itself.
pub fn parse_mapped(text: &str) -> (Document, SourceMap) {
    let normal = text.replace("\r\n", "\n");
    // Trailing line breaks show nothing in a sent email.
    let shown = normal.trim_end_matches('\n');
    if shown.trim().is_empty() {
        return (Document::default(), SourceMap::default());
    }
    let mut pieces = Pieces::default();
    pieces.push(shown.len(), 0..shown.len(), true);
    let mut blocks = vec![Node {
        source: 0..shown.len(),
        pieces: pieces.pieces,
        children: Vec::new(),
    }];
    restore_crlf(&mut blocks, text);
    let doc = Document {
        blocks: vec![Block::Paragraph(vec![Span {
            text: shown.to_owned(),
            style: SpanStyle::default(),
            link: None,
        }])],
    };
    (doc, SourceMap { blocks })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(text: &str) -> Document {
        Document {
            blocks: vec![Block::Paragraph(vec![Span {
                text: text.to_owned(),
                style: SpanStyle::default(),
                link: None,
            }])],
        }
    }

    #[test]
    fn keeps_markup_and_blank_lines_literal() {
        let text = "Hi **Sam**,\n\n\n# Not a heading\n- not a list <b>&amp;</b> [a](https://x.y)\n  indented";
        assert_eq!(parse(text), paragraph(text));
    }

    #[test]
    fn drops_only_trailing_line_breaks() {
        assert_eq!(parse("Thanks,\r\nAlex  \n\n"), paragraph("Thanks,\nAlex  "));
    }

    #[test]
    fn whitespace_only_is_empty() {
        assert_eq!(parse(" \n\t\n"), Document::default());
    }
}

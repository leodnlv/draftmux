//! Syntax highlighting for fenced code, in the token classes GitHub colors
//! code with (Primer's `prettylights` syntax colors). Grammars are bat's
//! Sublime syntaxes, bundled by `two-face`; scopes map to classes as in
//! GitHub's VS Code theme, which follows the site's own mapping.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::OnceLock;

use syntect::highlighting::{
    Color, HighlightState, Highlighter, RangedHighlightIterator, ScopeSelectors, StyleModifier,
    Theme, ThemeItem,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// A class of code text GitHub colors. Text in no class has the code
/// block's text color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Token {
    Comment = 1,
    Constant,
    /// Function names.
    Entity,
    /// Tag names, and markdown quotes.
    Tag,
    Keyword,
    String,
    Variable,
    /// An escape inside a regular expression, bold.
    RegexpEscape,
    /// A markup heading, bold.
    Heading,
    ListMarker,
    Bold,
    Italic,
    Inserted,
    Deleted,
    Changed,
    /// A diff's hunk range, bold.
    DiffRange,
    Invalid,
}

const TOKENS: [Token; 17] = [
    Token::Comment,
    Token::Constant,
    Token::Entity,
    Token::Tag,
    Token::Keyword,
    Token::String,
    Token::Variable,
    Token::RegexpEscape,
    Token::Heading,
    Token::ListMarker,
    Token::Bold,
    Token::Italic,
    Token::Inserted,
    Token::Deleted,
    Token::Changed,
    Token::DiffRange,
    Token::Invalid,
];

/// Scope selectors and the class they give; `None` resets text to the
/// default color. A more specific selector wins, then a later one.
const SCOPES: &[(&str, Option<Token>)] = &[
    (
        "comment, punctuation.definition.comment, string.comment",
        Some(Token::Comment),
    ),
    (
        "constant, entity.name.constant, variable.other.constant, \
         variable.other.enummember, variable.language, entity",
        Some(Token::Constant),
    ),
    (
        "entity.name, meta.export.default, meta.definition.variable",
        Some(Token::Variable),
    ),
    (
        "variable.parameter.function, meta.jsx.children, meta.block, \
         meta.tag.attributes, meta.object.member, meta.embedded.expression",
        None,
    ),
    (
        "entity.name.function, variable.function",
        Some(Token::Entity),
    ),
    ("entity.name.tag, support.class.component", Some(Token::Tag)),
    ("keyword, storage, storage.type", Some(Token::Keyword)),
    (
        "storage.modifier.package, storage.modifier.import, storage.type.java",
        None,
    ),
    ("string", Some(Token::String)),
    (
        "support, meta.property-name, meta.module-reference",
        Some(Token::Constant),
    ),
    ("variable", Some(Token::Variable)),
    ("variable.other", None),
    (
        "invalid.broken, invalid.deprecated, invalid.illegal, \
         invalid.unimplemented, message.error",
        Some(Token::Invalid),
    ),
    ("string variable", Some(Token::Constant)),
    ("source.regexp, string.regexp", Some(Token::String)),
    (
        "string.regexp constant.character.escape",
        Some(Token::RegexpEscape),
    ),
    ("support.type.property-name.json", Some(Token::Tag)),
    (
        "punctuation.definition.list.begin.markdown, \
         punctuation.definition.list_item.markdown",
        Some(Token::ListMarker),
    ),
    (
        "markup.heading, markup.heading entity.name",
        Some(Token::Heading),
    ),
    ("markup.quote", Some(Token::Tag)),
    ("markup.italic", Some(Token::Italic)),
    ("markup.bold", Some(Token::Bold)),
    ("markup.inline.raw, markup.raw", Some(Token::Constant)),
    (
        "markup.deleted, meta.diff.header.from-file, punctuation.definition.deleted",
        Some(Token::Deleted),
    ),
    ("punctuation.section.embedded", Some(Token::Keyword)),
    (
        "markup.inserted, meta.diff.header.to-file, punctuation.definition.inserted",
        Some(Token::Inserted),
    ),
    (
        "markup.changed, punctuation.definition.changed",
        Some(Token::Changed),
    ),
    ("meta.diff.range", Some(Token::DiffRange)),
    (
        "meta.diff.header, meta.separator, meta.output",
        Some(Token::Constant),
    ),
    (
        "constant.other.reference.link, string.other.link",
        Some(Token::String),
    ),
];

/// Fence languages GitHub knows that no grammar lists as a name or
/// extension, and the extension of their grammar.
const ALIASES: &[(&str, &str)] = &[
    ("csharp", "cs"),
    ("docker", "Dockerfile"),
    ("golang", "go"),
    ("shell", "sh"),
    ("zsh", "sh"),
];

/// Code longer than this is left plain, as highlighting it would hold up
/// rendering.
const MAX_LEN: usize = 100_000;

/// Highlighted code kept per thread; it is dropped whole when full.
const CACHE_SIZE: usize = 256;

pub type Tokens = Rc<[(Range<usize>, Token)]>;

/// The classified ranges of `text`, code in the language a fence names, in
/// order. `None` if no grammar is known for `lang`.
pub fn highlight(lang: &str, text: &str) -> Option<Tokens> {
    thread_local! {
        static CACHE: RefCell<HashMap<(String, String), Option<Tokens>>> =
            RefCell::new(HashMap::new());
    }
    let key = (lang.to_owned(), text.to_owned());
    if let Some(hit) = CACHE.with_borrow(|cache| cache.get(&key).cloned()) {
        return hit;
    }
    let tokens = uncached(lang, text).map(Tokens::from);
    CACHE.with_borrow_mut(|cache| {
        if cache.len() >= CACHE_SIZE {
            cache.clear();
        }
        cache.insert(key, tokens.clone());
    });
    tokens
}

/// Loads the grammars, which takes long enough to hold up the first
/// [`highlight`] call when done there instead.
pub fn load() {
    syntaxes();
    theme();
}

fn uncached(lang: &str, text: &str) -> Option<Vec<(Range<usize>, Token)>> {
    if text.len() > MAX_LEN {
        return None;
    }
    let syntaxes = syntaxes();
    let syntax = syntax(syntaxes, lang)?;
    let highlighter = Highlighter::new(theme());
    let mut parse = ParseState::new(syntax);
    let mut state = HighlightState::new(&highlighter, ScopeStack::new());
    let mut out: Vec<(Range<usize>, Token)> = Vec::new();
    let mut at = 0;
    for line in LinesWithEndings::from(text) {
        let ops = parse.parse_line(line, syntaxes).ok()?;
        for (style, _, range) in RangedHighlightIterator::new(&mut state, &ops, line, &highlighter)
        {
            let range = at + range.start..at + range.end;
            let Some(token) = token(style.foreground) else {
                continue;
            };
            match out.last_mut() {
                Some((last, last_token)) if *last_token == token && last.end == range.start => {
                    last.end = range.end;
                }
                _ => out.push((range, token)),
            }
        }
        at += line.len();
    }
    Some(out)
}

fn syntax<'a>(syntaxes: &'a SyntaxSet, lang: &str) -> Option<&'a SyntaxReference> {
    let lang = ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(lang))
        .map_or(lang, |(_, token)| token);
    syntaxes.find_syntax_by_token(lang)
}

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// A theme whose foreground's red channel is the class's [`Token`] value,
/// and 0 for text in no class.
fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| Theme {
        scopes: SCOPES
            .iter()
            .map(|(selectors, token)| ThemeItem {
                scope: ScopeSelectors::from_str(selectors).expect("valid scope selectors"),
                style: StyleModifier {
                    foreground: Some(Color {
                        r: token.map_or(0, |token| token as u8),
                        g: 0,
                        b: 0,
                        a: 0xff,
                    }),
                    ..StyleModifier::default()
                },
            })
            .collect(),
        ..Theme::default()
    })
}

fn token(color: Color) -> Option<Token> {
    TOKENS.iter().copied().find(|token| *token as u8 == color.r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(lang: &str, text: &str) -> Vec<(String, Token)> {
        highlight(lang, text)
            .expect("known language")
            .iter()
            .map(|(range, token)| (text[range.clone()].to_owned(), *token))
            .collect()
    }

    #[test]
    fn rust_is_classified() {
        let tokens = classes("rust", "// hi\nfn main() { let x = \"s\"; }");
        assert!(tokens.contains(&("// hi\n".into(), Token::Comment)));
        assert!(tokens.contains(&("fn".into(), Token::Keyword)));
        assert!(tokens.contains(&("main".into(), Token::Entity)));
        assert!(tokens.contains(&("\"s\"".into(), Token::String)));
    }

    #[test]
    fn aliases_and_extensions_find_grammars() {
        for lang in ["rs", "js", "TypeScript", "shell", "golang", "toml", "py"] {
            assert!(highlight(lang, "x").is_some(), "{lang}");
        }
    }

    #[test]
    fn unknown_languages_are_plain() {
        assert!(highlight("no-such-language", "x").is_none());
    }
}

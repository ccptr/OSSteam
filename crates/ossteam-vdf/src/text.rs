// SPDX-License-Identifier: GPL-2.0-or-later
//! Text KeyValues.
//!
//! ```text
//! // comment
//! "AppState"
//! {
//!     "appid"     "480"
//!     "name"      "Spacewar"   [$WIN32]
//!     unquoted    tokens
//! }
//! ```
//!
//! Quoted strings understand the escapes `\\`, `\"`, `\n`, `\t` and `\r`.
//! Any other backslash sequence is kept verbatim. That keeps Windows paths
//! written without escaping intact, which is how Steam's own reader behaves.

use std::fmt::Write as _;

use crate::{Entry, MAX_DEPTH, Object, Value};

/// A parse error with a 1-based line and column.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{line}:{column}: {kind}")]
pub struct Error {
    pub line: usize,
    pub column: usize,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ErrorKind {
    #[error("unterminated quoted string")]
    UnterminatedString,
    #[error("unterminated conditional")]
    UnterminatedConditional,
    #[error("expected a key, found {0}")]
    ExpectedKey(String),
    #[error("expected a value or '{{' after key {0:?}, found {1}")]
    ExpectedValue(String, String),
    #[error("unexpected '}}' at top level")]
    UnbalancedClose,
    #[error("missing '}}' before end of input")]
    UnclosedObject,
    #[error("directive {0} needs a file name")]
    DirectiveWithoutPath(String),
    #[error("unknown directive {0}")]
    UnknownDirective(String),
    #[error("nesting deeper than {MAX_DEPTH} levels")]
    TooDeep,
}

/// The kind of a file-level directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveKind {
    /// `#include "file"`: the named file's entries are added alongside this
    /// file's.
    Include,
    /// `#base "file"`: the named file supplies entries this file does not
    /// define.
    Base,
}

/// A `#include` or `#base` line. The parser records these and does not
/// resolve them; reading other files is the caller's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    pub kind: DirectiveKind,
    pub path: String,
}

/// A parsed text file: its directives and its top-level entries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    pub directives: Vec<Directive>,
    pub root: Object,
}

/// Parses a whole file. Most files have one top-level key, but several are
/// allowed and all are kept in `root`.
pub fn parse_document(input: &str) -> Result<Document, Error> {
    let mut p = Parser::new(input);
    let mut doc = Document::default();
    doc.root = p.parse_entries(0, Some(&mut doc.directives))?;
    Ok(doc)
}

/// Parses a file and returns its top-level entries. Directives are an
/// error here; use [`parse_document`] for files that may contain them.
pub fn parse(input: &str) -> Result<Object, Error> {
    let mut p = Parser::new(input);
    p.parse_entries(0, None)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Open,
    Close,
    /// A quoted or unquoted string. The flag is true for quoted strings,
    /// which never act as directives.
    Str(String, bool),
    Conditional(String),
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Token::Open => "'{'".into(),
            Token::Close => "'}'".into(),
            Token::Str(s, _) => format!("{s:?}"),
            Token::Conditional(c) => format!("[{c}]"),
        }
    }
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
    col: usize,
    peeked: Option<(Token, usize, usize)>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        Self {
            src,
            pos: 0,
            line: 1,
            col: 1,
            peeked: None,
        }
    }

    fn err(&self, line: usize, column: usize, kind: ErrorKind) -> Error {
        Error { line, column, kind }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.src[self.pos..].chars().next()?;
        self.pos += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek_char() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.src[self.pos..].starts_with("//") => {
                    while let Some(c) = self.peek_char() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => return,
            }
        }
    }

    /// Returns the next token with its starting position, or `None` at end
    /// of input.
    fn next_token(&mut self) -> Result<Option<(Token, usize, usize)>, Error> {
        if let Some(t) = self.peeked.take() {
            return Ok(Some(t));
        }
        self.skip_trivia();
        let (line, col) = (self.line, self.col);
        let Some(c) = self.peek_char() else {
            return Ok(None);
        };
        let tok = match c {
            '{' => {
                self.bump();
                Token::Open
            }
            '}' => {
                self.bump();
                Token::Close
            }
            '"' => {
                self.bump();
                Token::Str(self.quoted(line, col)?, true)
            }
            '[' => {
                self.bump();
                let start = self.pos;
                loop {
                    match self.bump() {
                        Some(']') => break,
                        Some('\n') | None => {
                            return Err(self.err(line, col, ErrorKind::UnterminatedConditional));
                        }
                        Some(_) => {}
                    }
                }
                Token::Conditional(self.src[start..self.pos - 1].to_owned())
            }
            _ => {
                let start = self.pos;
                while let Some(c) = self.peek_char() {
                    if c.is_whitespace() || matches!(c, '"' | '{' | '}') {
                        break;
                    }
                    if c == '/' && self.src[self.pos..].starts_with("//") {
                        break;
                    }
                    self.bump();
                }
                Token::Str(self.src[start..self.pos].to_owned(), false)
            }
        };
        Ok(Some((tok, line, col)))
    }

    fn peek_token(&mut self) -> Result<Option<&Token>, Error> {
        if self.peeked.is_none() {
            self.peeked = self.next_token()?;
        }
        Ok(self.peeked.as_ref().map(|(t, _, _)| t))
    }

    fn quoted(&mut self, line: usize, col: usize) -> Result<String, Error> {
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err(line, col, ErrorKind::UnterminatedString)),
                Some('"') => return Ok(out),
                Some('\\') => match self.peek_char() {
                    Some('\\') => {
                        self.bump();
                        out.push('\\');
                    }
                    Some('"') => {
                        self.bump();
                        out.push('"');
                    }
                    Some('n') => {
                        self.bump();
                        out.push('\n');
                    }
                    Some('t') => {
                        self.bump();
                        out.push('\t');
                    }
                    Some('r') => {
                        self.bump();
                        out.push('\r');
                    }
                    _ => out.push('\\'),
                },
                Some(c) => out.push(c),
            }
        }
    }

    fn take_conditional(&mut self) -> Result<Option<String>, Error> {
        if let Some(Token::Conditional(_)) = self.peek_token()?
            && let Some((Token::Conditional(c), _, _)) = self.next_token()?
        {
            return Ok(Some(c));
        }
        Ok(None)
    }

    fn parse_entries(
        &mut self,
        depth: usize,
        mut directives: Option<&mut Vec<Directive>>,
    ) -> Result<Object, Error> {
        let mut obj = Object::new();
        loop {
            let Some((tok, line, col)) = self.next_token()? else {
                if depth > 0 {
                    return Err(self.err(self.line, self.col, ErrorKind::UnclosedObject));
                }
                return Ok(obj);
            };
            let key = match tok {
                Token::Close if depth > 0 => return Ok(obj),
                Token::Close => return Err(self.err(line, col, ErrorKind::UnbalancedClose)),
                Token::Str(s, false) if depth == 0 && s.starts_with('#') => {
                    let kind = match s.to_ascii_lowercase().as_str() {
                        "#include" => DirectiveKind::Include,
                        "#base" => DirectiveKind::Base,
                        _ => return Err(self.err(line, col, ErrorKind::UnknownDirective(s))),
                    };
                    let path = match self.next_token()? {
                        Some((Token::Str(p, _), _, _)) => p,
                        _ => return Err(self.err(line, col, ErrorKind::DirectiveWithoutPath(s))),
                    };
                    match directives.as_deref_mut() {
                        Some(list) => list.push(Directive { kind, path }),
                        None => return Err(self.err(line, col, ErrorKind::UnknownDirective(s))),
                    }
                    continue;
                }
                Token::Str(s, _) => s,
                other => return Err(self.err(line, col, ErrorKind::ExpectedKey(other.describe()))),
            };

            let mut condition = self.take_conditional()?;
            let value = match self.next_token()? {
                Some((Token::Open, _, _)) => {
                    if depth + 1 >= MAX_DEPTH {
                        return Err(self.err(line, col, ErrorKind::TooDeep));
                    }
                    Value::Object(self.parse_entries(depth + 1, None)?)
                }
                Some((Token::Str(s, _), _, _)) => Value::String(s),
                Some((other, l, c)) => {
                    return Err(self.err(l, c, ErrorKind::ExpectedValue(key, other.describe())));
                }
                None => {
                    let found = "end of input".to_owned();
                    return Err(self.err(
                        self.line,
                        self.col,
                        ErrorKind::ExpectedValue(key, found),
                    ));
                }
            };
            if let Some(c) = self.take_conditional()? {
                condition = Some(c);
            }
            obj.entries.push(Entry {
                key,
                value,
                condition,
            });
        }
    }
}

/// Serialises entries as text KeyValues in the layout Steam writes: tab
/// indentation, braces on their own lines, two tabs between key and value.
pub fn to_string(obj: &Object) -> String {
    let mut out = String::new();
    write_entries(&mut out, obj, 0);
    out
}

/// Serialises a [`Document`], directives first.
pub fn document_to_string(doc: &Document) -> String {
    let mut out = String::new();
    for d in &doc.directives {
        let name = match d.kind {
            DirectiveKind::Include => "#include",
            DirectiveKind::Base => "#base",
        };
        let _ = writeln!(out, "{name} \"{}\"", escape(&d.path));
    }
    write_entries(&mut out, &doc.root, 0);
    out
}

fn write_entries(out: &mut String, obj: &Object, depth: usize) {
    for e in &obj.entries {
        indent(out, depth);
        let _ = write!(out, "\"{}\"", escape(&e.key));
        match &e.value {
            Value::Object(child) => {
                if let Some(c) = &e.condition {
                    let _ = write!(out, " [{c}]");
                }
                out.push('\n');
                indent(out, depth);
                out.push_str("{\n");
                write_entries(out, child, depth + 1);
                indent(out, depth);
                out.push_str("}\n");
            }
            scalar => {
                let s = scalar.to_text_scalar().unwrap_or_default();
                let _ = write!(out, "\t\t\"{}\"", escape(&s));
                if let Some(c) = &e.condition {
                    let _ = write!(out, " [{c}]");
                }
                out.push('\n');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
// leading comment
"AppState"
{
	"appid"		"480"
	"name"		"Spacewar"
	"installdir"		"C:\\Games\\Spacewar"
	"InstalledDepots"
	{
		"481"
		{
			"manifest"		"3183503801510301321"
			"size"		"1234"
		}
	}
}
"#;

    #[test]
    fn parses_app_manifest() {
        let root = parse(MANIFEST).unwrap();
        let app = root.get_object("appstate").unwrap();
        assert_eq!(app.get_str("appid"), Some("480"));
        assert_eq!(app.get_str("installdir"), Some(r"C:\Games\Spacewar"));
        let m = app
            .get_path(&["InstalledDepots", "481", "manifest"])
            .unwrap();
        assert_eq!(m.as_u64(), Some(3183503801510301321));
    }

    #[test]
    fn round_trips_through_writer() {
        let first = parse(MANIFEST).unwrap();
        let written = to_string(&first);
        assert_eq!(parse(&written).unwrap(), first);
        assert!(written.starts_with("\"AppState\"\n{\n\t\"appid\"\t\t\"480\"\n"));
    }

    #[test]
    fn unquoted_tokens_conditionals_and_duplicates() {
        let src = "root {\n key value [$WIN32]\n key \"two\"\n sub [!$OSX] { a b }\n}";
        let root = parse(src).unwrap();
        let r = root.get_object("root").unwrap();
        assert_eq!(r.entries[0].condition.as_deref(), Some("$WIN32"));
        assert_eq!(r.get_all("key").count(), 2);
        assert_eq!(r.entries[2].condition.as_deref(), Some("!$OSX"));
        assert_eq!(r.get_path(&["sub", "a"]).and_then(Value::as_str), Some("b"));
        let again = parse(&to_string(&root)).unwrap();
        assert_eq!(again, root);
    }

    #[test]
    fn escapes_and_unknown_backslashes() {
        let root = parse(r#""k" "a\"b\\c\nd\qe""#).unwrap();
        assert_eq!(root.get_str("k"), Some("a\"b\\c\nd\\qe"));
        let again = parse(&to_string(&root)).unwrap();
        assert_eq!(again, root);
    }

    #[test]
    fn comment_directly_after_unquoted_token() {
        let root = parse("a b// trailing\nc d").unwrap();
        assert_eq!(root.get_str("a"), Some("b"));
        assert_eq!(root.get_str("c"), Some("d"));
    }

    #[test]
    fn directives_are_recorded() {
        let doc = parse_document("#base \"base.vdf\"\n#include x.vdf\n\"a\" \"b\"").unwrap();
        assert_eq!(
            doc.directives,
            [
                Directive {
                    kind: DirectiveKind::Base,
                    path: "base.vdf".into()
                },
                Directive {
                    kind: DirectiveKind::Include,
                    path: "x.vdf".into()
                },
            ]
        );
        assert_eq!(doc.root.get_str("a"), Some("b"));
        assert_eq!(parse_document(&document_to_string(&doc)).unwrap(), doc);
        assert!(matches!(
            parse("#include x.vdf").unwrap_err().kind,
            ErrorKind::UnknownDirective(_)
        ));
    }

    #[test]
    fn bom_and_empty_input() {
        assert!(parse("").unwrap().is_empty());
        assert_eq!(
            parse("\u{feff}\"a\" \"b\"").unwrap().get_str("a"),
            Some("b")
        );
    }

    #[test]
    fn errors_carry_positions() {
        let e = parse("\"a\"\n{\n \"b\" \"c\"\n").unwrap_err();
        assert_eq!(e.kind, ErrorKind::UnclosedObject);
        let e = parse("\"a\" \"b\"\n}").unwrap_err();
        assert_eq!(
            (e.line, e.column, e.kind),
            (2, 1, ErrorKind::UnbalancedClose)
        );
        let e = parse("\"a").unwrap_err();
        assert_eq!(e.kind, ErrorKind::UnterminatedString);
        let e = parse("\"a\"").unwrap_err();
        assert!(matches!(e.kind, ErrorKind::ExpectedValue(..)));
        let e = parse("{").unwrap_err();
        assert!(matches!(e.kind, ErrorKind::ExpectedKey(_)));
    }

    #[test]
    fn rejects_excessive_nesting() {
        let src = "a {".repeat(MAX_DEPTH + 1);
        assert_eq!(parse(&src).unwrap_err().kind, ErrorKind::TooDeep);
    }
}

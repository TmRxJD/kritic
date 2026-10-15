//! Conservative JS/TS lexical import recognition. Strings/comments never become code.
#[derive(PartialEq)]
enum Kind {
    Identifier,
    String,
    Punctuation,
    Template,
}
struct Token {
    kind: Kind,
    value: String,
    line: usize,
}

/// Return literal static specifiers and lines requiring dynamic/escaped import review.
/// Template interpolation and language-specific syntax are deliberately not evaluated.
pub(super) fn imports(text: &str) -> (Vec<(usize, String)>, Vec<usize>) {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let (mut cursor, mut line) = (0, 1);
    while cursor < bytes.len() {
        let ch = bytes[cursor];
        if ch.is_ascii_whitespace() {
            if ch == b'\n' {
                line += 1;
            }
            cursor += 1;
        } else if bytes[cursor..].starts_with(b"//") {
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
        } else if bytes[cursor..].starts_with(b"/*") {
            cursor += 2;
            while cursor < bytes.len() && !bytes[cursor..].starts_with(b"*/") {
                if bytes[cursor] == b'\n' {
                    line += 1;
                }
                cursor += 1;
            }
            cursor = (cursor + 2).min(bytes.len());
        } else if matches!(ch, b'\'' | b'"' | b'`') {
            let token_line = line;
            cursor += 1;
            let start = cursor;
            while cursor < bytes.len() && bytes[cursor] != ch {
                if bytes[cursor] == b'\n' {
                    line += 1;
                }
                if bytes[cursor] == b'\\' && cursor + 1 < bytes.len() {
                    cursor += 1;
                    if bytes[cursor] == b'\n' {
                        line += 1;
                    }
                }
                cursor += 1;
            }
            tokens.push(Token {
                kind: if ch == b'`' {
                    Kind::Template
                } else {
                    Kind::String
                },
                value: text[start..cursor].into(),
                line: token_line,
            });
            cursor = (cursor + 1).min(bytes.len());
        } else if ch.is_ascii_alphabetic() || matches!(ch, b'_' | b'$') {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric() || matches!(bytes[cursor], b'_' | b'$'))
            {
                cursor += 1;
            }
            tokens.push(Token {
                kind: Kind::Identifier,
                value: text[start..cursor].into(),
                line,
            });
        } else {
            // Only ASCII punctuation is significant to these import patterns.
            if ch.is_ascii() {
                tokens.push(Token {
                    kind: Kind::Punctuation,
                    value: (ch as char).to_string(),
                    line,
                });
            }
            cursor += 1;
        }
    }
    let (mut specifiers, mut gaps) = (vec![], vec![]);
    // Slash syntax can be division or a regex literal; avoid hard findings on an
    // ambiguous line rather than pretending this small lexer is a language parser.
    let ambiguous_lines: std::collections::BTreeSet<_> = tokens
        .iter()
        .filter(|token| token.kind == Kind::Punctuation && token.value == "/")
        .map(|token| token.line)
        .collect();
    let mut declaration = false;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind == Kind::Template {
            // Interpolation may contain arbitrary executable imports; never claim coverage.
            if token.value.contains("${") {
                gaps.push(token.line);
            }
            continue;
        }
        if token.value == ";" && token.kind == Kind::Punctuation {
            declaration = false;
        }
        if token.kind != Kind::Identifier {
            continue;
        }
        let next = tokens.get(index + 1);
        let property = index > 0 && tokens[index - 1].value == ".";
        if property {
            continue;
        }
        if token.value == "import" || token.value == "export" {
            declaration = true;
        }
        let literal = if token.value == "import" && next.is_some_and(|t| t.kind == Kind::String) {
            next
        } else if token.value == "from"
            && declaration
            && next.is_some_and(|t| t.kind == Kind::String)
        {
            declaration = false;
            next
        } else if token.value == "require" && next.is_some_and(|t| t.value == "(") {
            let argument = tokens.get(index + 2);
            if !argument.is_some_and(|t| t.kind == Kind::String) {
                gaps.push(token.line);
            }
            argument.filter(|t| t.kind == Kind::String)
        } else {
            if token.value == "import" && next.is_some_and(|t| t.value == "(") {
                gaps.push(token.line);
                declaration = false;
            }
            None
        };
        if let Some(literal) = literal {
            if literal.value.contains('\\') || ambiguous_lines.contains(&token.line) {
                gaps.push(token.line);
            } else {
                specifiers.push((token.line, literal.value.clone()));
            }
        }
    }
    gaps.sort_unstable();
    gaps.dedup();
    (specifiers, gaps)
}

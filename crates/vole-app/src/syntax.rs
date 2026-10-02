//! Editor highlighting through text decorations, one function per document language.
use crate::theme::*;
use gpui::{HighlightStyle, rgb};
use gpui_kit::component::input::TextDecoration;
use vole_core::SourceLanguage;

pub fn tokens(language: SourceLanguage, source: &str) -> Vec<TextDecoration> {
    match language {
        SourceLanguage::Assembly => assembly_tokens(source),
        SourceLanguage::C => c_tokens(source),
    }
}

fn decoration(range: std::ops::Range<usize>, color: u32) -> TextDecoration {
    TextDecoration::new(
        range,
        HighlightStyle {
            color: Some(rgb(color).into()),
            ..Default::default()
        },
    )
}

pub fn assembly_tokens(source: &str) -> Vec<TextDecoration> {
    let mut tokens = Vec::new();
    let mut base = 0;
    for line in source.split_inclusive('\n') {
        let comment = line
            .char_indices()
            .find(|(_, c)| *c == ';' || *c == '@')
            .map(|(i, _)| i)
            .or_else(|| line.find("//"));
        let code = &line[..comment.unwrap_or(line.len())];
        let bytes = code.as_bytes();
        let mut index = 0;
        let mut first = true;
        while index < bytes.len() {
            if !(bytes[index].is_ascii_alphanumeric()
                || bytes[index] == b'_'
                || bytes[index] == b'.')
            {
                index += 1;
                continue;
            }
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'.'))
            {
                index += 1;
            }
            let token = &code[start..index];
            let lower = token.to_ascii_lowercase();
            let color = if lower.starts_with('r')
                && lower[1..].chars().all(|c| c.is_ascii_hexdigit())
                || lower.starts_with('x') && lower[1..].chars().all(|c| c.is_ascii_digit())
                || lower.starts_with('w') && lower[1..].chars().all(|c| c.is_ascii_digit())
                || matches!(
                    lower.as_str(),
                    "rax"
                        | "rbx"
                        | "rcx"
                        | "rdx"
                        | "eax"
                        | "ebx"
                        | "ecx"
                        | "edx"
                        | "rsp"
                        | "rbp"
                        | "esp"
                        | "ebp"
                        | "sp"
                        | "lr"
                        | "pc"
                ) {
                Some(INK)
            } else if token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                Some(WRITE)
            } else if first && bytes.get(index) != Some(&b':') {
                Some(READ)
            } else {
                None
            };
            if let Some(color) = color {
                tokens.push(decoration(base + start..base + index, color));
            }
            first = false;
        }
        if let Some(comment) = comment {
            tokens.push(decoration(
                base + comment..base + line.trim_end_matches('\n').len(),
                MUTED,
            ));
        }
        base += line.len();
    }
    tokens
}

const C_KEYWORDS: &[&str] = &[
    "if",
    "else",
    "for",
    "while",
    "do",
    "switch",
    "case",
    "default",
    "break",
    "continue",
    "return",
    "goto",
    "sizeof",
    "typedef",
    "static",
    "extern",
    "const",
    "volatile",
    "inline",
    "register",
    "restrict",
    "_Noreturn",
    "_Static_assert",
    "_Alignas",
    "_Alignof",
    "true",
    "false",
    "NULL",
];
const C_TYPES: &[&str] = &[
    "void",
    "char",
    "short",
    "int",
    "long",
    "signed",
    "unsigned",
    "_Bool",
    "bool",
    "struct",
    "union",
    "enum",
    "float",
    "double",
    "size_t",
    "ptrdiff_t",
    "intptr_t",
    "uintptr_t",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CToken {
    Keyword,
    Type,
    Number,
    Text,
    Comment,
    Preprocessor,
}

impl CToken {
    fn color(self) -> u32 {
        match self {
            Self::Keyword => READ,
            Self::Type => SYNTAX_TYPE,
            Self::Number => WRITE,
            Self::Text => SYNTAX_STRING,
            Self::Comment => SYNTAX_COMMENT,
            Self::Preprocessor => SYNTAX_PREPROCESSOR,
        }
    }
}

/// Lexes C well enough for highlighting: comments may span lines, strings and
/// character literals honor escapes, and a `#` directive colors its whole line
/// except its trailing comment.
pub fn c_spans(source: &str) -> Vec<(std::ops::Range<usize>, CToken)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    let mut line_start = true;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            line_start = true;
            index += 1;
            continue;
        }
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            spans.push((start..index, CToken::Comment));
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index < bytes.len()
                && !(bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/'))
            {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            spans.push((start..index, CToken::Comment));
            line_start = false;
            continue;
        }
        if byte == b'#' && line_start {
            while index < bytes.len()
                && bytes[index] != b'\n'
                && !(bytes[index] == b'/' && matches!(bytes.get(index + 1), Some(b'/' | b'*')))
            {
                index += 1;
            }
            let mut end = index;
            while end > start && bytes[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            spans.push((start..end, CToken::Preprocessor));
            line_start = false;
            continue;
        }
        line_start = false;
        if byte == b'"' || byte == b'\'' {
            index += 1;
            while index < bytes.len() && bytes[index] != byte && bytes[index] != b'\n' {
                index += if bytes[index] == b'\\' { 2 } else { 1 };
            }
            index = (index + 1).min(bytes.len());
            spans.push((start..index, CToken::Text));
            continue;
        }
        if byte.is_ascii_digit()
            || (byte == b'.' && bytes.get(index + 1).is_some_and(u8::is_ascii_digit))
        {
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'.')
            {
                index += 1;
            }
            spans.push((start..index, CToken::Number));
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            let word = &source[start..index];
            if C_TYPES.contains(&word) {
                spans.push((start..index, CToken::Type));
            } else if C_KEYWORDS.contains(&word) {
                spans.push((start..index, CToken::Keyword));
            }
            continue;
        }
        // Skip one whole UTF-8 character so ranges stay on character boundaries.
        index += source[index..].chars().next().map_or(1, char::len_utf8);
    }
    spans
}

pub fn c_tokens(source: &str) -> Vec<TextDecoration> {
    c_spans(source)
        .into_iter()
        .map(|(range, token)| decoration(range, token.color()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<(&str, CToken)> {
        c_spans(source)
            .into_iter()
            .map(|(range, token)| (&source[range], token))
            .collect()
    }

    #[test]
    fn c_lexer_classifies_tokens() {
        let source = "#include <vole.h> // runtime\nint main(void) {\n    char c = 'x'; /* a\n b */ return 0x1F + \"s\\\"t\";\n}\n";
        assert_eq!(
            kinds(source),
            vec![
                ("#include <vole.h>", CToken::Preprocessor),
                ("// runtime", CToken::Comment),
                ("int", CToken::Type),
                ("void", CToken::Type),
                ("char", CToken::Type),
                ("'x'", CToken::Text),
                ("/* a\n b */", CToken::Comment),
                ("return", CToken::Keyword),
                ("0x1F", CToken::Number),
                ("\"s\\\"t\"", CToken::Text),
            ]
        );
    }

    #[test]
    fn c_lexer_handles_unterminated_and_unicode() {
        let source = "/* open\n\"never closed\nint é = 1;";
        let spans = c_spans(source);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1, CToken::Comment);
        let source = "int é = 1; \"x";
        assert!(
            c_spans(source)
                .iter()
                .all(|(r, _)| source.is_char_boundary(r.start) && source.is_char_boundary(r.end))
        );
    }
}

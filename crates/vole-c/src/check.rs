//! Source checks that run before Clang: header allow list and unsupported features.
//!
//! A small preprocessing-token lexer handles line continuations, comments,
//! string and character literals, so words inside text are never reported.
use vole_core::{Diagnostic, Severity};

/// Headers a document may include. Any other include could read host files.
pub const ALLOWED_HEADERS: &[&str] = &[
    "vole.h",
    "stdint.h",
    "stddef.h",
    "stdbool.h",
    "limits.h",
    "stdarg.h",
    "stdalign.h",
    "stdnoreturn.h",
    "iso646.h",
    "float.h",
];

const MAX_REPORTS: usize = 20;

const SCOPE_HINT: &str = "Vole's C environment is freestanding integer C: char, short, int, \
    long and long long (signed and unsigned), _Bool, enums, pointers, arrays, structs and \
    unions, running on one thread without an operating system.";

/// A character of the source after line splicing, with its original position.
#[derive(Clone, Copy)]
struct Char {
    value: char,
    line: usize,
    column: usize,
}

/// Read the document the way Clang's preprocessor does: drop a leading byte
/// order mark, treat CR, CRLF and LF as line ends, splice a backslash
/// followed by optional horizontal whitespace and a line end, and read the
/// `%:` digraph as `#`. Trigraphs are disabled with `-fno-trigraphs`.
fn splice(source: &str) -> Vec<Char> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let source = source.replace("\r\n", "\n").replace('\r', "\n");
    let mut result = Vec::with_capacity(source.len());
    for (index, text) in source.split('\n').enumerate() {
        let line = index + 1;
        let mut column = 0;
        for value in text.chars() {
            column += 1;
            result.push(Char {
                value,
                line,
                column,
            });
        }
        result.push(Char {
            value: '\n',
            line,
            column: column + 1,
        });
    }
    result.pop();
    let mut spliced = Vec::with_capacity(result.len());
    let mut index = 0;
    while index < result.len() {
        if result[index].value == '\\' {
            let mut next = index + 1;
            while result.get(next).is_some_and(|c| {
                c.value == ' ' || c.value == '\t' || c.value == '\x0c' || c.value == '\x0b'
            }) {
                next += 1;
            }
            if result.get(next).is_some_and(|c| c.value == '\n') {
                index = next + 1;
                continue;
            }
        }
        spliced.push(result[index]);
        index += 1;
    }
    let mut normalized: Vec<Char> = Vec::with_capacity(spliced.len());
    let mut index = 0;
    while index < spliced.len() {
        if spliced[index].value == '%' && spliced.get(index + 1).is_some_and(|c| c.value == ':') {
            normalized.push(Char {
                value: '#',
                ..spliced[index]
            });
            index += 2;
        } else {
            normalized.push(spliced[index]);
            index += 1;
        }
    }
    normalized
}

fn forbidden_word(word: &str) -> Option<(String, &'static str)> {
    let floating = "Vole's teaching machines run integer instructions only; there are no \
        floating-point registers. Use integers, for example cents instead of dollars or \
        tenths of a degree.";
    let message = |what: &str| format!("{what} is outside the Vole C teaching scope");
    Some(match word {
        "float" | "double" => (message(&format!("Floating-point type '{word}'")), floating),
        "_Float16" | "__fp16" | "__bf16" | "_Float32" | "_Float64" | "_Float128" | "__float128"
        | "_Float32x" | "_Float64x" => {
            (message(&format!("Floating-point type '{word}'")), floating)
        }
        "_Complex" | "__complex__" | "_Imaginary" => {
            (message(&format!("Complex number type '{word}'")), floating)
        }
        "__int128" | "__int128_t" | "__uint128_t" => (
            message("The 128-bit integer type"),
            "The largest supported integer type is long long (64 bits).",
        ),
        "_Atomic" => (
            message("'_Atomic'"),
            "A Vole program runs on a single thread; use ordinary or volatile variables.",
        ),
        "_Thread_local" | "thread_local" | "__thread" => (
            message(&format!("Thread-local storage ('{word}')")),
            "A Vole program runs on a single thread; use an ordinary global or static variable.",
        ),
        "asm" | "__asm" | "__asm__" => (
            message("Inline assembly"),
            "Write the instructions in an assembly document instead, or call the runtime \
             functions declared in vole.h.",
        ),
        "__has_include" | "__has_include_next" => (
            format!("'{word}' is not available in Vole documents"),
            "Documents cannot probe for files. Include <vole.h> or one of the freestanding \
             headers directly.",
        ),
        "_Pragma" => (
            "'_Pragma' is not available in Vole documents".into(),
            "Pragmas can change how the compiler reads files and reports positions.",
        ),
        _ => return None,
    })
}

fn report(line: usize, column: usize, message: impl Into<String>, hint: &str) -> Diagnostic {
    Diagnostic::at(line, column, Severity::Error, message).with_hint(hint)
}

struct Lexer {
    chars: Vec<Char>,
    index: usize,
}

impl Lexer {
    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.index + offset).map(|c| c.value)
    }

    fn position(&self) -> (usize, usize) {
        self.chars
            .get(self.index)
            .or(self.chars.last())
            .map_or((1, 1), |c| (c.line, c.column))
    }

    /// Skips a comment at the cursor. Returns false if there is none.
    fn skip_comment(&mut self) -> bool {
        match (self.peek(0), self.peek(1)) {
            (Some('/'), Some('/')) => {
                while self.peek(0).is_some_and(|c| c != '\n') {
                    self.index += 1;
                }
                true
            }
            (Some('/'), Some('*')) => {
                self.index += 2;
                while self.index < self.chars.len()
                    && !(self.peek(0) == Some('*') && self.peek(1) == Some('/'))
                {
                    self.index += 1;
                }
                self.index = (self.index + 2).min(self.chars.len());
                true
            }
            _ => false,
        }
    }

    /// Skips horizontal whitespace and comments inside a directive line.
    fn skip_blank(&mut self) {
        loop {
            match self.peek(0) {
                Some(c) if c != '\n' && c.is_whitespace() => self.index += 1,
                _ if self.skip_comment() => {}
                _ => break,
            }
        }
    }

    fn identifier(&mut self) -> String {
        let mut word = String::new();
        while let Some(c) = self
            .peek(0)
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        {
            word.push(c);
            self.index += 1;
        }
        word
    }

    /// Skips a quoted literal; stops at an unescaped quote or the end of the line.
    fn skip_quoted(&mut self, quote: char) {
        self.index += 1;
        while let Some(c) = self.peek(0) {
            self.index += 1;
            if c == '\\' {
                if self.peek(0).is_some_and(|c| c != '\n') {
                    self.index += 1;
                }
            } else if c == quote {
                return;
            } else if c == '\n' {
                self.index -= 1;
                return;
            }
        }
    }

    /// Reads a preprocessing number and reports whether it is a floating constant.
    fn number(&mut self) -> bool {
        let start = self.index;
        let mut previous = '\0';
        while let Some(c) = self.peek(0) {
            let exponent_sign = (c == '+' || c == '-') && matches!(previous, 'e' | 'E' | 'p' | 'P');
            if !(c.is_alphanumeric() || c == '_' || c == '.' || exponent_sign) {
                break;
            }
            previous = c;
            self.index += 1;
        }
        let text: String = self.chars[start..self.index]
            .iter()
            .map(|c| c.value.to_ascii_lowercase())
            .collect();
        let hex = text.starts_with("0x");
        text.contains('.') || (hex && text.contains('p')) || (!hex && text.contains('e'))
    }

    fn directive(&mut self, diagnostics: &mut Vec<Diagnostic>) {
        let (hash_line, hash_column) = self.position();
        self.index += 1;
        self.skip_blank();
        let (line, column) = self.position();
        let name = self.identifier();
        match name.as_str() {
            "include" => {
                self.skip_blank();
                let (line, column) = self.position();
                let header = match self.peek(0) {
                    Some(open @ ('<' | '"')) => {
                        let close = if open == '<' { '>' } else { '"' };
                        self.index += 1;
                        let mut header = String::new();
                        while let Some(c) = self.peek(0).filter(|c| *c != close && *c != '\n') {
                            header.push(c);
                            self.index += 1;
                        }
                        Some(header.trim().to_string())
                    }
                    _ => None,
                };
                match header {
                    Some(header) if ALLOWED_HEADERS.contains(&header.as_str()) => {}
                    Some(header) => diagnostics.push(report(
                        line,
                        column,
                        format!("Cannot include '{header}' in a Vole document"),
                        &format!(
                            "Vole C programs are freestanding: there is no host C library, \
                             heap, input or file system, and documents cannot read other \
                             files. Allowed headers: {}.",
                            allowed_list()
                        ),
                    )),
                    None => diagnostics.push(report(
                        line,
                        column,
                        "#include must name a header directly, as in #include <vole.h>",
                        "Headers chosen through macros cannot be checked against the allow list.",
                    )),
                }
            }
            "include_next" | "import" | "embed" => diagnostics.push(report(
                line,
                column,
                format!("#{name} is not available in Vole documents"),
                &format!(
                    "Use #include with one of the allowed headers: {}.",
                    allowed_list()
                ),
            )),
            name if name.starts_with(|c: char| c.is_ascii_digit()) => diagnostics.push(report(
                line,
                column,
                "Line markers are not available in Vole documents",
                "The debugger maps every instruction back to the real lines of this document.",
            )),
            "line" => diagnostics.push(report(
                line,
                column,
                "#line is not available in Vole documents",
                "The debugger maps every instruction back to the real lines of this document.",
            )),
            "pragma" => {
                self.skip_blank();
                let first = self.identifier();
                self.skip_blank();
                let second = self.identifier();
                let allowed = first == "once"
                    || (matches!(first.as_str(), "GCC" | "clang") && second == "diagnostic");
                if !allowed {
                    diagnostics.push(report(
                        hash_line,
                        hash_column,
                        "Only '#pragma once' and diagnostic pragmas are available in Vole documents",
                        "Other pragmas can change how the compiler reads files and lays out code.",
                    ));
                }
            }
            _ => {}
        }
    }
}

fn allowed_list() -> String {
    ALLOWED_HEADERS
        .iter()
        .map(|header| format!("<{header}>"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reports forbidden includes, directives, keywords and floating constants.
pub(crate) fn check(source: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer {
        chars: splice(source),
        index: 0,
    };
    let mut diagnostics = Vec::new();
    let mut line_start = true;
    while let Some(c) = lexer.peek(0) {
        if diagnostics.len() >= MAX_REPORTS {
            break;
        }
        if c == '\n' {
            line_start = true;
            lexer.index += 1;
        } else if c.is_whitespace() || lexer.skip_comment() {
            if c.is_whitespace() {
                lexer.index += 1;
            }
        } else if c == '#' && line_start {
            lexer.directive(&mut diagnostics);
            line_start = false;
        } else if c == '"' || c == '\'' {
            lexer.skip_quoted(c);
            line_start = false;
        } else if c.is_ascii_digit()
            || (c == '.' && lexer.peek(1).is_some_and(|d| d.is_ascii_digit()))
        {
            let (line, column) = lexer.position();
            if lexer.number() {
                diagnostics.push(report(
                    line,
                    column,
                    "Floating-point constants are outside the Vole C teaching scope",
                    "Vole's teaching machines run integer instructions only; there are no \
                     floating-point registers. Use integer constants.",
                ));
            }
            line_start = false;
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let (line, column) = lexer.position();
            let word = lexer.identifier();
            if let Some((message, hint)) = forbidden_word(&word) {
                diagnostics.push(report(
                    line,
                    column,
                    message,
                    &format!("{hint} {SCOPE_HINT}"),
                ));
            }
            line_start = false;
        } else {
            lexer.index += 1;
            line_start = false;
        }
    }
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(source: &str) -> Vec<(usize, usize)> {
        check(source).iter().map(|d| (d.line, d.column)).collect()
    }

    #[test]
    fn keywords_outside_text_are_reported() {
        assert_eq!(lines("int main(void) { float x; }"), [(1, 18)]);
        assert_eq!(lines("long\ndouble y;"), [(2, 1)]);
        assert_eq!(lines("void f(void) { __asm__(\"nop\"); }"), [(1, 16)]);
        assert_eq!(
            lines("_Atomic int a; _Thread_local int b;"),
            [(1, 1), (1, 16)]
        );
        assert_eq!(
            lines("int x = 1.5; int y = 1e3; int z = 0x1p2;"),
            [(1, 9), (1, 22), (1, 35)]
        );
    }

    #[test]
    fn comments_strings_and_continuations_are_handled() {
        assert!(
            check("// float\n/* double\n asm */ char *s = \"float\"; char c = 'f';").is_empty()
        );
        assert!(check("int floaty; int x = 0xE5; int y = 10e;").len() <= 1);
        assert_eq!(lines("int x; fl\\\noat y;"), [(1, 8)]);
        assert!(check("#define S \"a\\\" float\"\nint x;").is_empty());
    }

    #[test]
    fn source_is_read_like_the_preprocessor() {
        let reported = |source: &str| {
            lines(source)
                .into_iter()
                .map(|(line, _)| line)
                .collect::<Vec<_>>()
        };
        assert_eq!(reported("%:include \"/etc/passwd\""), [1]);
        assert_eq!(reported("\u{feff}#include </etc/passwd>"), [1]);
        assert_eq!(reported("int x;\r#include </etc/passwd>"), [2]);
        assert_eq!(reported("#inc\\ \nlude </etc/passwd>"), [2]);
        assert_eq!(reported("# 900 \"/etc/hosts\""), [1]);
        assert_eq!(reported("#line 9"), [1]);
        assert!(check("int x;\r\nint y;\r\n").is_empty());
    }

    #[test]
    fn includes_are_checked() {
        assert!(check("#include <vole.h>\n#  include \"stdint.h\"\n").is_empty());
        assert_eq!(lines("#include <stdio.h>"), [(1, 10)]);
        assert_eq!(lines("  # /* c */ include </etc/passwd>"), [(1, 21)]);
        assert_eq!(lines("#inc\\\nlude <stdlib.h>"), [(2, 6)]);
        assert_eq!(lines("#define H <stdio.h>\n#include H"), [(2, 10)]);
        assert_eq!(lines("#include_next <vole.h>"), [(1, 2)]);
        assert_eq!(lines("#import <vole.h>"), [(1, 2)]);
        assert_eq!(
            lines("int x; # include <stdio.h>"),
            Vec::<(usize, usize)>::new()
        );
        assert_eq!(lines("#if __has_include(<stdio.h>)\n#endif"), [(1, 5)]);
    }
}

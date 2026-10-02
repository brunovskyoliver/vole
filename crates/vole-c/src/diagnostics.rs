//! Converts Clang and LLD messages into document diagnostics.
use vole_core::{Diagnostic, Severity};

const MAX_DIAGNOSTICS: usize = 64;

/// Functions the runtime provides, listed in link-error hints.
pub(crate) const RUNTIME_FUNCTIONS: &str = "vole_putc, vole_print, vole_println, \
    vole_print_int, vole_print_uint, vole_print_hex, vole_exit, putchar, puts, printf, memset, \
    memcpy, memmove, memcmp and strlen";

/// Converts a 1-based byte column on a source line into a 1-based character column.
fn character_column(source: &str, line: usize, byte_column: usize) -> usize {
    let Some(text) = source.lines().nth(line.saturating_sub(1)) else {
        return byte_column;
    };
    let end = byte_column.saturating_sub(1).min(text.len());
    let end = (0..=end)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    text[..end].chars().count() + 1
}

struct Located<'a> {
    file: &'a str,
    line: usize,
    column: usize,
    severity: Severity,
    message: &'a str,
}

fn parse_clang_line(text: &str) -> Option<Located<'_>> {
    for (marker, severity) in [
        (": fatal error: ", Severity::Error),
        (": error: ", Severity::Error),
        (": warning: ", Severity::Warning),
        (": note: ", Severity::Note),
    ] {
        let Some((position, message)) = text.split_once(marker) else {
            continue;
        };
        let mut parts = position.rsplitn(3, ':');
        let column = parts.next()?;
        let line = parts.next();
        let file = parts.next();
        let (file, line, column) = match (file, line) {
            (Some(file), Some(line)) => (file, line.parse().ok()?, column.parse().ok()?),
            // `file:line: message` without a column.
            (None, Some(file)) => (file, column.parse().ok()?, 1),
            _ => return None,
        };
        return Some(Located {
            file,
            line,
            column,
            severity,
            message: message.trim(),
        });
    }
    None
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Parses Clang's stderr for the user's document (compiled as `main.c`).
pub(crate) fn clang(output: &str, source: &str) -> Vec<Diagnostic> {
    let mut result: Vec<Diagnostic> = Vec::new();
    let mut include_line = 1;
    for text in output.lines() {
        if let Some(rest) = text
            .strip_prefix("In file included from ")
            .or_else(|| text.strip_prefix("                 from "))
        {
            if let Some(line) = rest
                .strip_prefix("main.c:")
                .and_then(|rest| rest.trim_end_matches(&[':', ','][..]).parse().ok())
            {
                include_line = line;
            }
            continue;
        }
        let Some(located) = parse_clang_line(text) else {
            // Driver errors without a position, e.g. `clang-14: error: ...`.
            if let Some((_, message)) = text
                .split_once("error: ")
                .filter(|(prefix, _)| prefix.starts_with("clang") || prefix.is_empty())
            {
                result.push(Diagnostic::new(1, format!("Compiler: {message}")));
            }
            continue;
        };
        let diagnostic = if located.file == "main.c" {
            include_line = 1;
            Diagnostic::at(
                located.line,
                character_column(source, located.line, located.column),
                located.severity,
                located.message,
            )
        } else {
            // Positions inside headers point at the #include line of the document.
            let header = file_name(located.file);
            let anchor = if located.severity == Severity::Note {
                result.last().map_or(include_line, |last| last.line)
            } else {
                include_line
            };
            let mut diagnostic = Diagnostic::at(
                anchor,
                1,
                located.severity,
                format!("{header}:{}: {}", located.line, located.message),
            );
            if located.severity != Severity::Note {
                diagnostic = diagnostic.with_hint(if header == "vole.h" {
                    format!(
                        "The problem is reported inside the runtime header vole.h. This usually \
                         means the document declares something with the same name as a runtime \
                         function ({RUNTIME_FUNCTIONS}) but a different type."
                    )
                } else {
                    format!(
                        "The problem is reported inside the compiler's header {header}, \
                         usually because of a conflicting declaration in the document."
                    )
                });
            }
            diagnostic
        };
        result.push(diagnostic);
        if result.len() == MAX_DIAGNOSTICS {
            break;
        }
    }
    result
}

/// Finds the first use of `identifier` in the document as a whole word, skipping
/// comments and literals with a simple scan. `only_line` restricts the result.
fn find_identifier(
    source: &str,
    identifier: &str,
    only_line: Option<usize>,
) -> Option<(usize, usize)> {
    let mut in_block_comment = false;
    for (index, line) in source.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut position = 0;
        let mut quote = None;
        while position < chars.len() {
            let c = chars[position];
            if in_block_comment {
                if c == '*' && chars.get(position + 1) == Some(&'/') {
                    in_block_comment = false;
                    position += 1;
                }
            } else if let Some(q) = quote {
                if c == '\\' {
                    position += 1;
                } else if c == q {
                    quote = None;
                }
            } else if c == '/' && chars.get(position + 1) == Some(&'/') {
                break;
            } else if c == '/' && chars.get(position + 1) == Some(&'*') {
                in_block_comment = true;
                position += 1;
            } else if c == '"' || c == '\'' {
                quote = Some(c);
            } else if c.is_alphabetic() || c == '_' {
                let start = position;
                while position < chars.len()
                    && (chars[position].is_alphanumeric() || chars[position] == '_')
                {
                    position += 1;
                }
                let word: String = chars[start..position].iter().collect();
                if word == identifier && only_line.is_none_or(|line| line == index + 1) {
                    return Some((index + 1, start + 1));
                }
                continue;
            }
            position += 1;
        }
    }
    None
}

/// Parses LLD's stderr into document diagnostics.
pub(crate) fn linker(output: &str, source: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = output.lines().collect();
    let mut result = Vec::new();
    for (index, text) in lines.iter().enumerate() {
        let Some((_, message)) = text.split_once("error: ") else {
            continue;
        };
        if text.starts_with(">>>") {
            continue;
        }
        // `>>> referenced by main.c:5 (./main.c:5)` follows undefined-symbol errors.
        let referenced_line = lines[index + 1..]
            .iter()
            .take_while(|line| line.starts_with(">>>"))
            .find_map(|line| {
                let rest = line.trim_start_matches('>').trim();
                let rest = rest.strip_prefix("referenced by main.c:")?;
                rest.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse::<usize>()
                    .ok()
            });
        let diagnostic = if let Some(symbol) = message.strip_prefix("undefined symbol: ") {
            let symbol = symbol.trim();
            if symbol == "main" {
                Diagnostic::new(1, "The program has no main function")
                    .with_hint("Define the entry point as: int main(void) { ... return 0; }")
            } else {
                let (line, column) = find_identifier(source, symbol, referenced_line)
                    .or_else(|| referenced_line.map(|line| (line, 1)))
                    .unwrap_or((1, 1));
                let mut diagnostic = Diagnostic::at(
                    line,
                    column,
                    Severity::Error,
                    format!("'{symbol}' is not defined in this document or the Vole runtime"),
                )
                .with_hint(format!(
                    "Vole C programs are freestanding: the runtime has no heap (malloc), no \
                     input (scanf, getchar) and no files (fopen). Available runtime functions: \
                     {RUNTIME_FUNCTIONS}."
                ));
                if symbol.starts_with("__") {
                    diagnostic.hint = Some(format!(
                        "The compiler needs the support routine {symbol}, which the Vole \
                         runtime does not provide. This usually comes from floating-point or \
                         128-bit arithmetic, which are outside the teaching scope."
                    ));
                }
                diagnostic
            }
        } else if let Some(symbol) = message.strip_prefix("duplicate symbol: ") {
            let symbol = symbol.trim();
            let (line, column) = find_identifier(source, symbol, None).unwrap_or((1, 1));
            Diagnostic::at(
                line,
                column,
                Severity::Error,
                format!("'{symbol}' is already defined by the Vole runtime"),
            )
            .with_hint("Rename the function or variable in your document.")
        } else if message.contains("VOLE_CODE_TOO_LARGE")
            || (message.contains("overlaps") && message.contains(".text"))
        {
            Diagnostic::new(
                1,
                "The program's machine code exceeds the 28 KiB code region (0x1000-0x7FFF)",
            )
            .with_hint("Make the program smaller, or try -O1 to produce less code.")
        } else if message.contains("VOLE_RODATA_TOO_LARGE")
            || (message.contains("overlaps") && message.contains(".rodata"))
        {
            Diagnostic::new(
                1,
                "Constant data and string literals exceed the 8 KiB read-only region \
                 (0x8000-0x9FFF)",
            )
            .with_hint("Shorten string literals or const arrays.")
        } else if message.contains("VOLE_DATA_TOO_LARGE") {
            Diagnostic::new(
                1,
                "Global and static variables exceed the 24 KiB data region (0xA000-0xFFFF)",
            )
            .with_hint(
                "Make global arrays smaller. Large local arrays use the 64 KiB stack instead.",
            )
        } else {
            Diagnostic::new(1, format!("Linker: {}", message.trim()))
        };
        result.push(diagnostic);
        if result.len() == MAX_DIAGNOSTICS {
            break;
        }
    }
    if result.is_empty() {
        result.push(Diagnostic::new(
            1,
            format!("Linking failed: {}", output.trim()),
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clang_lines_are_mapped() {
        let source =
            "#include <vole.h>\nint strlen(int);\nint main(void) { int é = 1; return y; }\n";
        let output = "main.c:2:5: error: conflicting types for 'strlen'\n\
            int strlen(int);\n    ^\n\
            include/vole.h:36:8: note: previous declaration is here\n\
            main.c:3:33: error: use of undeclared identifier 'y'\n\
            main.c:3:22: warning: unused variable [-Wunused-variable]\n\
            2 errors generated.\n";
        let diagnostics = clang(output, source);
        assert_eq!(diagnostics.len(), 4);
        assert_eq!((diagnostics[0].line, diagnostics[0].column), (2, 5));
        assert_eq!(diagnostics[1].severity, Severity::Note);
        assert_eq!(diagnostics[1].line, 2);
        assert!(diagnostics[1].message.starts_with("vole.h:36:"));
        // 'é' is two bytes; byte column 33 is character column 32.
        assert_eq!((diagnostics[2].line, diagnostics[2].column), (3, 32));
        assert_eq!(diagnostics[3].severity, Severity::Warning);
    }

    #[test]
    fn header_errors_point_to_the_include() {
        let output = "In file included from main.c:3:\ninclude/vole.h:10:1: error: bad\n";
        let diagnostics = clang(output, "");
        assert_eq!(diagnostics[0].line, 3);
        assert!(diagnostics[0].hint.is_some());
    }

    #[test]
    fn linker_errors_are_explained() {
        let source = "// malloc\nint main(void) {\n  char *p = malloc(4);\n}\n";
        let output = "ld.lld: error: undefined symbol: malloc\n>>> referenced by main.c:3 (./main.c:3)\n>>>               main.o:(main)\n";
        let diagnostics = linker(output, source);
        assert_eq!((diagnostics[0].line, diagnostics[0].column), (3, 13));
        assert!(diagnostics[0].hint.as_deref().unwrap().contains("heap"));
        let missing = linker(
            "ld.lld: error: undefined symbol: main\n>>> referenced by crt0.s:12\n",
            "",
        );
        assert!(missing[0].message.contains("no main"));
    }
}

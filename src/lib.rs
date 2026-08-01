use logos::Logos;
use std::fmt;

#[derive(Logos, Debug, PartialEq)]
pub enum Token {
    #[regex(r"[ \n\t\f]+")]
    Whitespace,

    // Comments start with # and go to end of line
    #[regex(r"#.*")]
    Comment,

    // Include or inherit directives
    #[token("include")]
    IncludeKw,
    #[token("require")]
    RequireKw,
    #[token("inherit")]
    InheritKw,
    #[token("export")]
    ExportKw,
    #[token("unset")]
    UnsetKw,
    #[token("addtask")]
    AddtaskKw,
    #[token("deltask")]
    DeltaskKw,
    #[token("python")]
    PythonKw,
    #[token("fakeroot")]
    FakerootKw,

    // Assignment operators: =, :=, ?=, ??=, +=, .=
    #[token("=")]
    Assign,
    #[token(":=")]
    WeakAssign,
    #[token("?=")]
    ConditionalAssign,
    #[token("??=")]
    LazyDefaultAssign,
    #[token("+=")]
    AppendAssign,
    #[token(".=")]
    PrependAssign,

    // Punctuation
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("\\")]
    LineContinuation,

    // Overrides separated by spaces or in variable names with ':'
    // Example: SRC_URI[append] or FILES_${PN} or VAR:append
    #[regex(r"[A-Za-z0-9_]+(:[A-Za-z0-9_]+)?", priority = 2)]
    Ident,

    // Variable references like ${VAR} or ${@python}
    #[regex(r"\$\{[^}]*\}")]
    VarRef,

    // Strings: double-quoted and single-quoted (keep the quotes in slice)
    #[regex(r#""([^"\\]|\\.)*""#)]
    DqString,
    #[regex(r"'([^'\\]|\\.)*'")]
    SqString,
}

pub fn get_line_col(text: &str, index: usize) -> (usize, usize) {
    let prefix = &text[..index];
    let line = prefix.chars().filter(|&c| c == '\n').count() + 1;
    let last_newline = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = index - last_newline + 1;
    (line, col)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError {
    line: usize,
    message: String,
}

impl FormatError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }

    pub fn line(&self) -> usize {
        self.line
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for FormatError {}

/// Formats the subset of BitBake syntax that can be changed safely.
///
/// Embedded shell and Python function bodies, Python `def` blocks, continued
/// statements, and unsupported top-level syntax are preserved byte-for-byte.
/// Structurally incomplete input produces an error instead of partial output.
pub fn format(text: &str) -> Result<String, FormatError> {
    let mut output = String::new();
    let mut offset = 0;
    let mut line_number = 1;

    while offset < text.len() {
        let line_end = next_line_end(text, offset);
        let line = &text[offset..line_end];

        if let Some((opening_brace, function_kind)) = function_opening_brace(line) {
            let block_end = find_brace_block_end(text, offset + opening_brace, function_kind)
                .ok_or_else(|| {
                    FormatError::new(line_number, "function body has no closing brace")
                })?;
            let block = &text[offset..block_end];
            output.push_str(block);
            line_number += count_newlines(block);
            offset = block_end;
            continue;
        }

        if is_python_def_start(line) {
            let block_end = find_python_def_end(text, line_end);
            let block = &text[offset..block_end];
            output.push_str(block);
            line_number += count_newlines(block);
            offset = block_end;
            continue;
        }

        if has_line_continuation(line) {
            let block_end = find_continuation_end(text, offset).ok_or_else(|| {
                FormatError::new(
                    line_number,
                    "statement ends with an unterminated continuation",
                )
            })?;
            let block = &text[offset..block_end];
            output.push_str(block);
            line_number += count_newlines(block);
            offset = block_end;
            continue;
        }

        if is_blank_line(line) {
            append_normalized_blank_line(&mut output, line);
        } else if let Some(formatted) = format_top_level_assignment(line, line_number)? {
            output.push_str(&formatted);
        } else {
            output.push_str(line);
        }

        line_number += count_newlines(line);
        offset = line_end;
    }

    Ok(output)
}

fn next_line_end(text: &str, start: usize) -> usize {
    text[start..]
        .find('\n')
        .map(|relative| start + relative + 1)
        .unwrap_or(text.len())
}

fn count_newlines(text: &str) -> usize {
    text.bytes().filter(|&byte| byte == b'\n').count()
}

fn split_line_ending(line: &str) -> (&str, &str) {
    if let Some(content) = line.strip_suffix("\r\n") {
        (content, "\r\n")
    } else if let Some(content) = line.strip_suffix('\n') {
        (content, "\n")
    } else {
        (line, "")
    }
}

fn is_blank_line(line: &str) -> bool {
    let (content, _) = split_line_ending(line);
    content.trim_matches([' ', '\t', '\r']).is_empty()
}

fn append_normalized_blank_line(output: &mut String, line: &str) {
    let (_, line_ending) = split_line_ending(line);
    if line_ending.is_empty() {
        return;
    }

    if !output.ends_with("\n\n") && !output.ends_with("\r\n\r\n") {
        output.push_str(line_ending);
    }
}

#[derive(Clone, Copy)]
enum FunctionKind {
    Shell,
    Python,
}

fn function_opening_brace(line: &str) -> Option<(usize, FunctionKind)> {
    let (content, _) = split_line_ending(line);
    let code = &content[..comment_start(content).unwrap_or(content.len())];
    let code = code.trim_end();
    let opening_brace = code.strip_suffix('{')?;
    let signature = opening_brace.trim_end();
    let closing_paren = signature.strip_suffix(')')?;
    let opening_paren = closing_paren.rfind('(')?;

    if !closing_paren[opening_paren + 1..].trim().is_empty() {
        return None;
    }

    let declaration = closing_paren[..opening_paren].trim();
    let (kind, name) = if let Some(name) = strip_keyword(declaration, "python") {
        (FunctionKind::Python, name)
    } else if let Some(name) = strip_keyword(declaration, "fakeroot") {
        (FunctionKind::Shell, name)
    } else {
        (FunctionKind::Shell, declaration)
    };

    if name.is_empty() && !matches!(kind, FunctionKind::Python) {
        return None;
    }
    if !name.is_empty() && !is_function_name(name) {
        return None;
    }

    Some((code.len() - 1, kind))
}

fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(keyword)?;
    if rest.is_empty() {
        Some(rest)
    } else if rest.starts_with(char::is_whitespace) {
        Some(rest.trim_start())
    } else {
        None
    }
}

fn is_function_name(name: &str) -> bool {
    !name
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'(' | b')' | b'#'))
}

fn comment_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote = None;
    let mut escaped = false;

    for (index, &byte) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if byte == active_quote {
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'#' {
            return Some(index);
        }
    }

    None
}

fn find_brace_block_end(
    text: &str,
    opening_brace: usize,
    function_kind: FunctionKind,
) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut index = opening_brace;

    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            index += 1;
            continue;
        }
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            index += 1;
            continue;
        }
        if let Some(active_quote) = quote {
            if byte == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }

        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'#' if is_comment_start_in_function(bytes, index, function_kind) => comment = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(next_line_end(text, index));
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

fn is_comment_start_in_function(bytes: &[u8], index: usize, function_kind: FunctionKind) -> bool {
    if matches!(function_kind, FunctionKind::Python) {
        return true;
    }

    index == 0
        || bytes[index - 1].is_ascii_whitespace()
        || matches!(
            bytes[index - 1],
            b';' | b'|' | b'&' | b'(' | b')' | b'{' | b'}'
        )
}

fn is_python_def_start(line: &str) -> bool {
    let (content, _) = split_line_ending(line);
    if content.starts_with(char::is_whitespace) {
        return false;
    }
    let code = &content[..comment_start(content).unwrap_or(content.len())];
    let code = code.trim_end();
    code.starts_with("def ") && code.ends_with(':')
}

fn find_python_def_end(text: &str, mut offset: usize) -> usize {
    while offset < text.len() {
        let line_end = next_line_end(text, offset);
        let line = &text[offset..line_end];
        let (content, _) = split_line_ending(line);
        if !content.trim().is_empty() && !content.starts_with(char::is_whitespace) {
            break;
        }
        offset = line_end;
    }
    offset
}

fn has_line_continuation(line: &str) -> bool {
    let (content, _) = split_line_ending(line);
    let content = content.trim_end_matches([' ', '\t', '\r']);
    let backslashes = content
        .as_bytes()
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count();
    backslashes % 2 == 1
}

fn find_continuation_end(text: &str, start: usize) -> Option<usize> {
    let mut offset = start;
    loop {
        let line_end = next_line_end(text, offset);
        let line = &text[offset..line_end];
        if !has_line_continuation(line) {
            return Some(line_end);
        }
        if line_end == text.len() {
            return None;
        }
        offset = line_end;
    }
}

fn format_top_level_assignment(
    line: &str,
    line_number: usize,
) -> Result<Option<String>, FormatError> {
    let (content, line_ending) = split_line_ending(line);
    if content.starts_with(char::is_whitespace) {
        return Ok(None);
    }

    let Some((operator_start, operator)) = find_assignment_operator(content) else {
        return Ok(None);
    };
    let left = content[..operator_start].trim_end();
    if !is_assignment_left_hand_side(left) {
        return Ok(None);
    }

    let right = &content[operator_start + operator.len()..];
    if !has_balanced_quotes(right) {
        return Err(FormatError::new(
            line_number,
            "top-level assignment contains an unclosed quote",
        ));
    }

    let right = right.trim_start_matches([' ', '\t']);
    let mut formatted = String::with_capacity(line.len() + 2);
    formatted.push_str(left);
    formatted.push(' ');
    formatted.push_str(operator);
    if !right.is_empty() {
        formatted.push(' ');
        formatted.push_str(right);
    }
    formatted.push_str(line_ending);
    Ok(Some(formatted))
}

fn find_assignment_operator(content: &str) -> Option<(usize, &'static str)> {
    const OPERATORS: [&str; 8] = ["??=", ":=", "?=", "+=", ".=", "=+", "=.", "="];

    let bytes = content.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            index += 1;
            continue;
        }
        if let Some(active_quote) = quote {
            if byte == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        if byte == b'#' {
            return None;
        }

        if let Some(operator) = OPERATORS
            .iter()
            .find(|operator| bytes[index..].starts_with(operator.as_bytes()))
        {
            return Some((index, *operator));
        }
        index += 1;
    }

    None
}

fn is_assignment_left_hand_side(left: &str) -> bool {
    let name = left
        .strip_prefix("export ")
        .or_else(|| left.strip_prefix("export\t"))
        .unwrap_or(left);
    if name.is_empty()
        || name.starts_with(char::is_whitespace)
        || name.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return false;
    }

    let Some(first) = name.bytes().next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }

    name.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'_' | b':' | b'-' | b'+' | b'.' | b'$' | b'{' | b'}' | b'@' | b'[' | b']'
            )
    })
}

fn has_balanced_quotes(text: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;

    for byte in text.bytes() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if byte == active_quote {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'#' {
            break;
        }
    }

    quote.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_line_col() {
        let text = "line1\nline2\nline3";
        assert_eq!(get_line_col(text, 0), (1, 1));
        assert_eq!(get_line_col(text, 5), (1, 6)); // newline char
        assert_eq!(get_line_col(text, 6), (2, 1));
        assert_eq!(get_line_col(text, 11), (2, 6)); // newline char
        assert_eq!(get_line_col(text, 12), (3, 1));
    }

    #[test]
    fn test_token_lexer() {
        let text = r#"
            include file
            VAR = "value"
            ${PN}
        "#;
        let mut lex = Token::lexer(text);

        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::IncludeKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident))); // file
        assert_eq!(lex.slice(), "file");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident))); // VAR
        assert_eq!(lex.slice(), "VAR");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Assign)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DqString)));
        assert_eq!(lex.slice(), "\"value\"");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::VarRef)));
        assert_eq!(lex.slice(), "${PN}");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
    }

    #[test]
    fn test_simple_tokens() {
        let mut lex = Token::lexer("VAR = \"val\"");
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "VAR");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Assign)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DqString)));
        assert_eq!(lex.slice(), "\"val\"");
    }

    #[test]
    fn test_keywords() {
        let mut lex =
            Token::lexer("include require inherit export unset addtask deltask python fakeroot");
        assert_eq!(lex.next(), Some(Ok(Token::IncludeKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::RequireKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::InheritKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::ExportKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::UnsetKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::AddtaskKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DeltaskKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::PythonKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::FakerootKw)));
    }

    #[test]
    fn test_new_syntax() {
        let text = r#"
            VAR ??= "val"
            VAR[flag] = "val"
            do_foo() {
                return 0
            }
        "#;
        let mut lex = Token::lexer(text);

        // VAR ??= "val"
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "VAR");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::LazyDefaultAssign)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DqString)));

        // VAR[flag] = "val"
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "VAR");
        assert_eq!(lex.next(), Some(Ok(Token::LBracket)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "flag");
        assert_eq!(lex.next(), Some(Ok(Token::RBracket)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Assign)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DqString)));

        // do_foo() {
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "do_foo");
        assert_eq!(lex.next(), Some(Ok(Token::LParen)));
        assert_eq!(lex.next(), Some(Ok(Token::RParen)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::LBrace)));

        // return 0
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "return");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident))); // 0 is parsed as Ident for now
        assert_eq!(lex.slice(), "0");

        // }
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::RBrace)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
    }

    #[test]
    fn test_format_spacing() {
        let input = "VAR=\"val\"";
        let expected = "VAR = \"val\"";
        assert_eq!(format(input).unwrap(), expected);

        let input = "VAR  =  \"val\"";
        let expected = "VAR = \"val\"";
        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_spacing_insertion() {
        let input = "VAR=\"val\"";
        let expected = "VAR = \"val\"";
        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_blank_lines() {
        let input = "VAR = \"val\"\n\n\n\nVAR2 = \"val\"";
        let expected = "VAR = \"val\"\n\nVAR2 = \"val\"";
        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_preserves_crlf_line_endings() {
        let input = "VAR=\"val\"\r\n\r\n\r\nOTHER=\"val\"\r\n";
        let expected = "VAR = \"val\"\r\n\r\nOTHER = \"val\"\r\n";

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_handles_utf8_without_panicking() {
        let input = "SUMMARY=\"Résumé de l’application\"\n";
        let expected = "SUMMARY = \"Résumé de l’application\"\n";

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_preserves_unsupported_top_level_syntax() {
        let input = "include conf/distro/example.inc\n!unsupported=value\nVAR=\"formatted\"\n";
        let expected = "include conf/distro/example.inc\n!unsupported=value\nVAR = \"formatted\"\n";

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_preserves_shell_function_body() {
        let input = concat!(
            "VAR=\"value\"\n",
            "do_configure() {\n",
            "    ./configure --prefix=/usr\n",
            "    local value=unchanged\n",
            "}\n",
            "OTHER  =  \"value\"\n",
        );
        let expected = concat!(
            "VAR = \"value\"\n",
            "do_configure() {\n",
            "    ./configure --prefix=/usr\n",
            "    local value=unchanged\n",
            "}\n",
            "OTHER = \"value\"\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_preserves_nested_braces_in_function_body() {
        let input = concat!(
            "do_install() {\n",
            "    helper() {\n",
            "        VALUE=unchanged\n",
            "    }\n",
            "    echo \"${VALUE}\"\n",
            "}\n",
            "VAR=\"formatted\"\n",
        );

        let formatted = format(input).unwrap();
        assert!(formatted.contains("        VALUE=unchanged\n"));
        assert!(formatted.contains("    echo \"${VALUE}\"\n"));
        assert!(formatted.ends_with("VAR = \"formatted\"\n"));
    }

    #[test]
    fn test_format_preserves_bitbake_python_function_body() {
        let input = concat!(
            "python do_example() {\n",
            "    values = {\"key\": \"value\"}\n",
            "    result=values[\"key\"]# } remains a comment\n",
            "}\n",
            "VAR=\"formatted\"\n",
        );

        let formatted = format(input).unwrap();
        assert!(formatted.contains("    result=values[\"key\"]# } remains a comment\n"));
        assert!(formatted.ends_with("VAR = \"formatted\"\n"));
    }

    #[test]
    fn test_format_preserves_python_def_body() {
        let input = concat!(
            "def get_value(d):\n",
            "    result=d.getVar(\"VALUE\")\n",
            "    return result\n",
            "\n",
            "VAR=\"formatted\"\n",
        );
        let expected = concat!(
            "def get_value(d):\n",
            "    result=d.getVar(\"VALUE\")\n",
            "    return result\n",
            "\n",
            "VAR = \"formatted\"\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_preserves_continued_statement() {
        let input = concat!(
            "SRC_URI = \" \\\n",
            "    file://one.patch \\\n",
            "    file://two.patch \\\n",
            "\"\n",
            "VAR=\"formatted\"\n",
        );

        let formatted = format(input).unwrap();
        assert!(formatted.starts_with("SRC_URI = \" \\\n    file://one.patch \\\n"));
        assert!(formatted.ends_with("VAR = \"formatted\"\n"));
    }

    #[test]
    fn test_format_rejects_unclosed_function_body() {
        let input = "do_install() {\n    ./configure --prefix=/usr\n";
        let error = format(input).unwrap_err();

        assert_eq!(error.line(), 1);
        assert_eq!(error.message(), "function body has no closing brace");
    }

    #[test]
    fn test_format_rejects_unclosed_top_level_quote() {
        let error = format("VAR = \"unterminated\n").unwrap_err();

        assert_eq!(error.line(), 1);
        assert_eq!(
            error.message(),
            "top-level assignment contains an unclosed quote"
        );
    }

    #[test]
    fn test_format_is_idempotent() {
        let input = concat!(
            "VAR=\"value\"\n",
            "\n",
            "\n",
            "\n",
            "do_compile() {\n",
            "    oe_runmake OPTION=value\n",
            "}\n",
            "OTHER  ?=  \"default\"\n",
        );
        let once = format(input).unwrap();
        let twice = format(&once).unwrap();

        assert_eq!(once, twice);
    }
}

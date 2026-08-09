use crate::TextRange;

/// The class of a diagnostic produced while inspecting an embedded body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyDiagnosticKind {
    /// A shell control-flow construct is not properly paired.
    ShellSyntax,
    /// A Python delimiter, string, or compound statement is malformed.
    PythonSyntax,
    /// Python indentation mixes styles or does not follow the surrounding
    /// block structure.
    PythonIndentation,
}

/// A source-relative finding from shell or Python body analysis.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BodyDiagnostic {
    kind: BodyDiagnosticKind,
    range: TextRange,
    message: String,
}

impl BodyDiagnostic {
    fn new(kind: BodyDiagnosticKind, range: TextRange, message: impl Into<String>) -> Self {
        Self {
            kind,
            range,
            message: message.into(),
        }
    }

    /// Returns the language-specific category of this finding.
    pub const fn kind(&self) -> BodyDiagnosticKind {
        self.kind
    }

    /// Returns the half-open range relative to the supplied body text.
    pub const fn range(&self) -> TextRange {
        self.range
    }

    /// Returns the human-readable explanation of the finding.
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellBlockKind {
    If,
    Loop,
    Case,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShellBlock {
    kind: ShellBlockKind,
    offset: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShellToken<'a> {
    text: &'a str,
    range: TextRange,
    command_position: bool,
}

/// Performs conservative syntax analysis on a shell function body.
///
/// The scanner intentionally understands only shell lexical boundaries and
/// reserved control-flow words. It never expands variables, executes
/// commands, or rewrites the body, which keeps it safe for BitBake metadata
/// containing build-environment expressions.
pub fn analyze_shell_body(source: &str) -> Vec<BodyDiagnostic> {
    let tokens = shell_tokens(source);
    let mut diagnostics = Vec::new();
    let mut blocks = Vec::new();

    for token in tokens {
        if !token.command_position {
            continue;
        }
        match token.text {
            "if" => blocks.push(ShellBlock {
                kind: ShellBlockKind::If,
                offset: token.range.start(),
            }),
            "for" | "while" | "until" | "select" => blocks.push(ShellBlock {
                kind: ShellBlockKind::Loop,
                offset: token.range.start(),
            }),
            "case" => blocks.push(ShellBlock {
                kind: ShellBlockKind::Case,
                offset: token.range.start(),
            }),
            "then" => {
                if blocks
                    .last()
                    .is_none_or(|block| block.kind != ShellBlockKind::If)
                {
                    diagnostics.push(BodyDiagnostic::new(
                        BodyDiagnosticKind::ShellSyntax,
                        token.range,
                        "shell 'then' has no matching 'if'",
                    ));
                }
            }
            "elif" | "else" => {
                if blocks
                    .last()
                    .is_none_or(|block| block.kind != ShellBlockKind::If)
                {
                    diagnostics.push(BodyDiagnostic::new(
                        BodyDiagnosticKind::ShellSyntax,
                        token.range,
                        format!("shell '{}' has no matching 'if'", token.text),
                    ));
                }
            }
            "fi" => close_shell_block(
                &mut blocks,
                ShellBlockKind::If,
                token,
                "fi",
                &mut diagnostics,
            ),
            "do" => {
                if blocks
                    .last()
                    .is_none_or(|block| block.kind != ShellBlockKind::Loop)
                {
                    diagnostics.push(BodyDiagnostic::new(
                        BodyDiagnosticKind::ShellSyntax,
                        token.range,
                        "shell 'do' has no matching loop",
                    ));
                }
            }
            "done" => close_shell_block(
                &mut blocks,
                ShellBlockKind::Loop,
                token,
                "done",
                &mut diagnostics,
            ),
            "esac" => close_shell_block(
                &mut blocks,
                ShellBlockKind::Case,
                token,
                "esac",
                &mut diagnostics,
            ),
            _ => {}
        }
    }

    for block in blocks.into_iter().rev() {
        let closing = match block.kind {
            ShellBlockKind::If => "fi",
            ShellBlockKind::Loop => "done",
            ShellBlockKind::Case => "esac",
        };
        diagnostics.push(BodyDiagnostic::new(
            BodyDiagnosticKind::ShellSyntax,
            TextRange::new(block.offset, block.offset + 1),
            format!("shell block is missing closing '{closing}'"),
        ));
    }
    diagnostics
}

fn close_shell_block(
    blocks: &mut Vec<ShellBlock>,
    expected: ShellBlockKind,
    token: ShellToken<'_>,
    closing: &str,
    diagnostics: &mut Vec<BodyDiagnostic>,
) {
    if blocks.last().is_some_and(|block| block.kind == expected) {
        blocks.pop();
        return;
    }
    diagnostics.push(BodyDiagnostic::new(
        BodyDiagnosticKind::ShellSyntax,
        token.range,
        format!("shell '{closing}' does not match the open control-flow block"),
    ));
}

fn shell_tokens(source: &str) -> Vec<ShellToken<'_>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut word_start = None;
    let mut command_position = true;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut pending_here_docs = Vec::<(Vec<u8>, bool)>::new();
    let mut active_here_docs = Vec::<(Vec<u8>, bool)>::new();
    let mut line_start = 0;

    while index < bytes.len() {
        if index == line_start && !active_here_docs.is_empty() {
            let line_end = bytes[index..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |relative| index + relative);
            let mut line = &bytes[index..line_end];
            if line.ends_with(b"\r") {
                line = &line[..line.len() - 1];
            }
            let matches = active_here_docs.iter().any(|(delimiter, strip_tabs)| {
                let candidate = if *strip_tabs {
                    let mut start = 0;
                    while line.get(start) == Some(&b'\t') {
                        start += 1;
                    }
                    &line[start..]
                } else {
                    line
                };
                candidate == delimiter.as_slice()
            });
            if matches {
                active_here_docs.remove(0);
            }
            index = if line_end < bytes.len() {
                line_end + 1
            } else {
                line_end
            };
            line_start = index;
            command_position = true;
            continue;
        }

        let byte = bytes[index];
        if comment {
            if byte == b'\n' {
                comment = false;
                command_position = true;
                line_start = index + 1;
                index += 1;
            } else {
                index = next_char_boundary(source, index);
            }
            continue;
        }
        if escaped {
            escaped = false;
            index = next_char_boundary(source, index);
            continue;
        }
        if byte == b'\\' && quote != Some(b'\'') {
            escaped = true;
            index += 1;
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
                index += 1;
            } else {
                index = next_char_boundary(source, index);
            }
            continue;
        }

        if let Some(end) = bitbake_expression_end(bytes, index) {
            index = end;
            continue;
        }

        if byte == b'\n' {
            emit_shell_word(
                source,
                &mut word_start,
                index,
                command_position,
                &mut tokens,
            );
            command_position = true;
            if !pending_here_docs.is_empty() {
                active_here_docs.append(&mut pending_here_docs);
            }
            line_start = index + 1;
            index += 1;
            continue;
        }
        if matches!(byte, b' ' | b'\t' | b'\r') {
            if emit_shell_word(
                source,
                &mut word_start,
                index,
                command_position,
                &mut tokens,
            ) {
                command_position = false;
            }
            index += 1;
            continue;
        }
        if byte == b'#'
            && (index == line_start
                || bytes[index - 1].is_ascii_whitespace()
                || matches!(bytes[index - 1], b';' | b'|' | b'&' | b'(' | b')'))
        {
            if emit_shell_word(
                source,
                &mut word_start,
                index,
                command_position,
                &mut tokens,
            ) {
                command_position = false;
            }
            comment = true;
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            if word_start.is_none() {
                word_start = Some(index);
            }
            quote = Some(byte);
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(b"<<") && !bytes[index..].starts_with(b"<<<") {
            if let Some(delimiter) = here_document_delimiter(bytes, index + 2) {
                pending_here_docs.push(delimiter);
            }
            index += 2;
            continue;
        }
        if matches!(byte, b';' | b'|' | b'&' | b'(' | b')') {
            emit_shell_word(
                source,
                &mut word_start,
                index,
                command_position,
                &mut tokens,
            );
            command_position = true;
            index += 1;
            continue;
        }
        if word_start.is_none() {
            word_start = Some(index);
        }
        index = next_char_boundary(source, index);
    }
    emit_shell_word(
        source,
        &mut word_start,
        bytes.len(),
        command_position,
        &mut tokens,
    );
    tokens
}

fn bitbake_expression_end(bytes: &[u8], start: usize) -> Option<usize> {
    if !bytes.get(start..)?.starts_with(b"${") {
        return None;
    }
    let mut depth = 1usize;
    let mut quote = None;
    let mut escaped = false;
    let mut index = start + 2;
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
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(index + 1);
            }
        }
        index += 1;
    }
    None
}

fn next_char_boundary(source: &str, index: usize) -> usize {
    source[index..]
        .chars()
        .next()
        .map_or(source.len(), |character| index + character.len_utf8())
}

fn emit_shell_word<'a>(
    source: &'a str,
    word_start: &mut Option<usize>,
    end: usize,
    command_position: bool,
    tokens: &mut Vec<ShellToken<'a>>,
) -> bool {
    let Some(start) = word_start.take() else {
        return false;
    };
    let text = &source[start..end];
    tokens.push(ShellToken {
        text,
        range: TextRange::new(start, end),
        command_position,
    });
    true
}

fn here_document_delimiter(bytes: &[u8], mut index: usize) -> Option<(Vec<u8>, bool)> {
    let strip_tabs = bytes.get(index) == Some(&b'-');
    if strip_tabs {
        index += 1;
    }
    while matches!(bytes.get(index), Some(b' ' | b'\t')) {
        index += 1;
    }
    let quote = matches!(bytes.get(index), Some(b'\'' | b'"')).then(|| bytes[index]);
    if let Some(quote) = quote {
        index += 1;
        let start = index;
        while bytes.get(index) != Some(&quote) {
            index += 1;
            bytes.get(index)?;
        }
        return Some((bytes[start..index].to_vec(), strip_tabs));
    }
    let start = index;
    while let Some(byte) = bytes.get(index) {
        if byte.is_ascii_whitespace() || matches!(byte, b';' | b'|' | b'&' | b'<' | b'>') {
            break;
        }
        index += 1;
    }
    (start < index).then(|| (bytes[start..index].to_vec(), strip_tabs))
}

/// Performs conservative syntax and indentation analysis on a Python body.
pub fn analyze_python_body(source: &str) -> Vec<BodyDiagnostic> {
    let mut diagnostics = analyze_python_tokens(source);
    diagnostics.extend(analyze_python_indentation(source));
    diagnostics.extend(analyze_python_compound_statements(source));
    diagnostics
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PythonQuote {
    Single,
    Double,
    TripleSingle,
    TripleDouble,
}

fn analyze_python_tokens(source: &str) -> Vec<BodyDiagnostic> {
    let bytes = source.as_bytes();
    let mut diagnostics = Vec::new();
    let mut delimiters = Vec::<(u8, usize)>::new();
    let mut quote = None;
    let mut quote_start = 0;
    let mut escaped = false;
    let mut index = 0;
    let mut comment = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'\n' {
                comment = false;
                index += 1;
            } else {
                index = next_char_boundary(source, index);
            }
            continue;
        }
        if let Some(active) = quote {
            if escaped {
                escaped = false;
                index = next_char_boundary(source, index);
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                index += 1;
                continue;
            }
            let closing = match active {
                PythonQuote::Single => byte == b'\'',
                PythonQuote::Double => byte == b'"',
                PythonQuote::TripleSingle => bytes[index..].starts_with(b"'''"),
                PythonQuote::TripleDouble => bytes[index..].starts_with(b"\"\"\""),
            };
            if closing {
                match active {
                    PythonQuote::TripleSingle | PythonQuote::TripleDouble => index += 3,
                    PythonQuote::Single | PythonQuote::Double => index += 1,
                }
                quote = None;
            } else {
                index = next_char_boundary(source, index);
            }
            continue;
        }
        if byte == b'#' {
            comment = true;
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            let triple = bytes[index..].starts_with(&[byte, byte, byte]);
            quote_start = index;
            quote = Some(match (byte, triple) {
                (b'\'', false) => PythonQuote::Single,
                (b'"', false) => PythonQuote::Double,
                (b'\'', true) => PythonQuote::TripleSingle,
                (b'"', true) => PythonQuote::TripleDouble,
                _ => unreachable!(),
            });
            index += if triple { 3 } else { 1 };
            continue;
        }
        if matches!(byte, b'(' | b'[' | b'{') {
            delimiters.push((byte, index));
        } else if matches!(byte, b')' | b']' | b'}') {
            let expected = match byte {
                b')' => b'(',
                b']' => b'[',
                b'}' => b'{',
                _ => unreachable!(),
            };
            if delimiters.last().is_some_and(|(open, _)| *open == expected) {
                delimiters.pop();
            } else {
                diagnostics.push(BodyDiagnostic::new(
                    BodyDiagnosticKind::PythonSyntax,
                    TextRange::new(index, index + 1),
                    format!(
                        "Python closing delimiter '{}' has no matching opener",
                        byte as char
                    ),
                ));
            }
        }
        index = next_char_boundary(source, index);
    }

    if let Some(active) = quote {
        let delimiter = match active {
            PythonQuote::Single => "'",
            PythonQuote::Double => "\"",
            PythonQuote::TripleSingle => "'''",
            PythonQuote::TripleDouble => "\"\"\"",
        };
        diagnostics.push(BodyDiagnostic::new(
            BodyDiagnosticKind::PythonSyntax,
            TextRange::new(
                quote_start,
                (quote_start + delimiter.len()).min(source.len()),
            ),
            format!("Python string is missing closing delimiter {delimiter}"),
        ));
    }
    for (open, offset) in delimiters.into_iter().rev() {
        let closing = match open {
            b'(' => ')',
            b'[' => ']',
            b'{' => '}',
            _ => unreachable!(),
        };
        diagnostics.push(BodyDiagnostic::new(
            BodyDiagnosticKind::PythonSyntax,
            TextRange::new(offset, offset + 1),
            format!(
                "Python delimiter '{}' is missing closing '{closing}'",
                open as char
            ),
        ));
    }
    diagnostics
}

fn analyze_python_indentation(source: &str) -> Vec<BodyDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut levels = Vec::new();
    let mut base = None;
    let mut previous_opens_block = false;
    let mut line_continuation = false;
    let mut offset = 0;
    let mut triple_quote = None;
    let delimiter_depths = python_delimiter_depths(source);

    for (line_index, line) in source.split_inclusive('\n').enumerate() {
        let content = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .strip_suffix('\r')
            .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line));
        let in_triple_quote = python_line_triple_state(content, &mut triple_quote);
        let prefix_len = content.len() - content.trim_start_matches([' ', '\t']).len();
        let prefix = &content[..prefix_len];
        let code = strip_python_comment(&content[prefix_len..]).trim();
        if in_triple_quote {
            offset += line.len();
            continue;
        }
        if line_continuation {
            previous_opens_block = code.ends_with(':')
                && !code.ends_with("::")
                && delimiter_depths
                    .get(line_index + 1)
                    .copied()
                    .unwrap_or_default()
                    == 0;
            line_continuation = code.ends_with('\\');
            offset += line.len();
            continue;
        }
        if code.is_empty() {
            offset += line.len();
            continue;
        }
        if prefix.contains(' ') && prefix.contains('\t') {
            diagnostics.push(BodyDiagnostic::new(
                BodyDiagnosticKind::PythonIndentation,
                TextRange::new(offset, offset + prefix_len),
                "Python indentation mixes tabs and spaces",
            ));
        }
        if delimiter_depths
            .get(line_index)
            .copied()
            .unwrap_or_default()
            > 0
        {
            previous_opens_block = code.ends_with(':')
                && !code.ends_with("::")
                && delimiter_depths
                    .get(line_index + 1)
                    .copied()
                    .unwrap_or_default()
                    == 0;
            line_continuation = code.ends_with('\\');
            offset += line.len();
            continue;
        }
        let indent = python_indent_width(prefix);
        let base_indent = *base.get_or_insert(indent);
        if levels.is_empty() {
            levels.push(base_indent);
        }
        if indent > *levels.last().unwrap() {
            if !previous_opens_block {
                diagnostics.push(BodyDiagnostic::new(
                    BodyDiagnosticKind::PythonIndentation,
                    TextRange::new(offset, offset + prefix_len),
                    "unexpected Python indentation",
                ));
            }
            levels.push(indent);
        } else if indent < *levels.last().unwrap() {
            while levels.len() > 1 && indent < *levels.last().unwrap() {
                levels.pop();
            }
            if indent != *levels.last().unwrap() {
                diagnostics.push(BodyDiagnostic::new(
                    BodyDiagnosticKind::PythonIndentation,
                    TextRange::new(offset, offset + prefix_len),
                    "Python indentation does not match an enclosing block",
                ));
            }
        }
        previous_opens_block = code.ends_with(':')
            && !code.ends_with("::")
            && delimiter_depths
                .get(line_index + 1)
                .copied()
                .unwrap_or_default()
                == 0;
        line_continuation = code.ends_with('\\');
        offset += line.len();
    }
    diagnostics
}

fn python_line_triple_state(line: &str, state: &mut Option<[u8; 3]>) -> bool {
    let was_active = state.is_some();
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = *state {
            if bytes[index..].starts_with(&delimiter) {
                *state = None;
                index += 3;
            } else {
                index = next_char_boundary(line, index);
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                quote = None;
            }
            index = next_char_boundary(line, index);
            continue;
        }
        if byte == b'#' {
            break;
        }
        if matches!(byte, b'\'' | b'"') {
            if bytes[index..].starts_with(&[byte, byte, byte]) {
                *state = Some([byte, byte, byte]);
                index += 3;
            } else {
                quote = Some(byte);
                index += 1;
            }
            continue;
        }
        index = next_char_boundary(line, index);
    }
    was_active
}

fn python_indent_width(prefix: &str) -> usize {
    prefix.bytes().fold(0, |width, byte| match byte {
        b'\t' => (width / 8 + 1) * 8,
        _ => width + 1,
    })
}

fn analyze_python_compound_statements(source: &str) -> Vec<BodyDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut offset = 0;
    let mut triple_quote = None;
    let delimiter_depths = python_delimiter_depths(source);
    for (line_index, line) in source.split_inclusive('\n').enumerate() {
        let content = line
            .strip_suffix('\n')
            .unwrap_or(line)
            .strip_suffix('\r')
            .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line));
        let was_in_triple_quote = python_line_triple_state(content, &mut triple_quote);
        if was_in_triple_quote || triple_quote.is_some() {
            offset += line.len();
            continue;
        }
        let leading = content.len() - content.trim_start_matches([' ', '\t']).len();
        let code_without_comment = strip_python_comment(&content[leading..]);
        let code_leading = code_without_comment.len() - code_without_comment.trim_start().len();
        let code = code_without_comment.trim();
        let code_start = offset + leading + code_leading;
        if code.is_empty()
            || code.ends_with('\\')
            || delimiter_depths
                .get(line_index)
                .copied()
                .unwrap_or_default()
                > 0
            || delimiter_depths
                .get(line_index + 1)
                .copied()
                .unwrap_or_default()
                > 0
            || !is_python_compound_start(code)
        {
            offset += line.len();
            continue;
        }
        if !has_python_top_level_colon(code) {
            diagnostics.push(BodyDiagnostic::new(
                BodyDiagnosticKind::PythonSyntax,
                TextRange::new(code_start, code_start + code.len()),
                "Python compound statement is missing ':'",
            ));
        }
        offset += line.len();
    }
    diagnostics
}

fn python_delimiter_depths(source: &str) -> Vec<usize> {
    let mut depths = Vec::new();
    let mut depth: usize = 0;
    let mut triple_quote: Option<[u8; 3]> = None;
    let mut quote: Option<u8> = None;
    let mut escaped = false;

    for line in source.split_inclusive('\n') {
        depths.push(depth);
        let bytes = line.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(delimiter) = triple_quote {
                if bytes[index..].starts_with(&delimiter) {
                    triple_quote = None;
                    index += 3;
                } else {
                    index = next_char_boundary(line, index);
                }
                continue;
            }
            if let Some(delimiter) = quote {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' && delimiter != b'\'' {
                    escaped = true;
                } else if byte == delimiter {
                    quote = None;
                }
                index = next_char_boundary(line, index);
                continue;
            }
            if byte == b'#' {
                break;
            }
            if matches!(byte, b'\'' | b'"') {
                if bytes[index..].starts_with(&[byte, byte, byte]) {
                    triple_quote = Some([byte, byte, byte]);
                    index += 3;
                } else {
                    quote = Some(byte);
                    index += 1;
                }
                continue;
            }
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            index = next_char_boundary(line, index);
        }
    }
    depths
}

fn strip_python_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && quote != Some(b'\'') {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'#' {
            return &line[..index];
        }
    }
    line
}

fn is_python_compound_start(code: &str) -> bool {
    [
        "if ",
        "elif ",
        "else",
        "for ",
        "while ",
        "try",
        "except",
        "finally",
        "with ",
        "def ",
        "class ",
        "async def ",
        "async for ",
        "async with ",
        "match ",
        "case ",
    ]
    .iter()
    .any(|prefix| {
        if let Some(keyword) = prefix.strip_suffix(' ') {
            code == keyword || code.starts_with(prefix)
        } else {
            code == *prefix || code.starts_with(&format!("{prefix} "))
        }
    }) && !["match ", "case "].iter().any(|prefix| {
        code.strip_prefix(prefix).is_some_and(|rest| {
            rest.trim_start()
                .starts_with(['=', '+', '-', '*', '/', '%', '&', '|', '^'])
        })
    })
}

fn has_python_top_level_colon(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0usize;
    for byte in bytes.iter().copied() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && quote != Some(b'\'') {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if matches!(byte, b'(' | b'[' | b'{') {
            depth += 1;
        } else if matches!(byte, b')' | b']' | b'}') {
            depth = depth.saturating_sub(1);
        } else if byte == b':' && depth == 0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_analysis_matches_control_flow_and_ignores_quoted_text() {
        let valid = "if [ -f \"${D}/value\" ]; then\n    echo \"if\"\nelse\n    for item in one two; do\n        echo \"$item\"\ndone\nfi\n";
        assert!(analyze_shell_body(valid).is_empty());

        let invalid = "if true; then\n    echo ok\n";
        let diagnostics = analyze_shell_body(invalid);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message().contains("missing closing 'fi'"));
    }

    #[test]
    fn shell_analysis_handles_tab_stripped_here_documents() {
        let source = "if true; then\n\tcat <<- EOF\n\tif this is text; then\n\t\techo text\n\tfi\n\tEOF\nfi\n";
        assert!(analyze_shell_body(source).is_empty());
    }

    #[test]
    fn shell_analysis_ignores_bitbake_python_expansions() {
        let source = "${@' '.join(['%s=%s' % (key, value) for key in keys for value in values])}\nfor name in one; do\n\techo $name\ndone\n";
        assert!(analyze_shell_body(source).is_empty());
    }

    #[test]
    fn python_analysis_reports_delimiters_compound_colons_and_indentation() {
        let valid = "    if value:\n        return {\"value\": value}\n\t    \n    values = (\n        one\n        two\n    )\n    continued = one + \\\n        two\n    try_value = 1\n    elsewhere = 2\n";
        assert!(analyze_python_body(valid).is_empty());

        let invalid = "    if value\n\treturn (value\n";
        let diagnostics = analyze_python_body(invalid);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind() == BodyDiagnosticKind::PythonSyntax)
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.kind() == BodyDiagnosticKind::PythonIndentation })
        );
    }

    #[test]
    fn python_analysis_ignores_shell_inside_triple_quoted_strings() {
        let source = "script = f'''\nif [ -f \"$file\" ]; then\n    for item in one two; do\n        echo $item\n    done\nfi\n'''\nmessage = \"embedded \\\"quote\\\"\"\nvalue = 1\n";
        assert!(analyze_python_body(source).is_empty());

        let keyword_named_variables = "match = value\ncase = other\ncase += more\n";
        assert!(analyze_python_body(keyword_named_variables).is_empty());
    }

    #[test]
    fn body_analysis_keeps_ranges_on_unicode_boundaries() {
        let source = "echo �é\nif true; then echo ☃; fi\n";
        for diagnostic in analyze_shell_body(source)
            .into_iter()
            .chain(analyze_python_body(source))
        {
            assert!(source.is_char_boundary(diagnostic.range().start()));
            assert!(source.is_char_boundary(diagnostic.range().end()));
        }
    }
}

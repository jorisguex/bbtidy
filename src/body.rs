use crate::{
    TextRange, bitbake_expression_end, next_char_boundary, skip_shell_command_substitution,
};

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
enum ShellBlockPhase {
    IfCondition,
    IfBody,
    IfElse,
    LoopHeader,
    LoopBody,
    CaseHeader,
    CasePatterns,
    CaseCommands,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShellBlock {
    kind: ShellBlockKind,
    offset: usize,
    phase: ShellBlockPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellBoundary {
    None,
    CasePatternEnd,
    CaseArmEnd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShellToken<'a> {
    text: &'a str,
    range: TextRange,
    command_position: bool,
    boundary: ShellBoundary,
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
    let mut blocks: Vec<ShellBlock> = Vec::new();
    let mut uncertain = false;

    for token in tokens {
        if uncertain {
            continue;
        }
        if let Some(block) = blocks.last_mut()
            && block.kind == ShellBlockKind::Case
        {
            match token.boundary {
                ShellBoundary::CasePatternEnd if block.phase == ShellBlockPhase::CasePatterns => {
                    block.phase = ShellBlockPhase::CaseCommands;
                    continue;
                }
                ShellBoundary::CaseArmEnd if block.phase == ShellBlockPhase::CaseCommands => {
                    block.phase = ShellBlockPhase::CasePatterns;
                    continue;
                }
                _ => {}
            }
            if block.phase == ShellBlockPhase::CasePatterns {
                if token.text == "in" && !token.command_position {
                    continue;
                }
                if token.text != "esac" {
                    continue;
                }
            }
        }
        if !token.command_position {
            if token.text == "in"
                && blocks.last().is_some_and(|block| {
                    block.kind == ShellBlockKind::Case && block.phase == ShellBlockPhase::CaseHeader
                })
            {
                if let Some(block) = blocks.last_mut() {
                    block.phase = ShellBlockPhase::CasePatterns;
                }
            }
            continue;
        }
        match token.text {
            "if" => blocks.push(ShellBlock {
                kind: ShellBlockKind::If,
                offset: token.range.start(),
                phase: ShellBlockPhase::IfCondition,
            }),
            "for" | "while" | "until" | "select" => blocks.push(ShellBlock {
                kind: ShellBlockKind::Loop,
                offset: token.range.start(),
                phase: ShellBlockPhase::LoopHeader,
            }),
            "case" => blocks.push(ShellBlock {
                kind: ShellBlockKind::Case,
                offset: token.range.start(),
                phase: ShellBlockPhase::CaseHeader,
            }),
            "then" => {
                let valid = blocks.last().is_some_and(|block| {
                    block.kind == ShellBlockKind::If && block.phase == ShellBlockPhase::IfCondition
                });
                if valid {
                    blocks.last_mut().unwrap().phase = ShellBlockPhase::IfBody;
                } else {
                    push_shell_error(
                        &mut diagnostics,
                        token,
                        "shell 'then' is not valid in the current if phase",
                    );
                    uncertain = true;
                }
            }
            "elif" => {
                if blocks.last().is_some_and(|block| {
                    block.kind == ShellBlockKind::If && block.phase == ShellBlockPhase::IfBody
                }) {
                    blocks.last_mut().unwrap().phase = ShellBlockPhase::IfCondition;
                } else {
                    push_shell_error(
                        &mut diagnostics,
                        token,
                        "shell 'elif' is not valid in the current if phase",
                    );
                    uncertain = true;
                }
            }
            "else" => {
                if blocks.last().is_some_and(|block| {
                    block.kind == ShellBlockKind::If && block.phase == ShellBlockPhase::IfBody
                }) {
                    blocks.last_mut().unwrap().phase = ShellBlockPhase::IfElse;
                } else {
                    push_shell_error(
                        &mut diagnostics,
                        token,
                        "shell 'else' is not valid in the current if phase",
                    );
                    uncertain = true;
                }
            }
            "fi" => {
                let valid = blocks.last().is_some_and(|block| {
                    block.kind == ShellBlockKind::If
                        && matches!(
                            block.phase,
                            ShellBlockPhase::IfBody | ShellBlockPhase::IfElse
                        )
                });
                if valid {
                    blocks.pop();
                } else {
                    push_shell_error(
                        &mut diagnostics,
                        token,
                        "shell 'fi' does not match the open if block",
                    );
                    uncertain = true;
                }
            }
            "do" => {
                let valid = blocks.last().is_some_and(|block| {
                    block.kind == ShellBlockKind::Loop && block.phase == ShellBlockPhase::LoopHeader
                });
                if valid {
                    blocks.last_mut().unwrap().phase = ShellBlockPhase::LoopBody;
                } else {
                    push_shell_error(
                        &mut diagnostics,
                        token,
                        "shell 'do' is not valid in the current loop phase",
                    );
                    uncertain = true;
                }
            }
            "done" => {
                let valid = blocks.last().is_some_and(|block| {
                    block.kind == ShellBlockKind::Loop && block.phase == ShellBlockPhase::LoopBody
                });
                if valid {
                    blocks.pop();
                } else {
                    push_shell_error(
                        &mut diagnostics,
                        token,
                        "shell 'done' does not match the open loop block",
                    );
                    uncertain = true;
                }
            }
            "esac" => {
                let valid = blocks.last().is_some_and(|block| {
                    block.kind == ShellBlockKind::Case
                        && matches!(
                            block.phase,
                            ShellBlockPhase::CasePatterns | ShellBlockPhase::CaseCommands
                        )
                });
                if valid {
                    blocks.pop();
                } else {
                    push_shell_error(
                        &mut diagnostics,
                        token,
                        "shell 'esac' does not match the open case block",
                    );
                    uncertain = true;
                }
            }
            _ => {}
        }
    }

    if !uncertain {
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
    }
    sort_body_diagnostics(&mut diagnostics);
    diagnostics
}

fn push_shell_error(diagnostics: &mut Vec<BodyDiagnostic>, token: ShellToken<'_>, message: &str) {
    diagnostics.push(BodyDiagnostic::new(
        BodyDiagnosticKind::ShellSyntax,
        token.range,
        message,
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
        if index == line_start
            && word_start.is_none()
            && let Some(end) = skip_embedded_python_definition(source, index)
        {
            index = end;
            line_start = end;
            command_position = true;
            continue;
        }
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
            if delimiter == b'"' && bytes[index..].starts_with(b"$(") {
                if let Some(end) = skip_shell_command_substitution(bytes, index + 2) {
                    index = end;
                    continue;
                }
            }
            if delimiter == b'"' && byte == b'`' {
                if let Some(end) = skip_shell_backtick_substitution(bytes, index + 1) {
                    index = end;
                    continue;
                }
            }
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

        if bytes[index..].starts_with(b"$(")
            || bytes[index..].starts_with(b"<(")
            || bytes[index..].starts_with(b">(")
        {
            if word_start.is_none() {
                word_start = Some(index);
            }
            if let Some(end) = skip_shell_command_substitution(bytes, index + 2) {
                index = end;
                continue;
            }
        }
        if byte == b'`' {
            if word_start.is_none() {
                word_start = Some(index);
            }
            if let Some(end) = skip_shell_backtick_substitution(bytes, index + 1) {
                index = end;
                continue;
            }
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
        if byte == b';' && bytes.get(index + 1) == Some(&b';') {
            emit_shell_word(
                source,
                &mut word_start,
                index,
                command_position,
                &mut tokens,
            );
            tokens.push(ShellToken {
                text: &source[index..index + 2],
                range: TextRange::new(index, index + 2),
                command_position: true,
                boundary: ShellBoundary::CaseArmEnd,
            });
            command_position = true;
            index += 2;
            continue;
        }
        if byte == b')' {
            emit_shell_word(
                source,
                &mut word_start,
                index,
                command_position,
                &mut tokens,
            );
            tokens.push(ShellToken {
                text: &source[index..index + 1],
                range: TextRange::new(index, index + 1),
                command_position: true,
                boundary: ShellBoundary::CasePatternEnd,
            });
            command_position = true;
            index += 1;
            continue;
        }
        if matches!(byte, b';' | b'|' | b'&' | b'(') {
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

fn skip_embedded_python_definition(source: &str, start: usize) -> Option<usize> {
    let first_end = source[start..]
        .find('\n')
        .map(|relative| start + relative + 1)
        .unwrap_or(source.len());
    let first_line = &source[start..first_end];
    let (content, _) = first_line
        .strip_suffix("\r\n")
        .map_or((first_line, ""), |line| (line, "\r\n"));
    let leading = content.len() - content.trim_start_matches([' ', '\t']).len();
    let trimmed = content[leading..].trim();
    if leading == 0 || !trimmed.starts_with("def ") || !trimmed.contains(':') {
        return None;
    }

    let mut offset = first_end;
    let mut saw_body = false;
    let mut triple_quote = None;
    while offset < source.len() {
        let line_end = source[offset..]
            .find('\n')
            .map(|relative| offset + relative + 1)
            .unwrap_or(source.len());
        let line = &source[offset..line_end];
        let (line_content, _) = line
            .strip_suffix("\r\n")
            .map_or((line, ""), |value| (value, "\r\n"));
        let line_leading = line_content.len() - line_content.trim_start_matches([' ', '\t']).len();
        let line_trimmed = line_content[line_leading..].trim();
        if saw_body && triple_quote.is_none() && !line_trimmed.is_empty() && line_leading <= leading
        {
            break;
        }
        if !line_trimmed.is_empty() && line_leading > leading {
            saw_body = true;
        }
        python_line_triple_state(line_content, &mut triple_quote);
        offset = line_end;
    }
    saw_body.then_some(offset)
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
        boundary: ShellBoundary::None,
    });
    true
}

fn skip_shell_backtick_substitution(bytes: &[u8], mut index: usize) -> Option<usize> {
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'`' {
            return Some(index + 1);
        }
        index += 1;
    }
    None
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
    let lexical_error = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind() == BodyDiagnosticKind::PythonSyntax);
    if !lexical_error {
        diagnostics.extend(analyze_python_indentation(source));
        diagnostics.extend(analyze_python_compound_statements(source));
    }
    sort_body_diagnostics(&mut diagnostics);
    diagnostics
}

fn sort_body_diagnostics(diagnostics: &mut Vec<BodyDiagnostic>) {
    diagnostics.sort_by(|left, right| {
        (
            left.range().start(),
            left.range().end(),
            body_diagnostic_kind_order(left.kind()),
            left.message(),
        )
            .cmp(&(
                right.range().start(),
                right.range().end(),
                body_diagnostic_kind_order(right.kind()),
                right.message(),
            ))
    });
    diagnostics.dedup_by(|left, right| {
        left.kind() == right.kind()
            && left.range() == right.range()
            && left.message() == right.message()
    });
}

const fn body_diagnostic_kind_order(kind: BodyDiagnosticKind) -> u8 {
    match kind {
        BodyDiagnosticKind::ShellSyntax => 0,
        BodyDiagnosticKind::PythonSyntax => 1,
        BodyDiagnosticKind::PythonIndentation => 2,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PythonQuote {
    Single,
    Double,
    TripleSingle,
    TripleDouble,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PythonDelimiter {
    opening: u8,
    offset: usize,
}

fn analyze_python_tokens(source: &str) -> Vec<BodyDiagnostic> {
    let bytes = source.as_bytes();
    let mut diagnostics = Vec::new();
    let mut delimiters = Vec::<PythonDelimiter>::new();
    let mut delimiter_uncertain = false;
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
            if !delimiter_uncertain {
                delimiters.push(PythonDelimiter {
                    opening: byte,
                    offset: index,
                });
            }
        } else if matches!(byte, b')' | b']' | b'}') {
            let expected = match byte {
                b')' => b'(',
                b']' => b'[',
                b'}' => b'{',
                _ => unreachable!(),
            };
            if delimiter_uncertain {
                // Once a delimiter mismatch is observed, later delimiter
                // structure is not reliable enough to diagnose without a
                // cascade. Continue lexing strings/comments, but suppress
                // dependent delimiter findings below.
            } else if delimiters
                .last()
                .is_some_and(|delimiter| delimiter.opening == expected)
            {
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
                delimiter_uncertain = true;
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
    if delimiter_uncertain {
        return diagnostics;
    }
    for delimiter in delimiters.into_iter().rev() {
        let open = delimiter.opening;
        let offset = delimiter.offset;
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
    let mut previous_uncertain = false;
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
        if prefix.contains(' ') && prefix.contains('\t') {
            diagnostics.push(BodyDiagnostic::new(
                BodyDiagnosticKind::PythonIndentation,
                TextRange::new(offset, offset + prefix_len),
                "Python indentation mixes tabs and spaces",
            ));
        }
        let indent = python_indent_width(prefix);
        let base_indent = *base.get_or_insert(indent);
        if levels.is_empty() {
            levels.push(base_indent);
        }
        if indent > *levels.last().unwrap() {
            if !previous_opens_block && !previous_uncertain {
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
        previous_uncertain = is_python_compound_start(code) && !has_python_top_level_colon(code);
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
    let mut delimiters = Vec::<u8>::new();
    let mut uncertain = false;
    let mut triple_quote: Option<[u8; 3]> = None;
    let mut quote: Option<u8> = None;
    let mut escaped = false;

    for line in source.split_inclusive('\n') {
        depths.push(if uncertain {
            usize::MAX
        } else {
            delimiters.len()
        });
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
                b'(' | b'[' | b'{' if !uncertain => delimiters.push(byte),
                b')' | b']' | b'}' if !uncertain => {
                    let expected = match byte {
                        b')' => b'(',
                        b']' => b'[',
                        b'}' => b'{',
                        _ => unreachable!(),
                    };
                    if delimiters.last().copied() == Some(expected) {
                        delimiters.pop();
                    } else {
                        uncertain = true;
                    }
                }
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
    let mut stack = Vec::new();
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
            stack.push(byte);
        } else if matches!(byte, b')' | b']' | b'}') {
            let expected = match byte {
                b')' => b'(',
                b']' => b'[',
                b'}' => b'{',
                _ => unreachable!(),
            };
            if stack.last().copied() != Some(expected) {
                return false;
            }
            stack.pop();
        } else if byte == b':' && stack.is_empty() {
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
    fn shell_analysis_keeps_case_patterns_and_substitutions_opaque() {
        let source = concat!(
            "case \"$value\" in\n",
            "    if) echo pattern ;;\n",
            "    *) echo \"$(if true; then echo nested)\" ;;\n",
            "esac\n",
            "echo `if true; then echo nested`\n",
        );
        assert!(analyze_shell_body(source).is_empty());
    }

    #[test]
    fn shell_analysis_allows_control_flow_inside_case_commands() {
        let source = concat!(
            "case \"$value\" in\n",
            "    one) if true; then echo one; fi ;;\n",
            "    *) echo other ;;\n",
            "esac\n",
        );
        assert!(analyze_shell_body(source).is_empty());
    }

    #[test]
    fn shell_analysis_reports_only_the_primary_phase_error() {
        let source = "if true; then echo one; else echo two; else echo three; fi\n";
        let diagnostics = analyze_shell_body(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].range(), TextRange::new(39, 43));
    }

    #[test]
    fn shell_analysis_ignores_bitbake_python_expansions() {
        let source = "${@' '.join(['%s=%s' % (key, value) for key in keys for value in values])}\nfor name in one; do\n\techo $name\ndone\n";
        assert!(analyze_shell_body(source).is_empty());
    }

    #[test]
    fn shell_analysis_keeps_nested_quotes_in_command_substitutions_opaque() {
        let source = concat!(
            "if [ -d ${BAREBOX_ENV_DIR} ]; then\n",
            "    value=\"$(grep CONFIG .config | tr -d '\"')\"\n",
            "else\n",
            "    value=empty\n",
            "fi\n",
        );
        assert!(analyze_shell_body(source).is_empty());
    }

    #[test]
    fn shell_analysis_ignores_embedded_python_definitions() {
        let source = concat!(
            "    def helper(d):\n",
            "        if value:\n",
            "            return d\n",
            "    helper(d)\n",
        );
        assert!(analyze_shell_body(source).is_empty());
    }

    #[test]
    fn python_analysis_reports_delimiters_compound_colons_and_indentation() {
        let valid = "    if value:\n        return {\"value\": value}\n\t    \n    values = (\n        one\n        two\n    )\n    continued = one + \\\n        two\n    try_value = 1\n    elsewhere = 2\n";
        assert!(analyze_python_body(valid).is_empty());

        let invalid = "    if value\n\treturn value\n";
        let diagnostics = analyze_python_body(invalid);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind(), BodyDiagnosticKind::PythonSyntax);
        assert_eq!(diagnostics[0].range(), TextRange::new(4, 12));
    }

    #[test]
    fn python_analysis_suppresses_cascades_after_delimiter_mismatch() {
        let source = "    value = (one]\n    if value:\n        return value\n";
        let diagnostics = analyze_python_body(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind(), BodyDiagnosticKind::PythonSyntax);
        assert_eq!(diagnostics[0].range(), TextRange::new(16, 17));
    }

    #[test]
    fn python_diagnostics_are_sorted_by_exact_source_range() {
        let source = "    if value\n\t return value\n";
        let diagnostics = analyze_python_body(source);
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].range().start() < diagnostics[1].range().start());
        assert_eq!(diagnostics[0].range(), TextRange::new(4, 12));
        assert_eq!(diagnostics[1].range(), TextRange::new(13, 15));
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

    #[test]
    fn body_diagnostics_preserve_crlf_tabs_and_exact_ranges() {
        let shell = "if true; then\r\n\telse\r\n\telse\r\nfi\r\n";
        let second_else = shell.rfind("else").unwrap();
        let shell_diagnostics = analyze_shell_body(shell);
        assert_eq!(shell_diagnostics.len(), 1);
        assert_eq!(
            shell_diagnostics[0].range(),
            TextRange::new(second_else, second_else + "else".len())
        );

        let python = "    if value\r\n\treturn value\r\n";
        let python_diagnostics = analyze_python_body(python);
        assert_eq!(python_diagnostics.len(), 1);
        assert_eq!(
            python_diagnostics[0].kind(),
            BodyDiagnosticKind::PythonSyntax
        );
        assert_eq!(python_diagnostics[0].range(), TextRange::new(4, 12));
        assert!(python.is_char_boundary(python_diagnostics[0].range().end()));
    }
}

use logos::Logos;
use std::fmt;

mod body;
mod config;
mod formatter;
mod lint;
mod project;
mod semantic;
mod syntax;
mod workspace;

pub use body::{BodyDiagnostic, BodyDiagnosticKind, analyze_python_body, analyze_shell_body};
pub use config::{
    Config, ConfigError, SafetyOptions, SemanticConfig, discover_config, load_config,
};
pub use formatter::{FormatOptions, MetadataListLayout, format_syntax, format_syntax_with_options};
pub use lint::{
    ExternalLintDiagnostic, LintDiagnostic, LintFailurePolicy, LintFix, LintFixError, LintOptions,
    LintRule, LintSeverity, apply_lint_fixes, lint, lint_rules, lint_syntax,
    lint_syntax_with_options, lint_syntax_with_workspace, lint_with_bitbake, lint_with_options,
    lint_with_workspace, semantic_lint_diagnostics,
};
pub use project::{
    BuildContext, BuildContextDiscoveryOptions, BuildContextError, BuildContextSource,
    discover_build_context, discover_build_context_with_options,
};
pub use semantic::{
    SemanticDiagnostic, SemanticEnvironment, SemanticError, SemanticOptions, SemanticReport,
    SemanticSeverity, analyze_bitbake,
};
pub use syntax::{
    AssignmentSyntax, DirectiveKeyword, DirectiveSyntax, FunctionKind, FunctionSyntax,
    PythonDefinitionSyntax, SyntaxKind, SyntaxNode, SyntaxTree, TextRange, parse,
};
pub use workspace::{
    WorkspaceCandidate, WorkspaceClassContext, WorkspaceDependency, WorkspaceDependencyKind,
    WorkspaceFileDirective, WorkspaceIndex, WorkspaceSearchScope,
};

#[derive(Logos, Clone, Copy, Debug, Eq, PartialEq)]
pub enum Token {
    #[regex(r"[ \r\n\t\f]+")]
    Whitespace,

    // Comments start with # and go to end of line
    #[regex(r"#[^\r\n]*")]
    Comment,

    // Metadata sharing directives
    #[token("include")]
    IncludeKw,
    #[token("include_all")]
    IncludeAllKw,
    #[token("require")]
    RequireKw,
    #[token("inherit")]
    InheritKw,
    #[token("inherit_defer")]
    InheritDeferKw,
    #[token("addfragments")]
    AddFragmentsKw,

    // Variable and task directives
    #[token("export")]
    ExportKw,
    #[token("unset")]
    UnsetKw,
    #[token("addtask")]
    AddtaskKw,
    #[token("deltask")]
    DeltaskKw,
    #[token("addhandler")]
    AddHandlerKw,
    #[token("addpylib")]
    AddPyLibKw,
    #[token("EXPORT_FUNCTIONS")]
    ExportFunctionsKw,
    #[token("before")]
    BeforeKw,
    #[token("after")]
    AfterKw,

    // Function modifiers
    #[token("python")]
    PythonKw,
    #[token("fakeroot")]
    FakerootKw,

    // Assignment operators. The names reflect BitBake evaluation semantics.
    #[token("=")]
    Assign,
    #[token(":=")]
    ImmediateAssign,
    #[token("?=")]
    DefaultAssign,
    #[token("??=")]
    WeakDefaultAssign,
    #[token("+=")]
    AppendAssign,
    #[token("=+")]
    PrependAssign,
    #[token(".=")]
    AppendNoSpaceAssign,
    #[token("=.")]
    PrependNoSpaceAssign,

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
    #[token(":")]
    Colon,
    #[token("/")]
    Slash,
    #[token("\\")]
    LineContinuation,

    // Identifiers include variable, override, flag, directive argument, and
    // function-name components. Colons and variable references are kept as
    // separate tokens so any number of dynamic or literal overrides can be
    // represented without flattening their structure.
    #[regex(r"[A-Za-z0-9_][A-Za-z0-9_+.\-]*", priority = 2)]
    Ident,

    // Variable references like ${VAR} or ${@python}
    #[regex(r"\$\{[^}\r\n]*\}")]
    VarRef,

    // Quoted values can span physical lines through a backslash continuation.
    // Quotes are retained in the token slice.
    #[regex(r#"(?s:"([^"\\]|\\.)*")"#)]
    DqString,
    #[regex(r"(?s:'([^'\\]|\\.)*')")]
    SqString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentOperator {
    Assign,
    Immediate,
    Default,
    WeakDefault,
    AppendWithSpace,
    PrependWithSpace,
    AppendWithoutSpace,
    PrependWithoutSpace,
}

impl AssignmentOperator {
    const ALL_BY_LENGTH: [Self; 8] = [
        Self::WeakDefault,
        Self::Immediate,
        Self::Default,
        Self::AppendWithSpace,
        Self::PrependWithSpace,
        Self::AppendWithoutSpace,
        Self::PrependWithoutSpace,
        Self::Assign,
    ];

    pub const fn lexeme(self) -> &'static str {
        match self {
            Self::Assign => "=",
            Self::Immediate => ":=",
            Self::Default => "?=",
            Self::WeakDefault => "??=",
            Self::AppendWithSpace => "+=",
            Self::PrependWithSpace => "=+",
            Self::AppendWithoutSpace => ".=",
            Self::PrependWithoutSpace => "=.",
        }
    }
}

impl Token {
    pub const fn assignment_operator(self) -> Option<AssignmentOperator> {
        match self {
            Self::Assign => Some(AssignmentOperator::Assign),
            Self::ImmediateAssign => Some(AssignmentOperator::Immediate),
            Self::DefaultAssign => Some(AssignmentOperator::Default),
            Self::WeakDefaultAssign => Some(AssignmentOperator::WeakDefault),
            Self::AppendAssign => Some(AssignmentOperator::AppendWithSpace),
            Self::PrependAssign => Some(AssignmentOperator::PrependWithSpace),
            Self::AppendNoSpaceAssign => Some(AssignmentOperator::AppendWithoutSpace),
            Self::PrependNoSpaceAssign => Some(AssignmentOperator::PrependWithoutSpace),
            _ => None,
        }
    }
}

pub fn get_line_col(text: &str, index: usize) -> (usize, usize) {
    let prefix = &text[..index];
    let line = prefix.chars().filter(|&c| c == '\n').count() + 1;
    let last_newline = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = prefix[last_newline..].chars().count() + 1;
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

/// An error encountered while building a structurally complete syntax tree.
///
/// This alias preserves the formatter's original public error type while
/// giving parsing clients terminology appropriate to the CST API.
pub type SyntaxError = FormatError;

/// Formats the subset of BitBake syntax that can be changed safely.
///
/// Embedded shell and Python function bodies, Python `def` blocks, continuation
/// tails, comments, and unsupported top-level syntax are preserved
/// byte-for-byte. Structurally incomplete input produces an error instead of
/// partial output.
pub fn format(text: &str) -> Result<String, FormatError> {
    let tree = parse(text)?;
    Ok(format_syntax(&tree))
}

/// Formats source with caller-provided top-level formatting options.
pub fn format_with_options(text: &str, options: &FormatOptions) -> Result<String, FormatError> {
    let tree = parse(text)?;
    Ok(format_syntax_with_options(&tree, options))
}

fn next_line_end(text: &str, start: usize) -> usize {
    text[start..]
        .find('\n')
        .map(|relative| start + relative + 1)
        .unwrap_or(text.len())
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

#[derive(Clone, Copy)]
enum ScannerFunctionKind {
    Shell,
    Python,
}

#[derive(Clone, Copy)]
enum FunctionQuote {
    Single(u8),
    Triple(u8),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ShellHereDoc {
    delimiter: Vec<u8>,
    strip_tabs: bool,
}

fn function_opening_brace(line: &str) -> Option<(usize, ScannerFunctionKind)> {
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
    let mut kind = ScannerFunctionKind::Shell;
    let mut name = declaration;
    loop {
        if let Some(rest) = strip_keyword(name, "python") {
            kind = ScannerFunctionKind::Python;
            name = rest;
        } else if let Some(rest) = strip_keyword(name, "fakeroot") {
            name = rest;
        } else {
            break;
        }
    }

    if name.is_empty() && !matches!(kind, ScannerFunctionKind::Python) {
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
    function_kind: ScannerFunctionKind,
) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut quote: Option<FunctionQuote> = None;
    let mut escaped = false;
    let mut comment = false;
    let mut here_documents = Vec::new();
    let mut arithmetic_depth = 0usize;
    let mut index = opening_brace;

    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'\n' {
                comment = false;
                index += 1;
                if !here_documents.is_empty() {
                    index = skip_shell_here_documents(bytes, index, &mut here_documents)?;
                }
                continue;
            }
            index += 1;
            continue;
        }
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if byte == b'\\'
            && !matches!(
                (function_kind, quote),
                (
                    ScannerFunctionKind::Shell,
                    Some(FunctionQuote::Single(b'\''))
                )
            )
        {
            escaped = true;
            index += 1;
            continue;
        }
        if let Some(active_quote) = quote {
            if matches!(
                (function_kind, active_quote),
                (ScannerFunctionKind::Shell, FunctionQuote::Single(b'"'))
            ) && bytes[index..].starts_with(b"$(")
            {
                index = skip_shell_command_substitution(bytes, index + 2)?;
                continue;
            }
            match active_quote {
                FunctionQuote::Single(delimiter) if byte == delimiter => {
                    quote = None;
                }
                FunctionQuote::Triple(delimiter)
                    if bytes[index..].starts_with(&[delimiter, delimiter, delimiter]) =>
                {
                    quote = None;
                    index += 3;
                    continue;
                }
                _ => {}
            }
            index += 1;
            continue;
        }

        if matches!(function_kind, ScannerFunctionKind::Python)
            && matches!(byte, b'\'' | b'"')
            && bytes[index..].starts_with(&[byte, byte, byte])
        {
            quote = Some(FunctionQuote::Triple(byte));
            index += 3;
            continue;
        }

        if matches!(function_kind, ScannerFunctionKind::Shell) && bytes[index..].starts_with(b"((")
        {
            arithmetic_depth += 1;
            index += 2;
            continue;
        }

        if matches!(function_kind, ScannerFunctionKind::Shell) && bytes[index..].starts_with(b"$(")
        {
            index = skip_shell_command_substitution(bytes, index + 2)?;
            continue;
        }

        if matches!(function_kind, ScannerFunctionKind::Shell)
            && arithmetic_depth > 0
            && bytes[index..].starts_with(b"))")
        {
            arithmetic_depth = arithmetic_depth.checked_sub(1)?;
            index += 2;
            continue;
        }

        if matches!(function_kind, ScannerFunctionKind::Shell)
            && arithmetic_depth == 0
            && bytes[index..].starts_with(b"<<")
            && let Some((here_document, end)) = parse_shell_here_document(bytes, index)
        {
            here_documents.push(here_document);
            index = end;
            continue;
        }

        match byte {
            b'\'' | b'"' => quote = Some(FunctionQuote::Single(byte)),
            b'#' if is_comment_start_in_function(bytes, index, function_kind) => comment = true,
            b'\n' if !here_documents.is_empty() => {
                index += 1;
                index = skip_shell_here_documents(bytes, index, &mut here_documents)?;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((next_line_end(text, index), index));
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

fn skip_shell_command_substitution(bytes: &[u8], mut index: usize) -> Option<usize> {
    let mut depth = 1_usize;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;

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
        if quote != Some(b'\'') && byte == b'\\' {
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

        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'#' if is_comment_start_in_function(bytes, index, ScannerFunctionKind::Shell) => {
                comment = true;
            }
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

fn parse_shell_here_document(bytes: &[u8], start: usize) -> Option<(ShellHereDoc, usize)> {
    if !bytes[start..].starts_with(b"<<") || bytes.get(start + 2) == Some(&b'<') {
        return None;
    }

    let mut index = start + 2;
    let strip_tabs = bytes.get(index) == Some(&b'-');
    if strip_tabs {
        index += 1;
    }

    while matches!(bytes.get(index), Some(b' ' | b'\t')) {
        index += 1;
    }

    let mut delimiter = Vec::new();
    match bytes.get(index).copied()? {
        b'\'' | b'"' => {
            let quote = bytes[index];
            index += 1;
            while index < bytes.len() && bytes[index] != quote {
                delimiter.push(bytes[index]);
                index += 1;
            }
            if bytes.get(index) != Some(&quote) {
                return None;
            }
            index += 1;
        }
        b'\\' => {
            index += 1;
            delimiter.push(bytes.get(index).copied()?);
            index += 1;
            while let Some(&byte) = bytes.get(index) {
                if byte.is_ascii_whitespace() || is_shell_operator(byte) {
                    break;
                }
                if byte == b'\\' {
                    index += 1;
                    delimiter.push(bytes.get(index).copied()?);
                } else {
                    delimiter.push(byte);
                }
                index += 1;
            }
        }
        _ => {
            while let Some(&byte) = bytes.get(index) {
                if byte.is_ascii_whitespace() || is_shell_operator(byte) {
                    break;
                }
                if byte == b'\\' {
                    index += 1;
                    delimiter.push(bytes.get(index).copied()?);
                } else {
                    delimiter.push(byte);
                }
                index += 1;
            }
        }
    }

    if delimiter.is_empty() {
        return None;
    }

    Some((
        ShellHereDoc {
            delimiter,
            strip_tabs,
        },
        index,
    ))
}

fn is_shell_operator(byte: u8) -> bool {
    matches!(byte, b';' | b'|' | b'&' | b'<' | b'>' | b'(' | b')')
}

fn skip_shell_here_documents(
    bytes: &[u8],
    mut offset: usize,
    here_documents: &mut Vec<ShellHereDoc>,
) -> Option<usize> {
    while !here_documents.is_empty() {
        let here_document = here_documents.remove(0);
        loop {
            let line_end = bytes[offset..]
                .iter()
                .position(|&byte| byte == b'\n')
                .map(|relative| offset + relative)
                .unwrap_or(bytes.len());
            let mut line = &bytes[offset..line_end];
            if line.ends_with(b"\r") {
                line = &line[..line.len() - 1];
            }
            if here_document.strip_tabs {
                while line.first() == Some(&b'\t') {
                    line = &line[1..];
                }
            }

            if line == here_document.delimiter.as_slice() {
                offset = if line_end < bytes.len() {
                    line_end + 1
                } else {
                    line_end
                };
                break;
            }

            if line_end == bytes.len() {
                return None;
            }
            offset = line_end + 1;
        }
    }

    Some(offset)
}

fn is_comment_start_in_function(
    bytes: &[u8],
    index: usize,
    function_kind: ScannerFunctionKind,
) -> bool {
    if matches!(function_kind, ScannerFunctionKind::Python) {
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

fn find_assignment_operator(content: &str) -> Option<(usize, AssignmentOperator)> {
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

        if let Some(operator) = AssignmentOperator::ALL_BY_LENGTH
            .iter()
            .find(|operator| bytes[index..].starts_with(operator.lexeme().as_bytes()))
        {
            return Some((index, *operator));
        }
        index += 1;
    }

    None
}

fn is_assignment_left_hand_side(left: &str) -> bool {
    let name = if let Some(rest) = left.strip_prefix("export") {
        if rest.starts_with(char::is_whitespace) {
            rest.trim_start()
        } else {
            left
        }
    } else {
        left
    };
    is_variable_name(name)
}

fn is_variable_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || matches!(first, b'_' | b'$')) {
        return false;
    }

    let mut index = 0;
    let mut component_has_content = false;

    while index < bytes.len() {
        match bytes[index] {
            byte if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.') => {
                component_has_content = true;
                index += 1;
            }
            b'$' => {
                let Some(reference_end) = variable_reference_end(bytes, index) else {
                    return false;
                };
                component_has_content = true;
                index = reference_end;
            }
            b':' if component_has_content => {
                component_has_content = false;
                index += 1;
            }
            b'[' if component_has_content => {
                return variable_flag_is_valid(&bytes[index..]);
            }
            _ => return false,
        }
    }

    component_has_content
}

fn variable_reference_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start..start + 2)? != b"${" {
        return None;
    }
    let relative_end = bytes[start + 2..].iter().position(|&byte| byte == b'}')?;
    let end = start + 2 + relative_end;
    let contents = &bytes[start + 2..end];
    if contents.is_empty() || contents.iter().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    Some(end + 1)
}

fn variable_flag_is_valid(flag: &[u8]) -> bool {
    if flag.len() < 3 || flag.last() != Some(&b']') {
        return false;
    }

    flag[1..flag.len() - 1]
        .iter()
        .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.'))
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

    fn lex_tokens(text: &str) -> Vec<(Token, String)> {
        let mut lexer = Token::lexer(text);
        let mut tokens = Vec::new();
        while let Some(result) = lexer.next() {
            let token = result.unwrap_or_else(|_| {
                panic!(
                    "unexpected lexer error at {:?}: {:?}",
                    lexer.span(),
                    lexer.slice()
                )
            });
            if token != Token::Whitespace {
                tokens.push((token, lexer.slice().to_owned()));
            }
        }
        tokens
    }

    #[test]
    fn test_get_line_col() {
        let text = "line1\nline2\nline3";
        assert_eq!(get_line_col(text, 0), (1, 1));
        assert_eq!(get_line_col(text, 5), (1, 6)); // newline char
        assert_eq!(get_line_col(text, 6), (2, 1));
        assert_eq!(get_line_col(text, 11), (2, 6)); // newline char
        assert_eq!(get_line_col(text, 12), (3, 1));

        let utf8 = "éx\nå β";
        assert_eq!(get_line_col(utf8, utf8.find('β').unwrap()), (2, 3));
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
        let tokens = lex_tokens(
            "include include_all require inherit inherit_defer addfragments \
             export unset addtask deltask addhandler addpylib EXPORT_FUNCTIONS \
             before after python fakeroot",
        );
        let actual: Vec<Token> = tokens.into_iter().map(|(token, _)| token).collect();
        let expected = vec![
            Token::IncludeKw,
            Token::IncludeAllKw,
            Token::RequireKw,
            Token::InheritKw,
            Token::InheritDeferKw,
            Token::AddFragmentsKw,
            Token::ExportKw,
            Token::UnsetKw,
            Token::AddtaskKw,
            Token::DeltaskKw,
            Token::AddHandlerKw,
            Token::AddPyLibKw,
            Token::ExportFunctionsKw,
            Token::BeforeKw,
            Token::AfterKw,
            Token::PythonKw,
            Token::FakerootKw,
        ];

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_assignment_operator_tokens_and_semantics() {
        let tokens = lex_tokens("= := ?= ??= += =+ .= =.");
        let actual: Vec<(Token, AssignmentOperator, String)> = tokens
            .into_iter()
            .map(|(token, slice)| {
                let operator = token.assignment_operator().unwrap();
                (token, operator, slice)
            })
            .collect();
        let expected = vec![
            (Token::Assign, AssignmentOperator::Assign, "=".into()),
            (
                Token::ImmediateAssign,
                AssignmentOperator::Immediate,
                ":=".into(),
            ),
            (
                Token::DefaultAssign,
                AssignmentOperator::Default,
                "?=".into(),
            ),
            (
                Token::WeakDefaultAssign,
                AssignmentOperator::WeakDefault,
                "??=".into(),
            ),
            (
                Token::AppendAssign,
                AssignmentOperator::AppendWithSpace,
                "+=".into(),
            ),
            (
                Token::PrependAssign,
                AssignmentOperator::PrependWithSpace,
                "=+".into(),
            ),
            (
                Token::AppendNoSpaceAssign,
                AssignmentOperator::AppendWithoutSpace,
                ".=".into(),
            ),
            (
                Token::PrependNoSpaceAssign,
                AssignmentOperator::PrependWithoutSpace,
                "=.".into(),
            ),
        ];

        assert_eq!(actual, expected);
        for (_, operator, slice) in actual {
            assert_eq!(operator.lexeme(), slice);
        }
    }

    #[test]
    fn test_override_key_expansion_and_varflag_tokens() {
        let tokens = lex_tokens(
            "RDEPENDS:${PN}:class-native += \"foo\"\n\
             do_fetch[network] = \"1\"\n\
             RDEPENDS_${PN} += \"legacy\"",
        );
        let actual: Vec<(Token, &str)> = tokens
            .iter()
            .map(|(token, slice)| (*token, slice.as_str()))
            .collect();
        let expected = vec![
            (Token::Ident, "RDEPENDS"),
            (Token::Colon, ":"),
            (Token::VarRef, "${PN}"),
            (Token::Colon, ":"),
            (Token::Ident, "class-native"),
            (Token::AppendAssign, "+="),
            (Token::DqString, "\"foo\""),
            (Token::Ident, "do_fetch"),
            (Token::LBracket, "["),
            (Token::Ident, "network"),
            (Token::RBracket, "]"),
            (Token::Assign, "="),
            (Token::DqString, "\"1\""),
            (Token::Ident, "RDEPENDS_"),
            (Token::VarRef, "${PN}"),
            (Token::AppendAssign, "+="),
            (Token::DqString, "\"legacy\""),
        ];

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_directive_paths_and_multiline_values_lex_without_errors() {
        let input = concat!(
            "include_all conf/distro/include/maintainers.inc\n",
            "inherit_defer ${VARNAME}\n",
            "addpylib ${LAYERDIR}/lib oeqa\n",
            "SRC_URI = \" \\\n",
            "    file://one.patch \\\n",
            "    file://two.patch \\\n",
            "\"\n",
        );
        let tokens = lex_tokens(input);

        assert!(tokens.contains(&(Token::IncludeAllKw, "include_all".into())));
        assert!(tokens.contains(&(Token::InheritDeferKw, "inherit_defer".into())));
        assert!(tokens.contains(&(Token::AddPyLibKw, "addpylib".into())));
        assert!(
            tokens
                .iter()
                .filter(|(token, _)| *token == Token::Slash)
                .count()
                >= 4
        );
        assert!(tokens.iter().any(|(token, slice)| {
            *token == Token::DqString
                && slice.contains("file://one.patch")
                && slice.contains("file://two.patch")
        }));
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
        assert_eq!(lex.next(), Some(Ok(Token::WeakDefaultAssign)));
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
    fn test_format_all_assignment_operators() {
        let input = concat!(
            "A=\"assign\"\n",
            "B:=\"immediate\"\n",
            "C?=\"default\"\n",
            "D??=\"weak default\"\n",
            "E+=\"append with space\"\n",
            "F=+\"prepend with space\"\n",
            "G.=\"append without space\"\n",
            "H=.\"prepend without space\"\n",
        );
        let expected = concat!(
            "A = \"assign\"\n",
            "B := \"immediate\"\n",
            "C ?= \"default\"\n",
            "D ??= \"weak default\"\n",
            "E += \"append with space\"\n",
            "F =+ \"prepend with space\"\n",
            "G .= \"append without space\"\n",
            "H =. \"prepend without space\"\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_modern_overrides_key_expansion_and_varflags() {
        let input = concat!(
            "RDEPENDS:${PN}:class-native+=\"package\"\n",
            "do_fetch[network]=\"1\"\n",
            "A${B}:machine:append.=\"suffix\"\n",
            "export PATH:prepend=\"${STAGING_BINDIR_NATIVE}:\"\n",
        );
        let expected = concat!(
            "RDEPENDS:${PN}:class-native += \"package\"\n",
            "do_fetch[network] = \"1\"\n",
            "A${B}:machine:append .= \"suffix\"\n",
            "export PATH:prepend = \"${STAGING_BINDIR_NATIVE}:\"\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_preserves_invalid_variable_shapes() {
        let input = "FOO[flag:override]=\"unchanged\"\nFOO::append=\"unchanged\"\n";

        assert_eq!(format(input).unwrap(), input);
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
    fn test_format_preserves_quotes_inside_shell_command_substitution() {
        let input = concat!(
            "do_compile() {\n",
            "    VALUE=\"$(printf '%s' '\"')\"\n",
            "}\n",
            "SUMMARY=\"Example\"\n",
        );
        let expected = concat!(
            "do_compile() {\n",
            "    VALUE=\"$(printf '%s' '\"')\"\n",
            "}\n",
            "SUMMARY = \"Example\"\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_preserves_backslash_at_end_of_shell_single_quote() {
        let input = concat!(
            "do_install() {\n",
            "    sed -e '$a\\' -e 'VALUE := example' ${D}/Makefile\n",
            "}\n",
            "SUMMARY=\"Example\"\n",
        );
        let expected = concat!(
            "do_install() {\n",
            "    sed -e '$a\\' -e 'VALUE := example' ${D}/Makefile\n",
            "}\n",
            "SUMMARY = \"Example\"\n",
        );

        assert_eq!(format(input).unwrap(), expected);
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
    fn test_format_preserves_triple_quoted_python_function_body() {
        let input = concat!(
            "python __anonymous() {\n",
            "    script = \"\"\"#!/bin/sh\n",
            "echo \"\n",
            "${VALUE}\n",
            "\"\"\"\n",
            "}\n",
            "SUMMARY=\"Example\"\n",
        );
        let expected = concat!(
            "python __anonymous() {\n",
            "    script = \"\"\"#!/bin/sh\n",
            "echo \"\n",
            "${VALUE}\n",
            "\"\"\"\n",
            "}\n",
            "SUMMARY = \"Example\"\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn test_format_preserves_combined_function_modifiers() {
        let input = concat!(
            "fakeroot python do_example() {\n",
            "    first = d.getVar('FIRST')\n",
            "\n",
            "\n",
            "    second = d.getVar('SECOND')\n",
            "}\n",
            "SUMMARY=\"Example\"\n",
        );
        let expected = concat!(
            "fakeroot python do_example() {\n",
            "    first = d.getVar('FIRST')\n",
            "\n",
            "\n",
            "    second = d.getVar('SECOND')\n",
            "}\n",
            "SUMMARY = \"Example\"\n",
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

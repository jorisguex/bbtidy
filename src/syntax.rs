use crate::{
    AssignmentOperator, FormatError, ScannerFunctionKind, SyntaxError, comment_start,
    find_assignment_operator, find_brace_block_end, find_continuation_end, find_python_def_end,
    function_opening_brace, has_balanced_quotes, has_line_continuation,
    is_assignment_left_hand_side, is_blank_line, is_python_def_start, next_line_end,
    split_line_ending,
};

/// A half-open byte range in the original source text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct TextRange {
    start: usize,
    end: usize,
}

impl TextRange {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn start(self) -> usize {
        self.start
    }

    pub const fn end(self) -> usize {
        self.end
    }

    pub const fn len(self) -> usize {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// A lossless, top-level concrete syntax tree for BitBake metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxTree<'a> {
    source: &'a str,
    nodes: Vec<SyntaxNode<'a>>,
}

impl<'a> SyntaxTree<'a> {
    pub fn source(&self) -> &'a str {
        self.source
    }

    pub fn nodes(&self) -> &[SyntaxNode<'a>] {
        &self.nodes
    }
}

/// A source-preserving top-level syntax node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxNode<'a> {
    kind: SyntaxKind<'a>,
    range: TextRange,
    text: &'a str,
}

impl<'a> SyntaxNode<'a> {
    pub fn kind(&self) -> &SyntaxKind<'a> {
        &self.kind
    }

    pub const fn range(&self) -> TextRange {
        self.range
    }

    pub fn text(&self) -> &'a str {
        self.text
    }
}

/// The recognized kind of a top-level syntax node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyntaxKind<'a> {
    Blank,
    Comment,
    Assignment(AssignmentSyntax<'a>),
    Directive(DirectiveSyntax<'a>),
    Function(FunctionSyntax<'a>),
    PythonDefinition(PythonDefinitionSyntax<'a>),
    Unknown,
}

/// Structured information retained for a top-level assignment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentSyntax<'a> {
    name: &'a str,
    name_range: TextRange,
    operator: AssignmentOperator,
    operator_range: TextRange,
    value: &'a str,
    value_range: TextRange,
    exported: bool,
    continued: bool,
}

impl<'a> AssignmentSyntax<'a> {
    pub fn name(&self) -> &'a str {
        self.name
    }

    pub const fn name_range(&self) -> TextRange {
        self.name_range
    }

    pub const fn operator(&self) -> AssignmentOperator {
        self.operator
    }

    pub const fn operator_range(&self) -> TextRange {
        self.operator_range
    }

    pub fn value(&self) -> &'a str {
        self.value
    }

    pub const fn value_range(&self) -> TextRange {
        self.value_range
    }

    pub const fn is_exported(&self) -> bool {
        self.exported
    }

    pub const fn is_continued(&self) -> bool {
        self.continued
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DirectiveKeyword {
    Include,
    IncludeAll,
    Require,
    Inherit,
    InheritDefer,
    AddFragments,
    AddPyLib,
    AddHandler,
    AddTask,
    DelTask,
    ExportFunctions,
    Export,
    Unset,
}

impl DirectiveKeyword {
    pub const fn lexeme(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::IncludeAll => "include_all",
            Self::Require => "require",
            Self::Inherit => "inherit",
            Self::InheritDefer => "inherit_defer",
            Self::AddFragments => "addfragments",
            Self::AddPyLib => "addpylib",
            Self::AddHandler => "addhandler",
            Self::AddTask => "addtask",
            Self::DelTask => "deltask",
            Self::ExportFunctions => "EXPORT_FUNCTIONS",
            Self::Export => "export",
            Self::Unset => "unset",
        }
    }
}

/// Structured information retained for a top-level directive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectiveSyntax<'a> {
    keyword: DirectiveKeyword,
    keyword_range: TextRange,
    arguments: &'a str,
    arguments_range: TextRange,
    continued: bool,
}

impl<'a> DirectiveSyntax<'a> {
    pub const fn keyword(&self) -> DirectiveKeyword {
        self.keyword
    }

    pub const fn keyword_range(&self) -> TextRange {
        self.keyword_range
    }

    pub fn arguments(&self) -> &'a str {
        self.arguments
    }

    pub const fn arguments_range(&self) -> TextRange {
        self.arguments_range
    }

    pub const fn is_continued(&self) -> bool {
        self.continued
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FunctionKind {
    Shell,
    Python,
}

/// The declaration and source range of a shell or Python function body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionSyntax<'a> {
    kind: FunctionKind,
    name: Option<&'a str>,
    name_range: Option<TextRange>,
    body_range: TextRange,
    fakeroot: bool,
}

impl<'a> FunctionSyntax<'a> {
    pub const fn function_kind(&self) -> FunctionKind {
        self.kind
    }

    pub fn name(&self) -> Option<&'a str> {
        self.name
    }

    pub const fn name_range(&self) -> Option<TextRange> {
        self.name_range
    }

    /// Returns the source range of the embedded function body, excluding the
    /// declaration's opening brace and closing brace.
    pub const fn body_range(&self) -> TextRange {
        self.body_range
    }

    pub const fn is_fakeroot(&self) -> bool {
        self.fakeroot
    }
}

/// The declaration and source range of an indented top-level Python `def` body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonDefinitionSyntax<'a> {
    name: &'a str,
    name_range: TextRange,
    body_range: TextRange,
}

impl<'a> PythonDefinitionSyntax<'a> {
    pub fn name(&self) -> &'a str {
        self.name
    }

    pub const fn name_range(&self) -> TextRange {
        self.name_range
    }

    /// Returns the source range occupied by the indented Python body.
    pub const fn body_range(&self) -> TextRange {
        self.body_range
    }
}

/// Parses BitBake metadata into a lossless top-level concrete syntax tree.
///
/// Every byte of `source` belongs to exactly one node. Unsupported syntax and
/// embedded function bodies are retained verbatim instead of being discarded.
pub fn parse(source: &str) -> Result<SyntaxTree<'_>, SyntaxError> {
    let mut nodes = Vec::new();
    let mut offset = 0;
    let mut line_number = 1;

    while offset < source.len() {
        let line_end = next_line_end(source, offset);
        let line = &source[offset..line_end];

        let (end, kind) = if let Some((opening_brace, scanner_kind)) = function_opening_brace(line)
        {
            let (end, closing_brace) =
                find_brace_block_end(source, offset + opening_brace, scanner_kind).ok_or_else(
                    || FormatError::new(line_number, "function body has no closing brace"),
                )?;
            (
                end,
                SyntaxKind::Function(parse_function(
                    source,
                    offset,
                    line,
                    scanner_kind,
                    TextRange::new(offset + opening_brace + 1, closing_brace),
                )),
            )
        } else if is_python_def_start(line) {
            let end = find_python_def_end(source, line_end);
            (
                end,
                SyntaxKind::PythonDefinition(parse_python_definition(
                    source,
                    offset,
                    line,
                    TextRange::new(line_end, end),
                )),
            )
        } else {
            let continued = has_line_continuation(line);
            let end = if continued {
                find_continuation_end(source, offset).ok_or_else(|| {
                    FormatError::new(
                        line_number,
                        "statement ends with an unterminated continuation",
                    )
                })?
            } else {
                line_end
            };
            let text = &source[offset..end];
            (
                end,
                classify_statement(source, offset, text, continued, line_number)?,
            )
        };

        let text = &source[offset..end];
        nodes.push(SyntaxNode {
            kind,
            range: TextRange::new(offset, end),
            text,
        });
        line_number += text.bytes().filter(|&byte| byte == b'\n').count();
        offset = end;
    }

    Ok(SyntaxTree { source, nodes })
}

fn classify_statement<'a>(
    source: &'a str,
    start: usize,
    text: &'a str,
    continued: bool,
    line_number: usize,
) -> Result<SyntaxKind<'a>, FormatError> {
    let first_line_end = next_line_end(text, 0);
    let first_line = &text[..first_line_end];
    if is_blank_line(first_line) {
        return Ok(SyntaxKind::Blank);
    }

    let (first_content, _) = split_line_ending(first_line);
    let leading = first_content.len() - first_content.trim_start_matches([' ', '\t']).len();
    let trimmed = &first_content[leading..];
    if trimmed.starts_with('#') {
        return Ok(SyntaxKind::Comment);
    }

    if !first_content.starts_with(char::is_whitespace)
        && let Some(assignment) = parse_assignment(source, start, text, continued, line_number)?
    {
        return Ok(SyntaxKind::Assignment(assignment));
    }

    if !first_content.starts_with(char::is_whitespace)
        && let Some(directive) = parse_directive(source, start, text, continued)
    {
        return Ok(SyntaxKind::Directive(directive));
    }

    Ok(SyntaxKind::Unknown)
}

fn parse_assignment<'a>(
    source: &'a str,
    start: usize,
    text: &'a str,
    continued: bool,
    line_number: usize,
) -> Result<Option<AssignmentSyntax<'a>>, FormatError> {
    let first_line_end = next_line_end(text, 0);
    let (first_content, _) = split_line_ending(&text[..first_line_end]);
    let Some((operator_start, operator)) = find_assignment_operator(first_content) else {
        return Ok(None);
    };
    let left = first_content[..operator_start].trim_end();
    if !is_assignment_left_hand_side(left) {
        return Ok(None);
    }

    let (name, name_offset, exported) = if let Some(rest) = left.strip_prefix("export") {
        if rest.starts_with(char::is_whitespace) {
            let whitespace = rest.len() - rest.trim_start().len();
            (rest.trim_start(), "export".len() + whitespace, true)
        } else {
            (left, 0, false)
        }
    } else {
        (left, 0, false)
    };
    let operator_end = operator_start + operator.lexeme().len();

    let content_end = text_without_final_line_ending(text).len();
    if !has_balanced_quotes(&text[operator_end..content_end]) {
        return Err(FormatError::new(
            line_number,
            "top-level assignment contains an unclosed quote",
        ));
    }

    let value_start = operator_end;
    Ok(Some(AssignmentSyntax {
        name: &source[start + name_offset..start + name_offset + name.len()],
        name_range: TextRange::new(start + name_offset, start + name_offset + name.len()),
        operator,
        operator_range: TextRange::new(start + operator_start, start + operator_end),
        value: &source[start + value_start..start + content_end],
        value_range: TextRange::new(start + value_start, start + content_end),
        exported,
        continued,
    }))
}

fn parse_directive<'a>(
    source: &'a str,
    start: usize,
    text: &'a str,
    continued: bool,
) -> Option<DirectiveSyntax<'a>> {
    let content = text_without_final_line_ending(text);
    let content_end = content.len();
    let leading = content.len() - content.trim_start_matches([' ', '\t']).len();
    let trimmed = &content[leading..];
    let keywords = [
        DirectiveKeyword::IncludeAll,
        DirectiveKeyword::InheritDefer,
        DirectiveKeyword::ExportFunctions,
        DirectiveKeyword::AddFragments,
        DirectiveKeyword::AddHandler,
        DirectiveKeyword::AddPyLib,
        DirectiveKeyword::Include,
        DirectiveKeyword::Require,
        DirectiveKeyword::Inherit,
        DirectiveKeyword::AddTask,
        DirectiveKeyword::DelTask,
        DirectiveKeyword::Export,
        DirectiveKeyword::Unset,
    ];

    for keyword in keywords {
        let lexeme = keyword.lexeme();
        let Some(rest) = trimmed.strip_prefix(lexeme) else {
            continue;
        };
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            continue;
        }

        let whitespace = rest.len() - rest.trim_start().len();
        let keyword_start = start + leading;
        let arguments_start = leading + lexeme.len() + whitespace;
        return Some(DirectiveSyntax {
            keyword,
            keyword_range: TextRange::new(keyword_start, keyword_start + lexeme.len()),
            arguments: &source[start + arguments_start..start + content_end],
            arguments_range: TextRange::new(start + arguments_start, start + content_end),
            continued,
        });
    }
    None
}

fn parse_function<'a>(
    source: &'a str,
    start: usize,
    line: &'a str,
    scanner_kind: ScannerFunctionKind,
    body_range: TextRange,
) -> FunctionSyntax<'a> {
    let (content, _) = split_line_ending(line);
    let code_end = comment_start(content).unwrap_or(content.len());
    let signature = content[..code_end]
        .trim_end()
        .strip_suffix('{')
        .expect("function scanner found an opening brace")
        .trim_end();
    let closing_paren = signature
        .strip_suffix(')')
        .expect("function scanner validated the signature");
    let opening_paren = closing_paren
        .rfind('(')
        .expect("function scanner validated the signature");
    let declaration = closing_paren[..opening_paren].trim();
    let fakeroot = declaration
        .split_ascii_whitespace()
        .any(|part| part == "fakeroot");
    let name = declaration
        .split_ascii_whitespace()
        .find(|part| !matches!(*part, "python" | "fakeroot"))
        .unwrap_or("");
    let name_range = if name.is_empty() {
        None
    } else {
        let relative = content[..opening_paren]
            .rfind(name)
            .expect("function name belongs to the declaration");
        Some(TextRange::new(
            start + relative,
            start + relative + name.len(),
        ))
    };

    FunctionSyntax {
        kind: match scanner_kind {
            ScannerFunctionKind::Shell => FunctionKind::Shell,
            ScannerFunctionKind::Python => FunctionKind::Python,
        },
        name: name_range.map(|range| &source[range.start()..range.end()]),
        name_range,
        body_range,
        fakeroot,
    }
}

fn parse_python_definition<'a>(
    source: &'a str,
    start: usize,
    line: &'a str,
    body_range: TextRange,
) -> PythonDefinitionSyntax<'a> {
    let (content, _) = split_line_ending(line);
    let name_start = "def ".len();
    let name_end = content[name_start..]
        .find('(')
        .map(|relative| name_start + relative)
        .unwrap_or_else(|| {
            content[name_start..]
                .find(':')
                .map(|relative| name_start + relative)
                .expect("Python definition scanner requires a colon")
        });
    let name = content[name_start..name_end].trim_end();
    let range = TextRange::new(start + name_start, start + name_start + name.len());
    PythonDefinitionSyntax {
        name: &source[range.start()..range.end()],
        name_range: range,
        body_range,
    }
}

fn text_without_final_line_ending(text: &str) -> &str {
    if let Some(content) = text.strip_suffix("\r\n") {
        content
    } else if let Some(content) = text.strip_suffix('\n') {
        content
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_is_lossless_and_ranges_are_contiguous() {
        let source = concat!(
            "# heading\r\n",
            "\r\n",
            "export FOO:append = \" value\"\n",
            "inherit base \\\n",
            "  extra\n",
            "python do_build() {\n",
            "    value = \"}\" # not the end\n",
            "}\n",
            "def helper(d):\n",
            "    return d.getVar(\"FOO\")\n",
            "opaque syntax\n",
        );
        let tree = parse(source).unwrap();

        let rebuilt: String = tree.nodes().iter().map(SyntaxNode::text).collect();
        assert_eq!(rebuilt, source);
        assert_eq!(tree.source(), source);

        let mut expected_start = 0;
        for node in tree.nodes() {
            assert_eq!(node.range().start(), expected_start);
            assert_eq!(
                node.text(),
                &source[node.range().start()..node.range().end()]
            );
            expected_start = node.range().end();
        }
        assert_eq!(expected_start, source.len());
    }

    #[test]
    fn exposes_structured_top_level_nodes() {
        let source = concat!(
            "export FOO:append = \" value\"\n",
            "inherit base \\\n",
            "  extra\n",
            "fakeroot python do_install() {\n",
            "}\n",
            "def helper(d):\n",
            "    pass\n",
        );
        let tree = parse(source).unwrap();

        let SyntaxKind::Assignment(assignment) = tree.nodes()[0].kind() else {
            panic!("expected assignment");
        };
        assert_eq!(assignment.name(), "FOO:append");
        assert_eq!(assignment.operator(), AssignmentOperator::Assign);
        assert_eq!(assignment.value(), " \" value\"");
        assert!(assignment.is_exported());
        assert_eq!(
            &source[assignment.name_range().start()..assignment.name_range().end()],
            assignment.name()
        );

        let SyntaxKind::Directive(directive) = tree.nodes()[1].kind() else {
            panic!("expected directive");
        };
        assert_eq!(directive.keyword(), DirectiveKeyword::Inherit);
        assert_eq!(directive.arguments(), "base \\\n  extra");
        assert!(directive.is_continued());

        let SyntaxKind::Function(function) = tree.nodes()[2].kind() else {
            panic!("expected function");
        };
        assert_eq!(function.function_kind(), FunctionKind::Python);
        assert_eq!(function.name(), Some("do_install"));
        assert!(function.is_fakeroot());

        let SyntaxKind::PythonDefinition(definition) = tree.nodes()[3].kind() else {
            panic!("expected Python definition");
        };
        assert_eq!(definition.name(), "helper");
    }

    #[test]
    fn retains_blank_comments_and_unknown_syntax() {
        let tree = parse("\n  # comment\nopaque\n").unwrap();
        assert!(matches!(tree.nodes()[0].kind(), SyntaxKind::Blank));
        assert!(matches!(tree.nodes()[1].kind(), SyntaxKind::Comment));
        assert!(matches!(tree.nodes()[2].kind(), SyntaxKind::Unknown));
    }

    #[test]
    fn directives_without_arguments_have_empty_ranges() {
        let tree = parse("include_all\n").unwrap();
        let SyntaxKind::Directive(directive) = tree.nodes()[0].kind() else {
            panic!("expected directive");
        };

        assert_eq!(directive.arguments(), "");
        assert_eq!(directive.arguments_range(), TextRange::new(11, 11));
    }

    #[test]
    fn reports_incomplete_structures() {
        let error = parse("do_build() {\n").unwrap_err();
        assert_eq!(error.line(), 1);
        assert_eq!(error.message(), "function body has no closing brace");

        let error = parse("\nFOO = \"unterminated\n").unwrap_err();
        assert_eq!(error.line(), 2);
        assert_eq!(
            error.message(),
            "top-level assignment contains an unclosed quote"
        );
    }
}

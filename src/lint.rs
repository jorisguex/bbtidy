use crate::{
    FormatError, comment_start, find_assignment_operator, find_brace_block_end,
    find_continuation_end, find_python_def_end, format, function_opening_brace, get_line_col,
    has_line_continuation, is_assignment_left_hand_side, is_python_def_start, next_line_end,
    split_line_ending,
};
use std::collections::HashSet;
use std::fmt;

const SUMMARY_LIMIT: usize = 80;

static LINT_RULES: &[LintRule] = &[
    LintRule::new(
        "BBT001",
        "trailing-whitespace",
        LintSeverity::Warning,
        "Lines must not end with spaces or tabs.",
    ),
    LintRule::new(
        "BBT002",
        "final-newline",
        LintSeverity::Warning,
        "Non-empty files must end with a newline.",
    ),
    LintRule::new(
        "BBT003",
        "summary-length",
        LintSeverity::Warning,
        "A literal SUMMARY must not exceed 80 characters.",
    ),
    LintRule::new(
        "BBT004",
        "autorev",
        LintSeverity::Warning,
        "SRCREV assignments must use a fixed revision instead of ${AUTOREV}.",
    ),
    LintRule::new(
        "BBT005",
        "duplicate-inherit",
        LintSeverity::Warning,
        "A class must not be inherited more than once in one file.",
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LintSeverity {
    Info,
    Warning,
    Error,
}

impl fmt::Display for LintSeverity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info => formatter.write_str("info"),
            Self::Warning => formatter.write_str("warning"),
            Self::Error => formatter.write_str("error"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LintRule {
    id: &'static str,
    name: &'static str,
    severity: LintSeverity,
    description: &'static str,
}

impl LintRule {
    const fn new(
        id: &'static str,
        name: &'static str,
        severity: LintSeverity,
        description: &'static str,
    ) -> Self {
        Self {
            id,
            name,
            severity,
            description,
        }
    }

    pub const fn id(&self) -> &'static str {
        self.id
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn severity(&self) -> LintSeverity {
        self.severity
    }

    pub const fn description(&self) -> &'static str {
        self.description
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LintDiagnostic {
    rule_id: &'static str,
    severity: LintSeverity,
    line: usize,
    column: usize,
    message: String,
}

impl LintDiagnostic {
    fn new(
        rule: &'static LintRule,
        line: usize,
        column: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            rule_id: rule.id,
            severity: rule.severity,
            line,
            column,
            message: message.into(),
        }
    }

    pub const fn rule_id(&self) -> &'static str {
        self.rule_id
    }

    pub const fn severity(&self) -> LintSeverity {
        self.severity
    }

    pub const fn line(&self) -> usize {
        self.line
    }

    pub const fn column(&self) -> usize {
        self.column
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

pub fn lint_rules() -> &'static [LintRule] {
    LINT_RULES
}

/// Checks BitBake metadata with bbtidy's default lint rules.
///
/// Diagnostics are returned in source order. Structurally incomplete input
/// returns the same [`FormatError`] used by the formatter instead of producing
/// potentially misleading findings.
pub fn lint(text: &str) -> Result<Vec<LintDiagnostic>, FormatError> {
    let _ = format(text)?;

    let mut diagnostics = Vec::new();
    check_trailing_whitespace(text, &mut diagnostics);
    check_final_newline(text, &mut diagnostics);

    let statements = top_level_statements(text);
    check_assignments(&statements, &mut diagnostics);
    check_duplicate_inherits(&statements, &mut diagnostics);

    diagnostics.sort_by(|left, right| {
        (left.line, left.column, left.rule_id).cmp(&(right.line, right.column, right.rule_id))
    });
    Ok(diagnostics)
}

fn check_trailing_whitespace(text: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    let rule = &LINT_RULES[0];
    let mut line_start = 0;

    for line in text.split_inclusive('\n') {
        let (content, _) = split_line_ending(line);
        let trimmed = content.trim_end_matches([' ', '\t']);
        if trimmed.len() != content.len() {
            let (line, column) = get_line_col(text, line_start + trimmed.len());
            diagnostics.push(LintDiagnostic::new(
                rule,
                line,
                column,
                "line ends with whitespace",
            ));
        }
        line_start += line.len();
    }
}

fn check_final_newline(text: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    if text.is_empty() || text.ends_with('\n') {
        return;
    }

    let rule = &LINT_RULES[1];
    let (line, column) = get_line_col(text, text.len());
    diagnostics.push(LintDiagnostic::new(
        rule,
        line,
        column,
        "file does not end with a newline",
    ));
}

fn check_assignments(statements: &[TopLevelStatement<'_>], diagnostics: &mut Vec<LintDiagnostic>) {
    for statement in statements {
        let Some(assignment) = parse_assignment(statement.text) else {
            continue;
        };

        if assignment.name == "SUMMARY" {
            check_summary(statement, &assignment, diagnostics);
        }
        if is_srcrev_name(assignment.name) {
            check_autorev(statement, &assignment, diagnostics);
        }
    }
}

fn check_summary(
    statement: &TopLevelStatement<'_>,
    assignment: &Assignment<'_>,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let Some((summary, _)) = simple_quoted_value(assignment.value) else {
        return;
    };
    if summary.contains(['\r', '\n', '$']) {
        return;
    }

    let length = summary.chars().count();
    if length <= SUMMARY_LIMIT {
        return;
    }

    let rule = &LINT_RULES[2];
    let (line, column) = get_line_col(statement.source, statement.start + assignment.name_offset);
    diagnostics.push(LintDiagnostic::new(
        rule,
        line,
        column,
        format!("SUMMARY is {length} characters; limit it to {SUMMARY_LIMIT}"),
    ));
}

fn check_autorev(
    statement: &TopLevelStatement<'_>,
    assignment: &Assignment<'_>,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let value =
        &assignment.value[..comment_start(assignment.value).unwrap_or(assignment.value.len())];
    let Some(relative_offset) = value.find("${AUTOREV}") else {
        return;
    };

    let rule = &LINT_RULES[3];
    let (line, column) = get_line_col(
        statement.source,
        statement.start + assignment.value_offset + relative_offset,
    );
    diagnostics.push(LintDiagnostic::new(
        rule,
        line,
        column,
        format!(
            "{} uses ${{AUTOREV}}; pin a source revision for reproducible builds",
            assignment.name
        ),
    ));
}

fn check_duplicate_inherits(
    statements: &[TopLevelStatement<'_>],
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let rule = &LINT_RULES[4];
    let mut inherited = HashSet::new();

    for statement in statements {
        let Some(arguments_offset) = inherit_arguments_offset(statement.text) else {
            continue;
        };
        let code_end = comment_start(statement.text).unwrap_or(statement.text.len());
        if arguments_offset >= code_end {
            continue;
        }

        let mut dynamic_expression = false;
        for (relative_offset, class) in words(&statement.text[arguments_offset..code_end]) {
            if dynamic_expression {
                dynamic_expression = !class.contains('}');
                continue;
            }
            if class.contains('$') {
                dynamic_expression = !class.contains('}');
                continue;
            }
            if class == "\\" || class.contains(['{', '}']) {
                continue;
            }
            if inherited.insert(class) {
                continue;
            }

            let offset = statement.start + arguments_offset + relative_offset;
            let (line, column) = get_line_col(statement.source, offset);
            diagnostics.push(LintDiagnostic::new(
                rule,
                line,
                column,
                format!("class '{class}' is inherited more than once"),
            ));
        }
    }
}

fn inherit_arguments_offset(statement: &str) -> Option<usize> {
    let leading = statement.len() - statement.trim_start_matches([' ', '\t']).len();
    let trimmed = &statement[leading..];
    for keyword in ["inherit_defer", "inherit"] {
        let Some(rest) = trimmed.strip_prefix(keyword) else {
            continue;
        };
        if rest.is_empty() || !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let whitespace = rest.len() - rest.trim_start().len();
        return Some(leading + keyword.len() + whitespace);
    }
    None
}

fn words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        let whitespace = text[offset..].find(|character: char| !character.is_whitespace())?;
        offset += whitespace;
        let start = offset;
        let length = text[start..]
            .find(char::is_whitespace)
            .unwrap_or(text.len() - start);
        offset = start + length;
        Some((start, &text[start..start + length]))
    })
}

fn is_srcrev_name(name: &str) -> bool {
    name == "SRCREV" || name.starts_with("SRCREV:") || name.starts_with("SRCREV_")
}

fn simple_quoted_value(value: &str) -> Option<(&str, usize)> {
    let leading = value.len() - value.trim_start_matches([' ', '\t']).len();
    let value = &value[leading..];
    let quote = *value.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }

    let mut escaped = false;
    for index in 1..value.len() {
        let byte = value.as_bytes()[index];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == quote {
            let remainder = value[index + 1..].trim();
            if remainder.is_empty() || remainder.starts_with('#') {
                return Some((&value[1..index], leading + 1));
            }
            return None;
        }
    }
    None
}

struct Assignment<'a> {
    name: &'a str,
    name_offset: usize,
    value: &'a str,
    value_offset: usize,
}

fn parse_assignment(statement: &str) -> Option<Assignment<'_>> {
    let (content, _) = split_line_ending(statement);
    let (operator_start, operator) = find_assignment_operator(content)?;
    let left = content[..operator_start].trim_end();
    if !is_assignment_left_hand_side(left) {
        return None;
    }

    let (name, name_offset) = if let Some(rest) = left.strip_prefix("export") {
        if rest.starts_with(char::is_whitespace) {
            let whitespace = rest.len() - rest.trim_start().len();
            (rest.trim_start(), "export".len() + whitespace)
        } else {
            (left, 0)
        }
    } else {
        (left, 0)
    };
    let value_offset = operator_start + operator.lexeme().len();

    Some(Assignment {
        name,
        name_offset,
        value: &content[value_offset..],
        value_offset,
    })
}

struct TopLevelStatement<'a> {
    source: &'a str,
    start: usize,
    text: &'a str,
}

fn top_level_statements(text: &str) -> Vec<TopLevelStatement<'_>> {
    let mut statements = Vec::new();
    let mut offset = 0;

    while offset < text.len() {
        let line_end = next_line_end(text, offset);
        let line = &text[offset..line_end];

        if let Some((opening_brace, function_kind)) = function_opening_brace(line) {
            let Some(block_end) = find_brace_block_end(text, offset + opening_brace, function_kind)
            else {
                break;
            };
            offset = block_end;
            continue;
        }
        if is_python_def_start(line) {
            offset = find_python_def_end(text, line_end);
            continue;
        }

        let statement_end = if has_line_continuation(line) {
            let Some(block_end) = find_continuation_end(text, offset) else {
                break;
            };
            block_end
        } else {
            line_end
        };
        statements.push(TopLevelStatement {
            source: text,
            start: offset,
            text: &text[offset..statement_end],
        });
        offset = statement_end;
    }

    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_stable_rule_metadata() {
        assert_eq!(
            lint_rules().iter().map(LintRule::id).collect::<Vec<_>>(),
            ["BBT001", "BBT002", "BBT003", "BBT004", "BBT005"]
        );
        assert!(
            lint_rules()
                .iter()
                .all(|rule| rule.severity() == LintSeverity::Warning)
        );
    }

    #[test]
    fn reports_whitespace_and_final_newline_locations() {
        let diagnostics = lint("SUMMARY = \"demo\"  \nLICENSE = \"MIT\"").unwrap();

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(
            (
                diagnostics[0].rule_id(),
                diagnostics[0].line(),
                diagnostics[0].column()
            ),
            ("BBT001", 1, 17)
        );
        assert_eq!(
            (
                diagnostics[1].rule_id(),
                diagnostics[1].line(),
                diagnostics[1].column()
            ),
            ("BBT002", 2, 16)
        );

        let utf8 = lint("# é  \n").unwrap();
        assert_eq!((utf8[0].line(), utf8[0].column()), (1, 4));
    }

    #[test]
    fn reports_long_literal_summary_but_skips_dynamic_values() {
        let long_summary = "a".repeat(81);
        let input = format!(
            "SUMMARY = \"{long_summary}\"\nSUMMARY:class-native = \"${{SUMMARY}} extension\"\n"
        );
        let diagnostics = lint(&input).unwrap();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].rule_id(), "BBT003");
        assert_eq!(diagnostics[0].line(), 1);
        assert!(diagnostics[0].message().contains("81 characters"));
    }

    #[test]
    fn reports_autorev_for_named_and_overridden_srcrev() {
        let input = concat!(
            "SRCREV = \"${AUTOREV}\"\n",
            "SRCREV:machine ?= '${AUTOREV}'\n",
            "SRCREV_meta = \"fixed\"\n",
            "OTHER = \"${AUTOREV}\"\n",
            "SRCREV = \"fixed\" # ${AUTOREV}\n",
        );
        let diagnostics = lint(input).unwrap();

        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics.iter().all(|item| item.rule_id() == "BBT004"));
        assert_eq!(diagnostics[0].line(), 1);
        assert_eq!(diagnostics[1].line(), 2);
    }

    #[test]
    fn reports_static_duplicate_inherits_outside_function_bodies() {
        let input = concat!(
            "inherit autotools pkgconfig\n",
            "inherit_defer cmake autotools\n",
            "inherit ${VARNAME} ${VARNAME}\n",
            "inherit ${@bb.utils.contains('FEATURES', 'x', 'dynamic', '', d)}\n",
            "inherit ${@bb.utils.contains('FEATURES', 'x', 'dynamic', '', d)}\n",
            "do_example() {\n",
            "    inherit autotools\n",
            "}\n",
        );
        let diagnostics = lint(input).unwrap();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].rule_id(), "BBT005");
        assert_eq!(diagnostics[0].line(), 2);
        assert_eq!(diagnostics[0].column(), 21);
    }

    #[test]
    fn malformed_input_returns_a_structural_error() {
        let error = lint("BROKEN = \"value\n").unwrap_err();

        assert_eq!(error.line(), 1);
        assert_eq!(
            error.message(),
            "top-level assignment contains an unclosed quote"
        );
    }
}

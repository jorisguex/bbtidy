use crate::{
    AssignmentSyntax, DirectiveKeyword, FormatError, SyntaxKind, SyntaxTree, WorkspaceCandidate,
    WorkspaceIndex, comment_start, get_line_col, parse, split_line_ending,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::str::FromStr;

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
    LintRule::new(
        "BBT006",
        "unresolved-require",
        LintSeverity::Warning,
        "A static require target must resolve within the indexed layers.",
    ),
    LintRule::new(
        "BBT007",
        "unresolved-inherit",
        LintSeverity::Warning,
        "A static inherited class must resolve within the indexed layers.",
    ),
    LintRule::new(
        "BBT008",
        "ambiguous-require",
        LintSeverity::Warning,
        "A static require target must resolve to one highest-priority file.",
    ),
    LintRule::new(
        "BBT009",
        "ambiguous-inherit",
        LintSeverity::Warning,
        "A static inherited class must resolve to one highest-priority definition.",
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LintSeverity {
    Info,
    Warning,
    Error,
}

impl FromStr for LintSeverity {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "info" => Ok(Self::Info),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            _ => Err(format!(
                "invalid lint severity '{value}'; expected info, warning, or error"
            )),
        }
    }
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

/// Configuration for selecting lint rules and overriding their severities.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LintOptions {
    disabled_rules: BTreeSet<String>,
    severity_overrides: BTreeMap<String, LintSeverity>,
}

impl LintOptions {
    /// Disables diagnostics for a stable lint rule ID.
    pub fn disable_rule(&mut self, rule_id: impl Into<String>) {
        self.disabled_rules.insert(rule_id.into());
    }

    /// Overrides the severity for a stable lint rule ID.
    pub fn set_severity(&mut self, rule_id: impl Into<String>, severity: LintSeverity) {
        self.severity_overrides.insert(rule_id.into(), severity);
    }

    pub(crate) fn from_parts(
        disabled_rules: BTreeSet<String>,
        severity_overrides: BTreeMap<String, LintSeverity>,
    ) -> Self {
        Self {
            disabled_rules,
            severity_overrides,
        }
    }

    fn is_enabled(&self, rule_id: &str) -> bool {
        !self.disabled_rules.contains(rule_id)
    }

    fn severity_for(&self, diagnostic: &LintDiagnostic) -> LintSeverity {
        self.severity_overrides
            .get(diagnostic.rule_id())
            .copied()
            .unwrap_or_else(|| diagnostic.severity())
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
    let tree = parse(text)?;
    Ok(lint_syntax(&tree))
}

/// Checks source with caller-provided rule selection and severity settings.
pub fn lint_with_options(
    text: &str,
    options: &LintOptions,
) -> Result<Vec<LintDiagnostic>, FormatError> {
    let tree = parse(text)?;
    Ok(lint_syntax_with_options(&tree, options))
}

/// Checks source with caller-provided rule settings and an indexed workspace.
///
/// Workspace-aware rules are enabled only when `path` belongs to a complete
/// indexed layer. Dynamic references and incomplete single-file contexts are
/// intentionally ignored to avoid pretending to evaluate BitBake metadata.
pub fn lint_with_workspace(
    text: &str,
    path: &std::path::Path,
    workspace: &WorkspaceIndex,
    options: &LintOptions,
) -> Result<Vec<LintDiagnostic>, FormatError> {
    let tree = parse(text)?;
    Ok(lint_syntax_with_workspace(&tree, path, workspace, options))
}

/// Checks a previously parsed syntax tree without reparsing its source.
pub fn lint_syntax(tree: &SyntaxTree<'_>) -> Vec<LintDiagnostic> {
    lint_syntax_with_options(tree, &LintOptions::default())
}

/// Checks a previously parsed syntax tree with caller-provided options.
pub fn lint_syntax_with_options(
    tree: &SyntaxTree<'_>,
    options: &LintOptions,
) -> Vec<LintDiagnostic> {
    finalize_diagnostics(collect_lint_diagnostics(tree), options)
}

/// Checks a previously parsed syntax tree with an indexed workspace.
pub fn lint_syntax_with_workspace(
    tree: &SyntaxTree<'_>,
    path: &std::path::Path,
    workspace: &WorkspaceIndex,
    options: &LintOptions,
) -> Vec<LintDiagnostic> {
    let mut diagnostics = collect_lint_diagnostics(tree);
    check_workspace_references(tree, path, workspace, &mut diagnostics);
    finalize_diagnostics(diagnostics, options)
}

fn collect_lint_diagnostics(tree: &SyntaxTree<'_>) -> Vec<LintDiagnostic> {
    let text = tree.source();
    let mut diagnostics = Vec::new();
    check_trailing_whitespace(text, &mut diagnostics);
    check_final_newline(text, &mut diagnostics);
    check_assignments(tree, &mut diagnostics);
    check_duplicate_inherits(tree, &mut diagnostics);
    diagnostics
}

fn finalize_diagnostics(
    mut diagnostics: Vec<LintDiagnostic>,
    options: &LintOptions,
) -> Vec<LintDiagnostic> {
    diagnostics.retain(|diagnostic| options.is_enabled(diagnostic.rule_id()));
    for diagnostic in &mut diagnostics {
        diagnostic.severity = options.severity_for(diagnostic);
    }

    diagnostics.sort_by(|left, right| {
        (left.line, left.column, left.rule_id).cmp(&(right.line, right.column, right.rule_id))
    });
    diagnostics
}

fn check_workspace_references(
    tree: &SyntaxTree<'_>,
    path: &std::path::Path,
    workspace: &WorkspaceIndex,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    if !workspace.is_complete_for(path) {
        return;
    }

    for node in tree.nodes() {
        let SyntaxKind::Directive(directive) = node.kind() else {
            continue;
        };
        let arguments = directive.arguments();
        let code_end = comment_start(arguments).unwrap_or(arguments.len());
        let arguments = &arguments[..code_end];
        match directive.keyword() {
            DirectiveKeyword::Require => {
                for (relative_offset, target) in static_words(arguments) {
                    let candidates = workspace.file_candidates(path, target);
                    if candidates.is_empty() {
                        let rule = &LINT_RULES[5];
                        let offset = directive.arguments_range().start() + relative_offset;
                        let (line, column) = get_line_col(tree.source(), offset);
                        diagnostics.push(LintDiagnostic::new(
                            rule,
                            line,
                            column,
                            format!("required file '{target}' was not found in indexed layers"),
                        ));
                        continue;
                    }
                    if let Some(message) = ambiguity_message(&candidates) {
                        let rule = &LINT_RULES[7];
                        let offset = directive.arguments_range().start() + relative_offset;
                        let (line, column) = get_line_col(tree.source(), offset);
                        diagnostics.push(LintDiagnostic::new(
                            rule,
                            line,
                            column,
                            format!("required file '{target}' {message}"),
                        ));
                    }
                }
            }
            DirectiveKeyword::Inherit | DirectiveKeyword::InheritDefer => {
                for (relative_offset, class) in static_words(arguments) {
                    let candidates = workspace.class_candidates(class);
                    if candidates.is_empty() {
                        let rule = &LINT_RULES[6];
                        let offset = directive.arguments_range().start() + relative_offset;
                        let (line, column) = get_line_col(tree.source(), offset);
                        diagnostics.push(LintDiagnostic::new(
                            rule,
                            line,
                            column,
                            format!("inherited class '{class}' was not found in indexed layers"),
                        ));
                        continue;
                    }
                    if let Some(message) = ambiguity_message(&candidates) {
                        let rule = &LINT_RULES[8];
                        let offset = directive.arguments_range().start() + relative_offset;
                        let (line, column) = get_line_col(tree.source(), offset);
                        diagnostics.push(LintDiagnostic::new(
                            rule,
                            line,
                            column,
                            format!("inherited class '{class}' {message}"),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

fn ambiguity_message(candidates: &[WorkspaceCandidate<'_>]) -> Option<String> {
    let priority = candidates.first()?.priority();
    let highest_priority = candidates
        .iter()
        .filter(|candidate| candidate.priority() == priority)
        .collect::<Vec<_>>();
    if highest_priority.len() < 2 {
        return None;
    }

    let paths = highest_priority
        .iter()
        .map(|candidate| candidate.path().display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "matches multiple candidates at layer priority {priority}: {paths}"
    ))
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

fn check_assignments(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };

        if assignment.name() == "SUMMARY" {
            check_summary(tree.source(), assignment, diagnostics);
        }
        if is_srcrev_name(assignment.name()) {
            check_autorev(tree.source(), assignment, diagnostics);
        }
    }
}

fn check_summary(
    source: &str,
    assignment: &AssignmentSyntax<'_>,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let Some((summary, _)) = simple_quoted_value(assignment.value()) else {
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
    let (line, column) = get_line_col(source, assignment.name_range().start());
    diagnostics.push(LintDiagnostic::new(
        rule,
        line,
        column,
        format!("SUMMARY is {length} characters; limit it to {SUMMARY_LIMIT}"),
    ));
}

fn check_autorev(
    source: &str,
    assignment: &AssignmentSyntax<'_>,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let value = assignment.value();
    let value = &value[..comment_start(value).unwrap_or(value.len())];
    let Some(relative_offset) = value.find("${AUTOREV}") else {
        return;
    };

    let rule = &LINT_RULES[3];
    let (line, column) = get_line_col(source, assignment.value_range().start() + relative_offset);
    diagnostics.push(LintDiagnostic::new(
        rule,
        line,
        column,
        format!(
            "{} uses ${{AUTOREV}}; pin a source revision for reproducible builds",
            assignment.name()
        ),
    ));
}

fn check_duplicate_inherits(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    let rule = &LINT_RULES[4];
    let mut inherited = HashSet::new();

    for node in tree.nodes() {
        let SyntaxKind::Directive(directive) = node.kind() else {
            continue;
        };
        if !matches!(
            directive.keyword(),
            DirectiveKeyword::Inherit | DirectiveKeyword::InheritDefer
        ) {
            continue;
        }

        let arguments = directive.arguments();
        let code_end = comment_start(arguments).unwrap_or(arguments.len());
        let mut dynamic_expression = false;
        for (relative_offset, class) in words(&arguments[..code_end]) {
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

            let offset = directive.arguments_range().start() + relative_offset;
            let (line, column) = get_line_col(tree.source(), offset);
            diagnostics.push(LintDiagnostic::new(
                rule,
                line,
                column,
                format!("class '{class}' is inherited more than once"),
            ));
        }
    }
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

fn static_words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut dynamic_expression = false;
    words(text).filter_map(move |(offset, word)| {
        if dynamic_expression {
            dynamic_expression = !word.contains('}');
            return None;
        }
        if word.contains('$') || word.contains('{') {
            dynamic_expression = !word.contains('}');
            return None;
        }
        if word == "\\" || word.contains('}') {
            return None;
        }
        Some((offset, word))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_stable_rule_metadata() {
        assert_eq!(
            lint_rules().iter().map(LintRule::id).collect::<Vec<_>>(),
            [
                "BBT001", "BBT002", "BBT003", "BBT004", "BBT005", "BBT006", "BBT007", "BBT008",
                "BBT009",
            ]
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

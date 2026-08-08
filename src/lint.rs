use crate::semantic::{SemanticReport, SemanticSeverity};
use crate::workspace::global_class_assignment_kind;
use crate::{
    AssignmentOperator, AssignmentSyntax, DirectiveKeyword, FormatError, SyntaxKind, SyntaxTree,
    TextRange, WorkspaceCandidate, WorkspaceClassContext, WorkspaceDependencyKind,
    WorkspaceFileDirective, WorkspaceIndex, comment_start, get_line_col, parse, split_line_ending,
};
use crate::{BodyDiagnosticKind, FunctionKind, analyze_python_body, analyze_shell_body};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

const SUMMARY_LIMIT: usize = 80;
const RULE_BITBAKE_DIAGNOSTIC: usize = 18;
const RULE_RECIPE_NAME: usize = 19;
const RULE_RECIPE_VERSION: usize = 20;
const RULE_LICENSE_CHECKSUM: usize = 21;
const RULE_SOURCE_CHECKSUM: usize = 22;
const RULE_PACKAGECONFIG: usize = 23;
const RULE_PACKAGECONFIG_FORMAT: usize = 24;
const RULE_PACKAGE_SCOPE: usize = 25;
const RULE_PACKAGE_LIST: usize = 26;
const RULE_URI_PARAMETERS: usize = 27;
const RULE_LAYER_COLLECTIONS: usize = 28;
const RULE_LAYER_PATTERN: usize = 29;
const RULE_LAYER_PRIORITY: usize = 30;
const RULE_LAYER_DEPENDS: usize = 31;
const RULE_LAYER_SERIES_COMPAT: usize = 32;
const RULE_SHELL_SYNTAX: usize = 33;
const RULE_PYTHON_SYNTAX: usize = 34;
const RULE_PYTHON_INDENTATION: usize = 35;

static LINT_RULES: &[LintRule] = &[
    LintRule::new(
        "BBT001",
        "trailing-whitespace",
        LintSeverity::Warning,
        "Lines must not end with spaces or tabs.",
        true,
    ),
    LintRule::new(
        "BBT002",
        "final-newline",
        LintSeverity::Warning,
        "Non-empty files must end with a newline.",
        true,
    ),
    LintRule::new(
        "BBT003",
        "summary-length",
        LintSeverity::Warning,
        "A literal SUMMARY must not exceed 80 characters.",
        false,
    ),
    LintRule::new(
        "BBT004",
        "autorev",
        LintSeverity::Warning,
        "SRCREV assignments must use a fixed revision instead of ${AUTOREV}.",
        false,
    ),
    LintRule::new(
        "BBT005",
        "duplicate-inherit",
        LintSeverity::Warning,
        "A class must not be inherited more than once in one file.",
        false,
    ),
    LintRule::new(
        "BBT006",
        "unresolved-require",
        LintSeverity::Warning,
        "A static require target must resolve within the indexed layers.",
        false,
    ),
    LintRule::new(
        "BBT007",
        "unresolved-inherit",
        LintSeverity::Warning,
        "A static inherited class must resolve within the indexed layers.",
        false,
    ),
    LintRule::new(
        "BBT008",
        "ambiguous-require",
        LintSeverity::Warning,
        "A static require target must resolve to one highest-priority file.",
        false,
    ),
    LintRule::new(
        "BBT009",
        "ambiguous-inherit",
        LintSeverity::Warning,
        "A static inherited class must resolve to one highest-priority definition.",
        false,
    ),
    LintRule::new(
        "BBT010",
        "dependency-cycle",
        LintSeverity::Warning,
        "A static metadata dependency must not close a resolution cycle.",
        false,
    ),
    LintRule::new(
        "BBT011",
        "missing-summary",
        LintSeverity::Warning,
        "Recipes must declare a SUMMARY.",
        false,
    ),
    LintRule::new(
        "BBT012",
        "missing-description",
        LintSeverity::Warning,
        "Recipes must declare a DESCRIPTION.",
        false,
    ),
    LintRule::new(
        "BBT013",
        "missing-license",
        LintSeverity::Warning,
        "Recipes must declare a LICENSE.",
        false,
    ),
    LintRule::new(
        "BBT014",
        "file-paths-immediate",
        LintSeverity::Warning,
        "FILESEXTRAPATHS must use immediate expansion.",
        false,
    ),
    LintRule::new(
        "BBT015",
        "git-uri-protocol",
        LintSeverity::Warning,
        "Git fetch URLs must declare their transport protocol.",
        false,
    ),
    LintRule::new(
        "BBT016",
        "duplicate-assignment",
        LintSeverity::Warning,
        "A variable should not be assigned directly more than once in one file.",
        false,
    ),
    LintRule::new(
        "BBT017",
        "duplicate-function",
        LintSeverity::Warning,
        "A task or function should not be declared more than once in one file.",
        false,
    ),
    LintRule::new(
        "BBT018",
        "empty-directive",
        LintSeverity::Warning,
        "Metadata dependency directives must name a target.",
        false,
    ),
    LintRule::new(
        "BBT019",
        "bitbake-diagnostic",
        LintSeverity::Error,
        "BitBake reported a semantic diagnostic.",
        false,
    ),
    LintRule::new(
        "BBT020",
        "recipe-name",
        LintSeverity::Warning,
        "An explicit PN should match the recipe filename.",
        false,
    ),
    LintRule::new(
        "BBT021",
        "recipe-version",
        LintSeverity::Warning,
        "An explicit PV should match the version in the recipe filename.",
        false,
    ),
    LintRule::new(
        "BBT022",
        "license-checksum",
        LintSeverity::Warning,
        "Non-CLOSED recipes must provide valid license-file checksums.",
        false,
    ),
    LintRule::new(
        "BBT023",
        "source-checksum",
        LintSeverity::Warning,
        "Remote source archives must provide a valid checksum.",
        false,
    ),
    LintRule::new(
        "BBT024",
        "packageconfig",
        LintSeverity::Warning,
        "Enabled PACKAGECONFIG features must have flag definitions.",
        false,
    ),
    LintRule::new(
        "BBT025",
        "packageconfig-format",
        LintSeverity::Warning,
        "PACKAGECONFIG flags must provide enable, disable, and dependency fields.",
        false,
    ),
    LintRule::new(
        "BBT026",
        "package-scope",
        LintSeverity::Warning,
        "Package-scoped variables must refer to declared packages.",
        false,
    ),
    LintRule::new(
        "BBT027",
        "package-list",
        LintSeverity::Warning,
        "PACKAGES must not contain duplicate package names.",
        false,
    ),
    LintRule::new(
        "BBT028",
        "uri-parameters",
        LintSeverity::Warning,
        "SRC_URI entries must not contain invalid or conflicting parameters.",
        false,
    ),
    LintRule::new(
        "BBT029",
        "layer-collections",
        LintSeverity::Warning,
        "A layer must declare nonempty BBFILE_COLLECTIONS metadata.",
        false,
    ),
    LintRule::new(
        "BBT030",
        "layer-pattern",
        LintSeverity::Warning,
        "Every layer collection must define a BBFILE_PATTERN.",
        false,
    ),
    LintRule::new(
        "BBT031",
        "layer-priority",
        LintSeverity::Warning,
        "Every layer collection must define an integer BBFILE_PRIORITY.",
        false,
    ),
    LintRule::new(
        "BBT032",
        "layer-depends",
        LintSeverity::Warning,
        "LAYERDEPENDS entries must reference indexed layer collections.",
        false,
    ),
    LintRule::new(
        "BBT033",
        "layer-series-compat",
        LintSeverity::Warning,
        "Every layer collection must declare LAYERSERIES_COMPAT.",
        false,
    ),
    LintRule::new(
        "BBT034",
        "shell-syntax",
        LintSeverity::Warning,
        "Shell function bodies must have balanced control-flow constructs.",
        false,
    ),
    LintRule::new(
        "BBT035",
        "python-syntax",
        LintSeverity::Warning,
        "Embedded Python bodies must have valid delimiters and compound statements.",
        false,
    ),
    LintRule::new(
        "BBT036",
        "python-indentation",
        LintSeverity::Warning,
        "Embedded Python bodies must use consistent indentation.",
        false,
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

/// Controls which lint severities cause the CLI to exit with a finding status.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LintFailurePolicy {
    Info,
    #[default]
    Warning,
    Error,
    Never,
}

impl FromStr for LintFailurePolicy {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "info" => Ok(Self::Info),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            "never" => Ok(Self::Never),
            _ => Err(format!(
                "invalid lint failure policy '{value}'; expected info, warning, error, or never"
            )),
        }
    }
}

impl fmt::Display for LintFailurePolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info => formatter.write_str("info"),
            Self::Warning => formatter.write_str("warning"),
            Self::Error => formatter.write_str("error"),
            Self::Never => formatter.write_str("never"),
        }
    }
}

impl LintFailurePolicy {
    /// Returns whether a diagnostic at `severity` should fail a lint command.
    pub const fn is_blocking(self, severity: LintSeverity) -> bool {
        match self {
            Self::Info => true,
            Self::Warning => matches!(severity, LintSeverity::Warning | LintSeverity::Error),
            Self::Error => matches!(severity, LintSeverity::Error),
            Self::Never => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LintRule {
    id: &'static str,
    name: &'static str,
    severity: LintSeverity,
    description: &'static str,
    fixable: bool,
}

impl LintRule {
    const fn new(
        id: &'static str,
        name: &'static str,
        severity: LintSeverity,
        description: &'static str,
        fixable: bool,
    ) -> Self {
        Self {
            id,
            name,
            severity,
            description,
            fixable,
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

    /// Returns whether the rule has a safe, machine-applicable fix.
    pub const fn fixable(&self) -> bool {
        self.fixable
    }

    /// Returns whether the rule has a safe, machine-applicable fix.
    pub const fn is_fixable(&self) -> bool {
        self.fixable()
    }
}

/// Configuration for selecting lint rules, overriding their severities, and
/// deciding which findings fail a lint command.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LintOptions {
    disabled_rules: BTreeSet<String>,
    severity_overrides: BTreeMap<String, LintSeverity>,
    fail_on: LintFailurePolicy,
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

    /// Sets the minimum effective severity that fails a lint command.
    pub fn set_fail_on(&mut self, policy: LintFailurePolicy) {
        self.fail_on = policy;
    }

    /// Returns the policy used to decide whether findings fail a lint command.
    pub const fn fail_on(&self) -> LintFailurePolicy {
        self.fail_on
    }

    /// Returns whether any diagnostic meets this options' failure threshold.
    pub fn has_blocking_findings(&self, diagnostics: &[LintDiagnostic]) -> bool {
        diagnostics
            .iter()
            .any(|diagnostic| self.fail_on.is_blocking(diagnostic.severity()))
    }

    pub(crate) fn from_parts(
        disabled_rules: BTreeSet<String>,
        severity_overrides: BTreeMap<String, LintSeverity>,
        fail_on: LintFailurePolicy,
    ) -> Self {
        Self {
            disabled_rules,
            severity_overrides,
            fail_on,
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

/// A source edit proposed by a lint diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LintFix {
    range: TextRange,
    replacement: String,
    message: String,
}

impl LintFix {
    fn new(range: TextRange, replacement: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
            message: message.into(),
        }
    }

    /// Returns the half-open byte range replaced by this fix.
    pub const fn range(&self) -> TextRange {
        self.range
    }

    /// Returns the replacement text.
    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    /// Returns a human-readable description of the edit.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A failure applying one or more lint edits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LintFixError {
    InvalidRange { start: usize, end: usize },
    OverlappingRanges { first: TextRange, second: TextRange },
}

impl fmt::Display for LintFixError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRange { start, end } => {
                write!(
                    formatter,
                    "lint fix has invalid source range {start}..{end}"
                )
            }
            Self::OverlappingRanges { first, second } => write!(
                formatter,
                "lint fixes overlap at {}..{} and {}..{}",
                first.start(),
                first.end(),
                second.start(),
                second.end()
            ),
        }
    }
}

impl std::error::Error for LintFixError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LintDiagnostic {
    rule_id: &'static str,
    severity: LintSeverity,
    line: usize,
    column: usize,
    end_line: usize,
    end_column: usize,
    range: TextRange,
    message: String,
    help: Option<String>,
    fixes: Vec<LintFix>,
}

impl LintDiagnostic {
    fn at(
        rule: &'static LintRule,
        source: &str,
        range: TextRange,
        message: impl Into<String>,
    ) -> Self {
        let (line, column) = get_line_col(source, range.start());
        let (end_line, end_column) = get_line_col(source, range.end());
        Self {
            rule_id: rule.id,
            severity: rule.severity,
            line,
            column,
            end_line,
            end_column,
            range,
            message: message.into(),
            help: None,
            fixes: Vec::new(),
        }
    }

    fn with_fix(mut self, fix: LintFix, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self.fixes.push(fix);
        self
    }

    pub(crate) fn external(
        rule: &'static LintRule,
        severity: LintSeverity,
        line: Option<usize>,
        column: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        let line = line.unwrap_or(1).max(1);
        let column = column.unwrap_or(1).max(1);
        Self {
            rule_id: rule.id,
            severity,
            line,
            column,
            end_line: line,
            end_column: column.saturating_add(1),
            range: TextRange::new(0, 0),
            message: message.into(),
            help: None,
            fixes: Vec::new(),
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

    /// Returns the one-based ending line of the primary range.
    pub const fn end_line(&self) -> usize {
        self.end_line
    }

    /// Returns the one-based ending column of the primary range.
    pub const fn end_column(&self) -> usize {
        self.end_column
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the primary source range associated with this finding.
    pub const fn range(&self) -> TextRange {
        self.range
    }

    /// Returns the optional explanation or suggested next step.
    pub fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }

    /// Returns safe edits that can be applied without human judgment.
    pub fn fixes(&self) -> &[LintFix] {
        &self.fixes
    }

    /// Returns whether this diagnostic has at least one safe fix.
    pub fn is_fixable(&self) -> bool {
        !self.fixes.is_empty()
    }
}

pub fn lint_rules() -> &'static [LintRule] {
    LINT_RULES
}

/// A lint diagnostic produced outside a source file, such as a BitBake
/// diagnostic or a finding against a fully expanded target environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalLintDiagnostic {
    pub label: String,
    pub diagnostic: LintDiagnostic,
}

/// Converts BitBake diagnostics and resolved target metadata into the same
/// rule-aware diagnostics used by ordinary linting.
pub fn semantic_lint_diagnostics(
    report: &SemanticReport,
    options: &LintOptions,
) -> Vec<ExternalLintDiagnostic> {
    let rule = &LINT_RULES[RULE_BITBAKE_DIAGNOSTIC];
    let mut findings = Vec::new();

    for diagnostic in report.diagnostics() {
        let severity = match diagnostic.severity() {
            SemanticSeverity::Debug | SemanticSeverity::Note => LintSeverity::Info,
            SemanticSeverity::Warning => LintSeverity::Warning,
            SemanticSeverity::Error => LintSeverity::Error,
        };
        let label = diagnostic
            .path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "bitbake".to_owned());
        findings.push(ExternalLintDiagnostic {
            label,
            diagnostic: LintDiagnostic::external(
                rule,
                severity,
                diagnostic.line(),
                diagnostic.column(),
                format!("BitBake: {}", diagnostic.message()),
            ),
        });
    }

    if !report.parse_succeeded() && !report.has_errors() {
        findings.push(ExternalLintDiagnostic {
            label: "bitbake".to_owned(),
            diagnostic: LintDiagnostic::external(
                rule,
                LintSeverity::Error,
                None,
                None,
                "BitBake parse failed without a source diagnostic",
            ),
        });
    }
    if report.parse_succeeded() && !report.target_queries_succeeded() && !report.has_errors() {
        findings.push(ExternalLintDiagnostic {
            label: "bitbake".to_owned(),
            diagnostic: LintDiagnostic::external(
                rule,
                LintSeverity::Error,
                None,
                None,
                "BitBake target query failed without a source diagnostic",
            ),
        });
    }

    for environment in report.environments() {
        let label = format!("bitbake -e {}", environment.target());
        for (rule, variable, message) in [
            (
                &LINT_RULES[10],
                "SUMMARY",
                "BitBake resolved SUMMARY to an empty value",
            ),
            (
                &LINT_RULES[11],
                "DESCRIPTION",
                "BitBake resolved DESCRIPTION to an empty value",
            ),
            (
                &LINT_RULES[12],
                "LICENSE",
                "BitBake resolved LICENSE to an empty value",
            ),
        ] {
            if environment
                .get(variable)
                .is_some_and(|value| !value.trim().is_empty())
            {
                continue;
            }
            findings.push(ExternalLintDiagnostic {
                label: label.clone(),
                diagnostic: LintDiagnostic::external(
                    rule,
                    rule.severity(),
                    None,
                    None,
                    format!(
                        "target '{}' has no resolved {variable}: {message}",
                        environment.target()
                    ),
                ),
            });
        }

        if environment
            .get("SRCREV")
            .is_some_and(|value| value.contains("${AUTOREV}") || value.contains("AUTOINC"))
            || environment
                .get("SRCPV")
                .is_some_and(|value| value.contains("AUTOINC"))
        {
            findings.push(ExternalLintDiagnostic {
                label: label.clone(),
                diagnostic: LintDiagnostic::external(
                    &LINT_RULES[3],
                    LINT_RULES[3].severity(),
                    None,
                    None,
                    format!(
                        "target '{}' resolves SRCREV through AUTOREV; pin a source revision for reproducible builds",
                        environment.target()
                    ),
                ),
            });
        }

        if let Some(value) = environment.get("SRC_URI") {
            for uri in value.split_whitespace() {
                if uri.starts_with("git://")
                    && !uri.split(';').any(|part| part.starts_with("protocol="))
                {
                    findings.push(ExternalLintDiagnostic {
                        label: label.clone(),
                        diagnostic: LintDiagnostic::external(
                            &LINT_RULES[14],
                            LINT_RULES[14].severity(),
                            None,
                            None,
                            format!(
                                "target '{}' resolves Git URI '{uri}' without a transport protocol",
                                environment.target()
                            ),
                        ),
                    });
                }

                if is_remote_archive(uri)
                    && !has_valid_checksum(uri, "md5sum")
                    && !has_valid_checksum(uri, "sha256sum")
                {
                    findings.push(ExternalLintDiagnostic {
                        label: label.clone(),
                        diagnostic: LintDiagnostic::external(
                            &LINT_RULES[RULE_SOURCE_CHECKSUM],
                            LINT_RULES[RULE_SOURCE_CHECKSUM].severity(),
                            None,
                            None,
                            format!(
                                "target '{}' resolves remote source URI '{uri}' without a valid md5sum or sha256sum",
                                environment.target()
                            ),
                        ),
                    });
                }

                let mut parameters = HashSet::new();
                for parameter in uri.split(';').skip(1) {
                    let Some((key, parameter_value)) = parameter.split_once('=') else {
                        continue;
                    };
                    let invalid = key.is_empty()
                        || !parameters.insert(key)
                        || parameter_value.is_empty()
                        || (key == "branch" && !uri.starts_with("git://"))
                        || (key == "protocol" && !uri.starts_with("git://"))
                        || (key == "protocol"
                            && !matches!(
                                parameter_value,
                                "git" | "http" | "https" | "ssh" | "file"
                            ));
                    if !invalid {
                        continue;
                    }
                    findings.push(ExternalLintDiagnostic {
                        label: label.clone(),
                        diagnostic: LintDiagnostic::external(
                            &LINT_RULES[RULE_URI_PARAMETERS],
                            LINT_RULES[RULE_URI_PARAMETERS].severity(),
                            None,
                            None,
                            format!(
                                "target '{}' resolves SRC_URI entry '{uri}' with invalid or conflicting parameter '{parameter}'",
                                environment.target()
                            ),
                        ),
                    });
                }
            }
        }

        if environment.get("LICENSE").is_some_and(|value| {
            !value
                .split_ascii_whitespace()
                .any(|license| license == "CLOSED")
        }) {
            let checksum = environment.get("LIC_FILES_CHKSUM").unwrap_or_default();
            let file_entries = checksum
                .split_whitespace()
                .filter(|uri| uri.starts_with("file://"))
                .collect::<Vec<_>>();
            if file_entries.is_empty() {
                findings.push(ExternalLintDiagnostic {
                    label: label.clone(),
                    diagnostic: LintDiagnostic::external(
                        &LINT_RULES[RULE_LICENSE_CHECKSUM],
                        LINT_RULES[RULE_LICENSE_CHECKSUM].severity(),
                        None,
                        None,
                        format!(
                            "target '{}' has non-CLOSED LICENSE but no resolved LIC_FILES_CHKSUM file entry",
                            environment.target()
                        ),
                    ),
                });
            } else if file_entries
                .iter()
                .any(|uri| !has_valid_checksum(uri, "md5") && !has_valid_checksum(uri, "sha256"))
            {
                findings.push(ExternalLintDiagnostic {
                    label,
                    diagnostic: LintDiagnostic::external(
                        &LINT_RULES[RULE_LICENSE_CHECKSUM],
                        LINT_RULES[RULE_LICENSE_CHECKSUM].severity(),
                        None,
                        None,
                        format!(
                            "target '{}' resolves a license file without a valid md5 or sha256 checksum",
                            environment.target()
                        ),
                    ),
                });
            }
        }
    }

    findings.retain(|finding| options.is_enabled(finding.diagnostic.rule_id()));
    for finding in &mut findings {
        finding.diagnostic.severity = options.severity_for(&finding.diagnostic);
    }
    findings.sort_by(|left, right| {
        (
            left.label.as_str(),
            left.diagnostic.line,
            left.diagnostic.column,
            left.diagnostic.rule_id,
        )
            .cmp(&(
                right.label.as_str(),
                right.diagnostic.line,
                right.diagnostic.column,
                right.diagnostic.rule_id,
            ))
    });
    findings
}

/// Runs BitBake and converts its semantic results into lint diagnostics.
///
/// This is the library equivalent of the CLI's `lint --semantic` integration.
/// Source-local rules still use [`lint_with_workspace`]; this function covers
/// BitBake diagnostics and checks that require resolved target environments.
pub fn lint_with_bitbake(
    semantic_options: &crate::semantic::SemanticOptions,
    lint_options: &LintOptions,
) -> Result<(SemanticReport, Vec<ExternalLintDiagnostic>), crate::semantic::SemanticError> {
    let report = crate::semantic::analyze_bitbake(semantic_options)?;
    let findings = semantic_lint_diagnostics(&report, lint_options);
    Ok((report, findings))
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

/// Applies all safe edits proposed by `diagnostics` to `text`.
///
/// Edits are validated before any mutation is made. This makes overlapping
/// or stale edit plans fail atomically instead of producing partially edited
/// source.
pub fn apply_lint_fixes(
    text: &str,
    diagnostics: &[LintDiagnostic],
) -> Result<String, LintFixError> {
    let mut fixes = diagnostics
        .iter()
        .flat_map(|diagnostic| diagnostic.fixes.iter())
        .collect::<Vec<_>>();
    fixes.sort_by_key(|fix| (fix.range.start(), fix.range.end()));

    for fix in &fixes {
        let range = fix.range;
        if range.start() > range.end()
            || range.end() > text.len()
            || !text.is_char_boundary(range.start())
            || !text.is_char_boundary(range.end())
        {
            return Err(LintFixError::InvalidRange {
                start: range.start(),
                end: range.end(),
            });
        }
    }
    for pair in fixes.windows(2) {
        if pair[0].range.end() > pair[1].range.start() {
            return Err(LintFixError::OverlappingRanges {
                first: pair[0].range,
                second: pair[1].range,
            });
        }
    }

    let mut fixed = text.to_owned();
    for fix in fixes.into_iter().rev() {
        fixed.replace_range(fix.range.start()..fix.range.end(), &fix.replacement);
    }
    Ok(fixed)
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
    check_recipe_qa(tree, path, &mut diagnostics);
    if workspace.is_complete_for(path) {
        check_recipe_metadata(tree, path, &mut diagnostics);
        check_workspace_references(tree, path, workspace, &mut diagnostics);
        if is_layer_configuration(path) {
            check_layer_qa(tree, workspace, &mut diagnostics);
        }
    }
    finalize_diagnostics(diagnostics, options)
}

fn collect_lint_diagnostics(tree: &SyntaxTree<'_>) -> Vec<LintDiagnostic> {
    let text = tree.source();
    let mut diagnostics = Vec::new();
    check_trailing_whitespace(text, &mut diagnostics);
    check_final_newline(text, &mut diagnostics);
    check_assignments(tree, &mut diagnostics);
    check_duplicate_functions(tree, &mut diagnostics);
    check_body_diagnostics(tree, &mut diagnostics);
    check_empty_directives(tree, &mut diagnostics);
    check_duplicate_inherits(tree, &mut diagnostics);
    diagnostics
}

fn check_body_diagnostics(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    for node in tree.nodes() {
        let (body_range, body_diagnostics) = match node.kind() {
            SyntaxKind::Function(function) => {
                let body =
                    &tree.source()[function.body_range().start()..function.body_range().end()];
                let diagnostics = match function.function_kind() {
                    FunctionKind::Shell => analyze_shell_body(body),
                    FunctionKind::Python => analyze_python_body(body),
                };
                (function.body_range(), diagnostics)
            }
            SyntaxKind::PythonDefinition(definition) => {
                let body =
                    &tree.source()[definition.body_range().start()..definition.body_range().end()];
                (definition.body_range(), analyze_python_body(body))
            }
            _ => continue,
        };

        for body_diagnostic in body_diagnostics {
            let rule = match body_diagnostic.kind() {
                BodyDiagnosticKind::ShellSyntax => &LINT_RULES[RULE_SHELL_SYNTAX],
                BodyDiagnosticKind::PythonSyntax => &LINT_RULES[RULE_PYTHON_SYNTAX],
                BodyDiagnosticKind::PythonIndentation => &LINT_RULES[RULE_PYTHON_INDENTATION],
            };
            let relative = body_diagnostic.range();
            let range = TextRange::new(
                body_range.start() + relative.start(),
                body_range.start() + relative.end(),
            );
            diagnostics.push(LintDiagnostic::at(
                rule,
                tree.source(),
                range,
                body_diagnostic.message(),
            ));
        }
    }
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
    let class_contexts = workspace.class_contexts_for_path(path);
    let class_context = class_contexts[0];
    for node in tree.nodes() {
        if let SyntaxKind::Assignment(assignment) = node.kind()
            && class_contexts.contains(&WorkspaceClassContext::Global)
        {
            if let Some(kind) = global_class_assignment_kind(assignment.name())
                && let Some((value, value_offset)) = simple_quoted_value(assignment.value())
            {
                for (relative_offset, class) in static_words(value) {
                    check_class_reference(
                        tree,
                        path,
                        workspace,
                        &[WorkspaceClassContext::Global],
                        assignment.value_range().start() + value_offset,
                        relative_offset,
                        class,
                        kind,
                        diagnostics,
                    );
                }
            }
            continue;
        }

        let SyntaxKind::Directive(directive) = node.kind() else {
            continue;
        };
        let arguments = directive.arguments();
        let code_end = comment_start(arguments).unwrap_or(arguments.len());
        let arguments = &arguments[..code_end];
        match directive.keyword() {
            DirectiveKeyword::Require => {
                for (relative_offset, target) in static_words(arguments) {
                    let candidates = workspace.file_candidates_for(
                        path,
                        target,
                        WorkspaceFileDirective::Require,
                    );
                    if candidates.is_empty() {
                        let rule = &LINT_RULES[5];
                        let offset = directive.arguments_range().start() + relative_offset;
                        diagnostics.push(LintDiagnostic::at(
                            rule,
                            tree.source(),
                            TextRange::new(offset, offset + target.len()),
                            format!("required file '{target}' was not found in indexed layers"),
                        ));
                        continue;
                    }
                    if let Some(message) = ambiguity_message(&candidates) {
                        let rule = &LINT_RULES[7];
                        let offset = directive.arguments_range().start() + relative_offset;
                        diagnostics.push(LintDiagnostic::at(
                            rule,
                            tree.source(),
                            TextRange::new(offset, offset + target.len()),
                            format!("required file '{target}' {message}"),
                        ));
                    }
                    if let Some(candidate) = candidates.first().copied() {
                        check_dependency_cycle(
                            tree,
                            path,
                            workspace,
                            directive.arguments_range().start(),
                            relative_offset,
                            class_context,
                            WorkspaceDependencyKind::Require,
                            candidate,
                            diagnostics,
                        );
                    }
                }
            }
            DirectiveKeyword::Inherit | DirectiveKeyword::InheritDefer => {
                for (relative_offset, class) in static_words(arguments) {
                    let kind = if matches!(directive.keyword(), DirectiveKeyword::Inherit) {
                        WorkspaceDependencyKind::Inherit
                    } else {
                        WorkspaceDependencyKind::InheritDefer
                    };
                    check_class_reference(
                        tree,
                        path,
                        workspace,
                        &class_contexts,
                        directive.arguments_range().start(),
                        relative_offset,
                        class,
                        kind,
                        diagnostics,
                    );
                }
            }
            DirectiveKeyword::Include | DirectiveKeyword::IncludeAll => {
                // BitBake treats both directives as optional: a missing
                // include is not an error, and include_all intentionally
                // expands to every match. Keep them out of unresolved and
                // ambiguity diagnostics while retaining their semantics in
                // the public workspace-resolution API.
                let file_directive = if matches!(directive.keyword(), DirectiveKeyword::Include) {
                    WorkspaceFileDirective::Include
                } else {
                    WorkspaceFileDirective::IncludeAll
                };
                let kind = if matches!(directive.keyword(), DirectiveKeyword::Include) {
                    WorkspaceDependencyKind::Include
                } else {
                    WorkspaceDependencyKind::IncludeAll
                };
                for (relative_offset, target) in static_words(arguments) {
                    let candidates = workspace.file_candidates_for(path, target, file_directive);
                    let candidates = if matches!(kind, WorkspaceDependencyKind::IncludeAll) {
                        candidates
                    } else {
                        candidates.into_iter().take(1).collect()
                    };
                    for candidate in candidates {
                        check_dependency_cycle(
                            tree,
                            path,
                            workspace,
                            directive.arguments_range().start(),
                            relative_offset,
                            class_context,
                            kind,
                            candidate,
                            diagnostics,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_recipe_metadata(
    tree: &SyntaxTree<'_>,
    path: &Path,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    if path.extension().and_then(|extension| extension.to_str()) != Some("bb") {
        return;
    }

    let assignments = tree
        .nodes()
        .iter()
        .filter_map(|node| match node.kind() {
            SyntaxKind::Assignment(assignment) => Some(assignment.name()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let source = tree.source();
    for (rule, name, message) in [
        (
            &LINT_RULES[10],
            "SUMMARY",
            "recipe is missing a SUMMARY assignment",
        ),
        (
            &LINT_RULES[11],
            "DESCRIPTION",
            "recipe is missing a DESCRIPTION assignment",
        ),
        (
            &LINT_RULES[12],
            "LICENSE",
            "recipe is missing a LICENSE assignment",
        ),
    ] {
        if assignments.contains(name) {
            continue;
        }
        let end = source.len();
        diagnostics.push(LintDiagnostic::at(
            rule,
            source,
            TextRange::new(end, end),
            message,
        ));
    }
}

fn check_recipe_qa(tree: &SyntaxTree<'_>, path: &Path, diagnostics: &mut Vec<LintDiagnostic>) {
    if path.extension().and_then(|extension| extension.to_str()) != Some("bb") {
        return;
    }

    check_recipe_identity(tree, path, diagnostics);
    check_license_checksum(tree, diagnostics);
    check_source_checksums(tree, diagnostics);
    check_packageconfig(tree, diagnostics);
    check_package_scope(tree, diagnostics);
    check_uri_parameters(tree, diagnostics);
}

fn check_recipe_identity(
    tree: &SyntaxTree<'_>,
    path: &Path,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return;
    };
    let Some((expected_name, expected_version)) = stem.rsplit_once('_') else {
        return;
    };
    if expected_name.is_empty()
        || expected_version.is_empty()
        || expected_name.contains(['$', '%'])
        || expected_version.contains(['$', '%'])
    {
        return;
    }

    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        let Some((value, _)) = simple_quoted_value(assignment.value()) else {
            continue;
        };
        if assignment.name() == "PN" && !value.contains('$') && value != expected_name {
            diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_RECIPE_NAME],
                tree.source(),
                assignment.name_range(),
                format!("PN '{value}' does not match recipe filename name '{expected_name}'"),
            ));
        }
        if assignment.name() == "PV" && !value.contains('$') && value != expected_version {
            diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_RECIPE_VERSION],
                tree.source(),
                assignment.name_range(),
                format!("PV '{value}' does not match recipe filename version '{expected_version}'"),
            ));
        }
    }
}

fn check_license_checksum(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    let mut license = None;
    let mut license_checksum = None;
    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        match assignment.name() {
            "LICENSE" => license = Some(assignment),
            "LIC_FILES_CHKSUM" => license_checksum = Some(assignment),
            _ => {}
        }
    }
    let Some(license) = license else {
        return;
    };
    let Some((license_value, _)) = simple_quoted_value(license.value()) else {
        return;
    };
    if license_value
        .split_ascii_whitespace()
        .any(|value| value == "CLOSED")
    {
        return;
    }

    let Some(checksum_assignment) = license_checksum else {
        diagnostics.push(LintDiagnostic::at(
            &LINT_RULES[RULE_LICENSE_CHECKSUM],
            tree.source(),
            license.name_range(),
            "non-CLOSED recipe is missing LIC_FILES_CHKSUM",
        ));
        return;
    };
    let Some((value, value_offset)) = simple_quoted_value(checksum_assignment.value()) else {
        return;
    };
    let mut found_file = false;
    for (relative_offset, uri) in static_words(value) {
        if !uri.starts_with("file://") {
            continue;
        }
        found_file = true;
        let invalid = !has_valid_checksum(uri, "md5") && !has_valid_checksum(uri, "sha256");
        if invalid {
            let offset = checksum_assignment.value_range().start() + value_offset + relative_offset;
            diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_LICENSE_CHECKSUM],
                tree.source(),
                TextRange::new(offset, offset + uri.len()),
                format!("license file URI '{uri}' is missing a valid md5 or sha256 checksum"),
            ));
        }
    }
    if !found_file && !value.trim().is_empty() {
        diagnostics.push(LintDiagnostic::at(
            &LINT_RULES[RULE_LICENSE_CHECKSUM],
            tree.source(),
            checksum_assignment.name_range(),
            "LIC_FILES_CHKSUM must contain at least one static file:// entry",
        ));
    }
}

fn check_source_checksums(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        if !is_src_uri_name(assignment.name()) {
            continue;
        }
        let Some((value, value_offset)) = simple_quoted_value(assignment.value()) else {
            continue;
        };
        for (relative_offset, uri) in static_words(value) {
            if !is_remote_archive(uri) {
                continue;
            }
            if has_valid_checksum(uri, "md5sum") || has_valid_checksum(uri, "sha256sum") {
                continue;
            }
            let offset = assignment.value_range().start() + value_offset + relative_offset;
            diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_SOURCE_CHECKSUM],
                tree.source(),
                TextRange::new(offset, offset + uri.len()),
                format!("remote source URI '{uri}' is missing a valid md5sum or sha256sum"),
            ));
        }
    }
}

fn check_packageconfig(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    let mut enabled = Vec::new();
    let mut definitions = HashSet::new();
    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        if is_packageconfig_value(assignment.name())
            && let Some((value, value_offset)) = simple_quoted_value(assignment.value())
        {
            for (relative_offset, feature) in static_words(value) {
                enabled.push((
                    feature.to_owned(),
                    assignment.value_range().start() + value_offset + relative_offset,
                    feature.len(),
                ));
            }
        }
        if let Some(feature) = packageconfig_feature(assignment.name()) {
            definitions.insert(feature.to_owned());
            let Some((value, _)) = simple_quoted_value(assignment.value()) else {
                continue;
            };
            if value.contains('$') {
                continue;
            }
            let fields = value.split(',').collect::<Vec<_>>();
            if !(3..=4).contains(&fields.len()) {
                diagnostics.push(LintDiagnostic::at(
                    &LINT_RULES[RULE_PACKAGECONFIG_FORMAT],
                    tree.source(),
                    assignment.name_range(),
                    format!(
                        "PACKAGECONFIG feature '{feature}' has {} fields; expected 3 or 4",
                        fields.len()
                    ),
                ));
            }
        }
    }
    for (feature, offset, length) in enabled {
        if definitions.contains(&feature) {
            continue;
        }
        diagnostics.push(LintDiagnostic::at(
            &LINT_RULES[RULE_PACKAGECONFIG],
            tree.source(),
            TextRange::new(offset, offset + length),
            format!("PACKAGECONFIG feature '{feature}' has no PACKAGECONFIG[{feature}] definition"),
        ));
    }
}

fn check_package_scope(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    let mut packages = BTreeSet::new();
    let mut package_assignment = None;
    let mut seen = HashSet::new();
    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        if assignment.name() != "PACKAGES" {
            continue;
        }
        package_assignment = Some(assignment);
        let Some((value, value_offset)) = simple_quoted_value(assignment.value()) else {
            continue;
        };
        for (relative_offset, package) in static_words(value) {
            if !seen.insert(package) {
                let offset = assignment.value_range().start() + value_offset + relative_offset;
                diagnostics.push(LintDiagnostic::at(
                    &LINT_RULES[RULE_PACKAGE_LIST],
                    tree.source(),
                    TextRange::new(offset, offset + package.len()),
                    format!("package '{package}' is listed more than once in PACKAGES"),
                ));
            }
            packages.insert(package.to_owned());
        }
    }
    if package_assignment.is_none() || packages.is_empty() {
        return;
    }

    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        let Some(package) = package_scope(assignment.name()) else {
            continue;
        };
        if package.contains('$') || packages.contains(package) {
            continue;
        }
        diagnostics.push(LintDiagnostic::at(
            &LINT_RULES[RULE_PACKAGE_SCOPE],
            tree.source(),
            assignment.name_range(),
            format!(
                "{} is scoped to undeclared package '{package}'",
                assignment.name()
            ),
        ));
    }
}

fn check_uri_parameters(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        if !is_src_uri_name(assignment.name()) {
            continue;
        }
        let Some((value, value_offset)) = simple_quoted_value(assignment.value()) else {
            continue;
        };
        for (relative_offset, uri) in static_words(value) {
            let mut parameters = HashSet::new();
            let is_git = uri.starts_with("git://");
            for parameter in uri.split(';').skip(1) {
                let Some((key, parameter_value)) = parameter.split_once('=') else {
                    continue;
                };
                let invalid = key.is_empty()
                    || !parameters.insert(key)
                    || parameter_value.is_empty()
                    || (key == "branch" && !is_git)
                    || (key == "protocol" && !is_git)
                    || (key == "protocol"
                        && !matches!(parameter_value, "git" | "http" | "https" | "ssh" | "file"));
                if !invalid {
                    continue;
                }
                let offset = assignment.value_range().start()
                    + value_offset
                    + relative_offset
                    + uri.find(parameter).unwrap_or(0);
                diagnostics.push(LintDiagnostic::at(
                    &LINT_RULES[RULE_URI_PARAMETERS],
                    tree.source(),
                    TextRange::new(offset, offset + parameter.len()),
                    format!(
                        "SRC_URI entry '{uri}' has invalid or conflicting parameter '{parameter}'"
                    ),
                ));
            }
        }
    }
}

fn check_layer_qa(
    tree: &SyntaxTree<'_>,
    workspace: &WorkspaceIndex,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let source = tree.source();
    let mut collections = BTreeMap::new();
    let mut collection_count = 0;
    let mut has_dynamic_collections = false;
    for node in tree.nodes() {
        let SyntaxKind::Assignment(assignment) = node.kind() else {
            continue;
        };
        if assignment.name() != "BBFILE_COLLECTIONS" {
            continue;
        }
        let Some((value, value_offset)) = simple_quoted_value(assignment.value()) else {
            continue;
        };
        has_dynamic_collections |= value.contains('$') || value.contains('{');
        for (relative_offset, collection) in static_words(value) {
            collection_count += 1;
            let offset = assignment.value_range().start() + value_offset + relative_offset;
            if collections
                .insert(
                    collection.to_owned(),
                    TextRange::new(offset, offset + collection.len()),
                )
                .is_some()
            {
                diagnostics.push(LintDiagnostic::at(
                    &LINT_RULES[RULE_LAYER_COLLECTIONS],
                    source,
                    TextRange::new(offset, offset + collection.len()),
                    format!("layer collection '{collection}' is declared more than once"),
                ));
            }
        }
    }
    if collection_count == 0 {
        if has_dynamic_collections {
            return;
        }
        diagnostics.push(LintDiagnostic::at(
            &LINT_RULES[RULE_LAYER_COLLECTIONS],
            source,
            TextRange::new(source.len(), source.len()),
            "layer is missing a static BBFILE_COLLECTIONS declaration",
        ));
        return;
    }

    let indexed_collections = workspace.collection_names();
    for collection in collections.keys() {
        let pattern_name = format!("BBFILE_PATTERN_{collection}");
        match find_assignment(tree, &pattern_name) {
            Some(assignment)
                if scalar_value(assignment.value()).is_some_and(|value| !value.is_empty()) => {}
            Some(assignment) => diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_LAYER_PATTERN],
                source,
                assignment.name_range(),
                format!("{pattern_name} must not be empty"),
            )),
            None => diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_LAYER_PATTERN],
                source,
                TextRange::new(source.len(), source.len()),
                format!("layer collection '{collection}' is missing {pattern_name}"),
            )),
        }

        let priority_name = format!("BBFILE_PRIORITY_{collection}");
        match find_assignment(tree, &priority_name) {
            Some(assignment)
                if scalar_value(assignment.value()).is_some_and(|value| value.contains('$')) => {}
            Some(assignment)
                if scalar_value(assignment.value())
                    .and_then(|value| value.parse::<i32>().ok())
                    .is_some() => {}
            Some(assignment) => diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_LAYER_PRIORITY],
                source,
                assignment.name_range(),
                format!("{priority_name} must be an integer"),
            )),
            None => diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_LAYER_PRIORITY],
                source,
                TextRange::new(source.len(), source.len()),
                format!("layer collection '{collection}' is missing {priority_name}"),
            )),
        }

        let compat_name = format!("LAYERSERIES_COMPAT_{collection}");
        match find_assignment(tree, &compat_name) {
            Some(assignment)
                if scalar_value(assignment.value()).is_some_and(|value| !value.is_empty()) => {}
            Some(assignment) => diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_LAYER_SERIES_COMPAT],
                source,
                assignment.name_range(),
                format!("{compat_name} must not be empty"),
            )),
            None => diagnostics.push(LintDiagnostic::at(
                &LINT_RULES[RULE_LAYER_SERIES_COMPAT],
                source,
                TextRange::new(source.len(), source.len()),
                format!("layer collection '{collection}' is missing {compat_name}"),
            )),
        }

        let depends_name = format!("LAYERDEPENDS_{collection}");
        if let Some(assignment) = find_assignment(tree, &depends_name)
            && let Some((value, value_offset)) = simple_quoted_value(assignment.value())
        {
            for (relative_offset, dependency) in static_words(value) {
                if dependency.starts_with('(') || dependency.ends_with(')') {
                    continue;
                }
                if indexed_collections.contains(dependency) {
                    continue;
                }
                let offset = assignment.value_range().start() + value_offset + relative_offset;
                diagnostics.push(LintDiagnostic::at(
                    &LINT_RULES[RULE_LAYER_DEPENDS],
                    source,
                    TextRange::new(offset, offset + dependency.len()),
                    format!("{depends_name} references unknown layer collection '{dependency}'"),
                ));
            }
        }
    }
}

fn find_assignment<'tree, 'source>(
    tree: &'tree SyntaxTree<'source>,
    name: &str,
) -> Option<&'tree AssignmentSyntax<'source>> {
    tree.nodes().iter().find_map(|node| match node.kind() {
        SyntaxKind::Assignment(assignment) if assignment.name() == name => Some(assignment),
        _ => None,
    })
}

fn is_layer_configuration(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("layer.conf")
        && path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("conf")
}

fn is_src_uri_name(name: &str) -> bool {
    name == "SRC_URI" || name.starts_with("SRC_URI:") || name.starts_with("SRC_URI_")
}

fn is_packageconfig_value(name: &str) -> bool {
    name == "PACKAGECONFIG"
        || name.starts_with("PACKAGECONFIG:")
        || name.starts_with("PACKAGECONFIG_")
}

fn packageconfig_feature(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("PACKAGECONFIG[")?;
    let end = rest.find(']')?;
    let suffix = &rest[end + 1..];
    if suffix.is_empty() || suffix.starts_with(':') || suffix.starts_with('_') {
        Some(&rest[..end])
    } else {
        None
    }
}

fn package_scope(name: &str) -> Option<&str> {
    for base in [
        "FILES",
        "RDEPENDS",
        "RRECOMMENDS",
        "RPROVIDES",
        "RCONFLICTS",
        "RREPLACES",
    ] {
        if let Some(rest) = name.strip_prefix(&format!("{base}:")) {
            return rest.split(':').next();
        }
        if let Some(rest) = name.strip_prefix(&format!("{base}_")) {
            return rest.split('_').next();
        }
    }
    None
}

fn scalar_value(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.starts_with(['\'', '"']) {
        return simple_quoted_value(value).map(|(value, _)| value);
    }
    Some(value.split('#').next()?.trim())
}

fn has_valid_checksum(uri: &str, key: &str) -> bool {
    uri.split(';').skip(1).any(|parameter| {
        let Some(value) = parameter.strip_prefix(&format!("{key}=")) else {
            return false;
        };
        let expected_length = if key == "md5" || key == "md5sum" {
            32
        } else {
            64
        };
        value.len() == expected_length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn is_remote_archive(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("ftp://")
}

#[allow(clippy::too_many_arguments)]
fn check_class_reference(
    tree: &SyntaxTree<'_>,
    path: &std::path::Path,
    workspace: &WorkspaceIndex,
    contexts: &[WorkspaceClassContext],
    arguments_start: usize,
    relative_offset: usize,
    class: &str,
    kind: WorkspaceDependencyKind,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let resolutions = contexts
        .iter()
        .copied()
        .filter_map(|context| {
            let candidates = workspace.class_candidates_for(class, context);
            (!candidates.is_empty()).then_some((context, candidates))
        })
        .collect::<Vec<_>>();
    if resolutions.is_empty() {
        let rule = &LINT_RULES[6];
        let offset = arguments_start + relative_offset;
        diagnostics.push(LintDiagnostic::at(
            rule,
            tree.source(),
            TextRange::new(offset, offset + class.len()),
            format!("inherited class '{class}' was not found in indexed layers"),
        ));
        return;
    }

    let (context, candidates) = resolutions
        .iter()
        .find(|(_, candidates)| ambiguity_message(candidates).is_none())
        .unwrap_or(&resolutions[0]);
    if let Some(message) = ambiguity_message(candidates) {
        let rule = &LINT_RULES[8];
        let offset = arguments_start + relative_offset;
        diagnostics.push(LintDiagnostic::at(
            rule,
            tree.source(),
            TextRange::new(offset, offset + class.len()),
            format!("inherited class '{class}' {message}"),
        ));
    }
    if let Some(candidate) = candidates.first().copied() {
        check_dependency_cycle(
            tree,
            path,
            workspace,
            arguments_start,
            relative_offset,
            *context,
            kind,
            candidate,
            diagnostics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn check_dependency_cycle(
    tree: &SyntaxTree<'_>,
    from: &std::path::Path,
    workspace: &WorkspaceIndex,
    arguments_start: usize,
    relative_offset: usize,
    context: WorkspaceClassContext,
    kind: WorkspaceDependencyKind,
    candidate: WorkspaceCandidate<'_>,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let Some(cycle) = workspace.dependency_cycle_for(from, candidate.path(), context) else {
        return;
    };

    let rule = &LINT_RULES[9];
    let offset = arguments_start + relative_offset;
    let cycle = cycle
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(" -> ");
    diagnostics.push(LintDiagnostic::at(
        rule,
        tree.source(),
        word_range(tree.source(), offset),
        format!(
            "static {} dependency resolves to {} and forms a cycle: {cycle}",
            kind.keyword(),
            candidate_description(candidate)
        ),
    ));
}

fn ambiguity_message(candidates: &[WorkspaceCandidate<'_>]) -> Option<String> {
    let first = candidates.first()?;
    let priority = first.priority();
    let scope = first.scope();
    let highest_priority = candidates
        .iter()
        .filter(|candidate| candidate.priority() == priority && candidate.scope() == scope)
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
        "resolves to {}, but {} candidates share effective priority {priority} through {}: {paths}",
        candidate_description(*first),
        highest_priority.len(),
        first.scope().description(),
    ))
}

fn candidate_description(candidate: WorkspaceCandidate<'_>) -> String {
    let collection = candidate
        .collection()
        .map(|collection| format!(", collection '{collection}'"))
        .unwrap_or_default();
    format!(
        "'{}' through {} in layer '{}'{} at priority {}",
        candidate.path().display(),
        candidate.scope().description(),
        candidate.layer().display(),
        collection,
        candidate.priority(),
    )
}

fn word_range(source: &str, start: usize) -> TextRange {
    let end = source[start..]
        .find(char::is_whitespace)
        .map(|relative| start + relative)
        .unwrap_or(source.len());
    TextRange::new(start, end)
}

fn check_trailing_whitespace(text: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    let rule = &LINT_RULES[0];
    let mut line_start = 0;

    for line in text.split_inclusive('\n') {
        let (content, _) = split_line_ending(line);
        let trimmed = content.trim_end_matches([' ', '\t']);
        if trimmed.len() != content.len() {
            let range = TextRange::new(line_start + trimmed.len(), line_start + content.len());
            diagnostics.push(
                LintDiagnostic::at(rule, text, range, "line ends with whitespace").with_fix(
                    LintFix::new(range, "", "remove trailing whitespace"),
                    "Remove the trailing spaces or tabs from this line.",
                ),
            );
        }
        line_start += line.len();
    }
}

fn check_final_newline(text: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    if text.is_empty() || text.ends_with('\n') {
        return;
    }

    let rule = &LINT_RULES[1];
    let range = TextRange::new(text.len(), text.len());
    diagnostics.push(
        LintDiagnostic::at(rule, text, range, "file does not end with a newline").with_fix(
            LintFix::new(range, "\n", "append a final newline"),
            "Append a newline at the end of the file.",
        ),
    );
}

fn check_assignments(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    let mut direct_assignments = HashSet::new();
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
        check_file_paths(tree.source(), assignment, diagnostics);
        check_git_uri_protocol(tree.source(), assignment, diagnostics);
        if matches!(
            assignment.operator(),
            AssignmentOperator::Assign | AssignmentOperator::Immediate
        ) && !direct_assignments.insert(assignment.name())
        {
            let rule = &LINT_RULES[15];
            diagnostics.push(LintDiagnostic::at(
                rule,
                tree.source(),
                assignment.name_range(),
                format!(
                    "variable '{}' is assigned directly more than once",
                    assignment.name()
                ),
            ));
        }
    }
}

fn check_file_paths(
    source: &str,
    assignment: &AssignmentSyntax<'_>,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    if !assignment.name().starts_with("FILESEXTRAPATHS")
        || assignment.operator() == AssignmentOperator::Immediate
    {
        return;
    }

    let rule = &LINT_RULES[13];
    diagnostics.push(LintDiagnostic::at(
        rule,
        source,
        assignment.operator_range(),
        format!(
            "{} must use ':=' so path expansion happens before parsing",
            assignment.name()
        ),
    ));
}

fn check_git_uri_protocol(
    source: &str,
    assignment: &AssignmentSyntax<'_>,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    if !(assignment.name() == "SRC_URI"
        || assignment.name().starts_with("SRC_URI:")
        || assignment.name().starts_with("SRC_URI_"))
    {
        return;
    }
    let Some((value, value_offset)) = simple_quoted_value(assignment.value()) else {
        return;
    };

    let rule = &LINT_RULES[14];
    for (relative_offset, uri) in static_words(value) {
        if !uri.starts_with("git://") || uri.split(';').any(|part| part.starts_with("protocol=")) {
            continue;
        }
        let offset = assignment.value_range().start() + value_offset + relative_offset;
        diagnostics.push(LintDiagnostic::at(
            rule,
            source,
            TextRange::new(offset, offset + uri.len()),
            format!("Git URI '{uri}' does not declare a protocol"),
        ));
    }
}

fn check_duplicate_functions(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    let mut functions = HashSet::new();
    let rule = &LINT_RULES[16];
    for node in tree.nodes() {
        let SyntaxKind::Function(function) = node.kind() else {
            continue;
        };
        let Some(name) = function.name() else {
            continue;
        };
        if functions.insert(name) {
            continue;
        }
        let Some(range) = function.name_range() else {
            continue;
        };
        diagnostics.push(LintDiagnostic::at(
            rule,
            tree.source(),
            range,
            format!("function '{name}' is declared more than once"),
        ));
    }
}

fn check_empty_directives(tree: &SyntaxTree<'_>, diagnostics: &mut Vec<LintDiagnostic>) {
    let rule = &LINT_RULES[17];
    for node in tree.nodes() {
        let SyntaxKind::Directive(directive) = node.kind() else {
            continue;
        };
        if !matches!(
            directive.keyword(),
            DirectiveKeyword::Include
                | DirectiveKeyword::IncludeAll
                | DirectiveKeyword::Require
                | DirectiveKeyword::Inherit
                | DirectiveKeyword::InheritDefer
        ) {
            continue;
        }
        let arguments = directive.arguments();
        let code_end = comment_start(arguments).unwrap_or(arguments.len());
        if !arguments[..code_end].trim().is_empty() {
            continue;
        }
        diagnostics.push(LintDiagnostic::at(
            rule,
            tree.source(),
            directive.keyword_range(),
            format!("{} directive has no target", directive.keyword().lexeme()),
        ));
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
    let range = assignment.name_range();
    diagnostics.push(LintDiagnostic::at(
        rule,
        source,
        range,
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
    let offset = assignment.value_range().start() + relative_offset;
    diagnostics.push(LintDiagnostic::at(
        rule,
        source,
        TextRange::new(offset, offset + "${AUTOREV}".len()),
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
            diagnostics.push(LintDiagnostic::at(
                rule,
                tree.source(),
                TextRange::new(offset, offset + class.len()),
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
                "BBT009", "BBT010", "BBT011", "BBT012", "BBT013", "BBT014", "BBT015", "BBT016",
                "BBT017", "BBT018", "BBT019", "BBT020", "BBT021", "BBT022", "BBT023", "BBT024",
                "BBT025", "BBT026", "BBT027", "BBT028", "BBT029", "BBT030", "BBT031", "BBT032",
                "BBT033", "BBT034", "BBT035", "BBT036",
            ]
        );
        assert!(
            lint_rules()[..18]
                .iter()
                .all(|rule| rule.severity() == LintSeverity::Warning)
        );
        assert_eq!(lint_rules()[18].severity(), LintSeverity::Error);
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

        let autorev = diagnostics
            .iter()
            .filter(|item| item.rule_id() == "BBT004")
            .collect::<Vec<_>>();
        assert_eq!(autorev.len(), 2);
        assert_eq!(autorev[0].line(), 1);
        assert_eq!(autorev[1].line(), 2);
        assert!(
            diagnostics
                .iter()
                .any(|item| item.rule_id() == "BBT016" && item.line() == 5)
        );
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

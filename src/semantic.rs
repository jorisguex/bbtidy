//! Authoritative BitBake semantic analysis.
//!
//! The syntax tree and workspace index in this crate are intentionally
//! dependency-free and conservative.  They cannot, however, reproduce
//! BitBake's Python expansion, override selection, anonymous Python, class
//! handlers, machine/distro configuration, or recipe collection rules.  This
//! module bridges that gap by executing the BitBake engine in an existing
//! build directory and exposing its resolved results through a typed API.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Options used for an authoritative BitBake analysis.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticOptions {
    /// BitBake executable to invoke.  The default is the executable found on
    /// `PATH`.
    pub bitbake: PathBuf,
    /// Existing BitBake build directory containing `conf/local.conf` and
    /// `conf/bblayers.conf`.
    pub build_dir: PathBuf,
    /// Recipes or targets whose fully expanded environments should be
    /// collected.  An empty list still performs a complete parse-only check.
    pub targets: Vec<String>,
    /// Variables to extract from each target's `bitbake -e` output.  An empty
    /// list keeps the full environment available through [`SemanticEnvironment::raw`]
    /// without materializing a huge default variable map.
    pub variables: Vec<String>,
}

impl Default for SemanticOptions {
    fn default() -> Self {
        Self {
            bitbake: PathBuf::from("bitbake"),
            build_dir: PathBuf::new(),
            targets: Vec::new(),
            variables: Vec::new(),
        }
    }
}

impl SemanticOptions {
    /// Creates options for a build directory using the `bitbake` on `PATH`.
    pub fn for_build_dir(build_dir: impl Into<PathBuf>) -> Self {
        Self {
            build_dir: build_dir.into(),
            ..Self::default()
        }
    }

    /// Creates options for a validated discovered build context using the
    /// `bitbake` executable on `PATH`.
    pub fn for_context(context: &crate::BuildContext) -> Self {
        Self::for_build_dir(context.build_dir())
    }

    /// Adds one target whose resolved environment should be inspected.
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.targets.push(target.into());
        self
    }

    /// Adds one variable to extract from every target environment.
    pub fn variable(mut self, variable: impl Into<String>) -> Self {
        self.variables.push(variable.into());
        self
    }
}

/// Severity of a diagnostic emitted by BitBake during semantic analysis.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SemanticSeverity {
    Debug,
    Note,
    Warning,
    Error,
}

impl fmt::Display for SemanticSeverity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Debug => "debug",
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        })
    }
}

/// The BitBake phase that produced a semantic diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticDiagnosticPhase {
    Parse,
    TargetQuery,
}

/// The stream from which BitBake emitted a semantic diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SemanticDiagnosticStream {
    Stdout,
    Stderr,
}

/// A source-aware diagnostic parsed from BitBake's standard output or error.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticDiagnostic {
    phase: SemanticDiagnosticPhase,
    target: Option<String>,
    stream: SemanticDiagnosticStream,
    severity: SemanticSeverity,
    path: Option<PathBuf>,
    line: Option<usize>,
    column: Option<usize>,
    message: String,
    raw: String,
}

impl SemanticDiagnostic {
    pub const fn phase(&self) -> SemanticDiagnosticPhase {
        self.phase
    }

    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    pub const fn stream(&self) -> SemanticDiagnosticStream {
        self.stream
    }

    pub fn severity(&self) -> SemanticSeverity {
        self.severity
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub const fn line(&self) -> Option<usize> {
        self.line
    }

    pub const fn column(&self) -> Option<usize> {
        self.column
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }
}

/// A fully expanded recipe environment returned by `bitbake -e`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticEnvironment {
    target: String,
    variables: BTreeMap<String, String>,
    #[serde(skip_serializing)]
    raw: String,
}

impl SemanticEnvironment {
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Returns a resolved value when the variable was selected in
    /// [`SemanticOptions::variables`]. Values are unquoted and common BitBake
    /// escape sequences are decoded, while function bodies and unknown forms
    /// remain available through [`Self::raw`].
    pub fn get(&self, variable: &str) -> Option<&str> {
        self.variables.get(variable).map(String::as_str)
    }

    pub fn variables(&self) -> &BTreeMap<String, String> {
        &self.variables
    }

    /// Returns the exact environment dump emitted by BitBake.
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

/// The outcome of querying one requested BitBake target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticTargetResult {
    target: String,
    succeeded: bool,
    queried: bool,
    diagnostics: Vec<SemanticDiagnostic>,
    environment: Option<SemanticEnvironment>,
}

impl SemanticTargetResult {
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Returns whether BitBake was invoked for this target.
    pub const fn queried(&self) -> bool {
        self.queried
    }

    /// Returns whether the target environment query succeeded.
    pub const fn succeeded(&self) -> bool {
        self.succeeded
    }

    pub fn diagnostics(&self) -> &[SemanticDiagnostic] {
        &self.diagnostics
    }

    pub fn environment(&self) -> Option<&SemanticEnvironment> {
        self.environment.as_ref()
    }
}

/// The result of a complete BitBake parse and optional environment queries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticReport {
    bitbake: PathBuf,
    bitbake_version: String,
    build_dir: PathBuf,
    requested_targets: Vec<String>,
    requested_variables: Vec<String>,
    parse_succeeded: bool,
    target_queries_succeeded: bool,
    diagnostics: Vec<SemanticDiagnostic>,
    environments: Vec<SemanticEnvironment>,
    target_results: Vec<SemanticTargetResult>,
}

impl SemanticReport {
    pub fn bitbake(&self) -> &Path {
        &self.bitbake
    }

    pub fn bitbake_version(&self) -> &str {
        &self.bitbake_version
    }

    pub fn build_dir(&self) -> &Path {
        &self.build_dir
    }

    pub fn requested_targets(&self) -> &[String] {
        &self.requested_targets
    }

    pub fn requested_variables(&self) -> &[String] {
        &self.requested_variables
    }

    pub const fn parse_succeeded(&self) -> bool {
        self.parse_succeeded
    }

    pub const fn target_queries_succeeded(&self) -> bool {
        self.target_queries_succeeded
    }

    pub const fn analysis_succeeded(&self) -> bool {
        self.parse_succeeded && self.target_queries_succeeded
    }

    pub fn diagnostics(&self) -> &[SemanticDiagnostic] {
        &self.diagnostics
    }

    pub fn environments(&self) -> &[SemanticEnvironment] {
        &self.environments
    }

    pub fn target_results(&self) -> &[SemanticTargetResult] {
        &self.target_results
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == SemanticSeverity::Error)
    }
}

/// Operational failures while invoking or inspecting BitBake.
#[derive(Debug)]
pub enum SemanticError {
    InvalidBuildDirectory { path: PathBuf, reason: String },
    Io(io::Error),
    BitBakeVersion(String),
}

impl fmt::Display for SemanticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBuildDirectory { path, reason } => {
                write!(
                    formatter,
                    "invalid BitBake build directory {}: {reason}",
                    path.display()
                )
            }
            Self::Io(error) => write!(formatter, "could not invoke BitBake: {error}"),
            Self::BitBakeVersion(output) => {
                write!(formatter, "BitBake did not report a version: {output}")
            }
        }
    }
}

impl std::error::Error for SemanticError {}

impl From<io::Error> for SemanticError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Runs BitBake's parser and, for requested targets, captures its fully
/// expanded recipe environments.
///
/// This is deliberately a subprocess boundary.  It means semantics are
/// supplied by the exact BitBake/Yocto installation selected by the caller,
/// including Python metadata, overrides, anonymous functions, layer
/// priorities, machine/distro configuration, and external providers.
pub fn analyze_bitbake(options: &SemanticOptions) -> Result<SemanticReport, SemanticError> {
    validate_build_dir(&options.build_dir)?;

    let version_output = run_bitbake(options, &["--version"])?;
    let bitbake_version = version_output
        .status
        .success()
        .then(|| parse_version(&version_output))
        .flatten()
        .ok_or_else(|| SemanticError::BitBakeVersion(combined_output(&version_output)))?;

    let mut diagnostics = Vec::new();
    let parse_args = parse_arguments(&options.targets);
    let parse_output = run_bitbake(options, &parse_args)?;
    diagnostics.extend(parse_diagnostics(
        &parse_output,
        SemanticDiagnosticPhase::Parse,
        None,
    ));

    let parse_succeeded = parse_output.status.success();
    let mut target_queries_succeeded = true;
    let mut environments = Vec::new();
    let mut target_results = Vec::new();
    if parse_succeeded {
        for target in &options.targets {
            let environment_output = run_bitbake(options, &["--environment", target])?;
            let target_diagnostics = parse_diagnostics(
                &environment_output,
                SemanticDiagnosticPhase::TargetQuery,
                Some(target),
            );
            diagnostics.extend(target_diagnostics.clone());
            if environment_output.status.success() {
                let raw = String::from_utf8_lossy(&environment_output.stdout).into_owned();
                let environment = parse_environment(target, &raw, &options.variables);
                environments.push(environment.clone());
                target_results.push(SemanticTargetResult {
                    target: target.clone(),
                    succeeded: true,
                    queried: true,
                    diagnostics: target_diagnostics,
                    environment: Some(environment),
                });
            } else {
                target_queries_succeeded = false;
                target_results.push(SemanticTargetResult {
                    target: target.clone(),
                    succeeded: false,
                    queried: true,
                    diagnostics: target_diagnostics,
                    environment: None,
                });
            }
        }
    } else if !options.targets.is_empty() {
        target_queries_succeeded = false;
        target_results.extend(options.targets.iter().map(|target| SemanticTargetResult {
            target: target.clone(),
            succeeded: false,
            queried: false,
            diagnostics: Vec::new(),
            environment: None,
        }));
    }

    Ok(SemanticReport {
        bitbake: options.bitbake.clone(),
        bitbake_version,
        build_dir: options.build_dir.clone(),
        requested_targets: options.targets.clone(),
        requested_variables: options.variables.clone(),
        parse_succeeded,
        target_queries_succeeded,
        diagnostics,
        environments,
        target_results,
    })
}

fn validate_build_dir(build_dir: &Path) -> Result<(), SemanticError> {
    if build_dir.as_os_str().is_empty() {
        return Err(SemanticError::InvalidBuildDirectory {
            path: build_dir.to_path_buf(),
            reason: "a build directory is required".to_owned(),
        });
    }
    let metadata =
        fs::metadata(build_dir).map_err(|error| SemanticError::InvalidBuildDirectory {
            path: build_dir.to_path_buf(),
            reason: error.to_string(),
        })?;
    if !metadata.is_dir() {
        return Err(SemanticError::InvalidBuildDirectory {
            path: build_dir.to_path_buf(),
            reason: "path is not a directory".to_owned(),
        });
    }
    for filename in ["conf/local.conf", "conf/bblayers.conf"] {
        if !build_dir.join(filename).is_file() {
            return Err(SemanticError::InvalidBuildDirectory {
                path: build_dir.to_path_buf(),
                reason: format!("missing {filename}"),
            });
        }
    }
    Ok(())
}

fn parse_arguments(targets: &[String]) -> Vec<&str> {
    let mut arguments = vec!["--parse-only"];
    arguments.extend(targets.iter().map(String::as_str));
    arguments
}

fn run_bitbake(options: &SemanticOptions, arguments: &[&str]) -> Result<Output, SemanticError> {
    Command::new(&options.bitbake)
        .current_dir(&options.build_dir)
        .args(arguments)
        .output()
        .map_err(SemanticError::Io)
}

fn combined_output(output: &Output) -> String {
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    combined
}

fn parse_version(output: &Output) -> Option<String> {
    let text = combined_output(output);
    text.lines()
        .find(|line| line.contains("BitBake") && line.to_ascii_lowercase().contains("version"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

fn parse_diagnostics(
    output: &Output,
    phase: SemanticDiagnosticPhase,
    target: Option<&str>,
) -> Vec<SemanticDiagnostic> {
    [
        (SemanticDiagnosticStream::Stdout, output.stdout.as_slice()),
        (SemanticDiagnosticStream::Stderr, output.stderr.as_slice()),
    ]
    .into_iter()
    .flat_map(|(stream, bytes)| {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter_map(move |line| parse_diagnostic_line(line, phase, target, stream))
            .collect::<Vec<_>>()
    })
    .collect()
}

fn parse_diagnostic_line(
    line: &str,
    phase: SemanticDiagnosticPhase,
    target: Option<&str>,
    stream: SemanticDiagnosticStream,
) -> Option<SemanticDiagnostic> {
    let line = line.trim_end_matches('\r');
    let (severity, remainder) = if let Some(value) = line.strip_prefix("DEBUG:") {
        (SemanticSeverity::Debug, value.trim_start())
    } else if let Some(value) = line.strip_prefix("NOTE:") {
        (SemanticSeverity::Note, value.trim_start())
    } else if let Some(value) = line.strip_prefix("WARNING:") {
        (SemanticSeverity::Warning, value.trim_start())
    } else if let Some(value) = line.strip_prefix("ERROR:") {
        (SemanticSeverity::Error, value.trim_start())
    } else {
        return None;
    };

    let (path, line_number, column, message) = parse_source_location(remainder);
    Some(SemanticDiagnostic {
        phase,
        target: target.map(str::to_owned),
        stream,
        severity,
        path,
        line: line_number,
        column,
        message,
        raw: line.to_owned(),
    })
}

fn parse_source_location(text: &str) -> (Option<PathBuf>, Option<usize>, Option<usize>, String) {
    let mut location = None;
    let bytes = text.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b':' {
            continue;
        }
        let mut end = index + 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == index + 1 || bytes.get(end) != Some(&b':') {
            continue;
        }
        let line_number = std::str::from_utf8(&bytes[index + 1..end])
            .ok()
            .and_then(|value| value.parse().ok());
        let mut message_start = end;
        let mut column = None;
        if bytes.get(message_start) == Some(&b':') {
            let mut column_start = message_start + 1;
            while column_start < bytes.len() && bytes[column_start].is_ascii_whitespace() {
                column_start += 1;
            }
            let mut column_end = column_start;
            while column_end < bytes.len() && bytes[column_end].is_ascii_digit() {
                column_end += 1;
            }
            if column_end > column_start && bytes.get(column_end) == Some(&b':') {
                column = std::str::from_utf8(&bytes[column_start..column_end])
                    .ok()
                    .and_then(|value| value.parse().ok());
                message_start = column_end + 1;
            } else {
                message_start = column_start;
            }
        }
        while message_start < bytes.len() && bytes[message_start].is_ascii_whitespace() {
            message_start += 1;
        }
        let before = text[..index].trim();
        let path_text = before
            .rsplit_once(" at ")
            .map(|(_, value)| value.trim())
            .unwrap_or(before);
        if !path_text.is_empty() && looks_like_path(path_text) {
            location = Some((
                PathBuf::from(path_text),
                line_number,
                column,
                text[message_start..].trim().to_owned(),
            ));
            break;
        }
    }

    if let Some((path, line_number, column, message)) = location {
        (Some(path), line_number, column, message)
    } else {
        (None, None, None, text.trim().to_owned())
    }
}

fn looks_like_path(value: &str) -> bool {
    value.ends_with(".bb")
        || value.ends_with(".bbappend")
        || value.ends_with(".bbclass")
        || value.ends_with(".inc")
        || value.ends_with(".conf")
        || value.contains('/')
        || value.contains('\\')
}

fn parse_environment(
    target: &str,
    raw: &str,
    requested_variables: &[String],
) -> SemanticEnvironment {
    let mut variables = BTreeMap::new();
    let lines = raw.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let Some((name, first_value)) = parse_environment_assignment(lines[index]) else {
            index += 1;
            continue;
        };

        let mut encoded_value = first_value.to_owned();
        let mut next = index + 1;
        while has_unterminated_double_quote(&encoded_value) && next < lines.len() {
            encoded_value.push('\n');
            encoded_value.push_str(lines[next]);
            next += 1;
        }
        if requested_variables
            .iter()
            .any(|requested| requested == name)
        {
            variables.insert(name.to_owned(), decode_environment_value(&encoded_value));
        }
        index = next.max(index + 1);
    }
    SemanticEnvironment {
        target: target.to_owned(),
        variables,
        raw: raw.to_owned(),
    }
}

fn parse_environment_assignment(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('#') || line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let equals = line.find('=')?;
    let name = line[..equals].trim();
    let name = name.strip_prefix("export ").unwrap_or(name);
    if name.is_empty() || !is_environment_name(name) {
        return None;
    }
    let value = line[equals + 1..].trim();
    Some((name, value))
}

fn has_unterminated_double_quote(value: &str) -> bool {
    if !value.starts_with('"') {
        return false;
    }

    let mut escaped = false;
    for character in value[1..].chars() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return false;
        }
    }
    true
}

fn is_environment_name(name: &str) -> bool {
    name.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'-' | b'+' | b'.' | b':' | b'[' | b']')
    })
}

fn decode_environment_value(value: &str) -> String {
    let Some(quoted) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return value.to_owned();
    };
    let mut decoded = String::with_capacity(quoted.len());
    let mut escaped = false;
    for character in quoted.chars() {
        if escaped {
            if character != '\n' {
                decoded.push(match character {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            decoded.push(character);
        }
    }
    if escaped {
        decoded.push('\\');
    }
    decoded
}

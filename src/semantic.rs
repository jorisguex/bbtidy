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

/// Optional BitBake analyses to run after the parser check.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SemanticAnalysisOptions {
    /// Collect BitBake's task, recipe, and package dependency graph files.
    pub dependency_graph: bool,
    /// Run BitBake's dry-run scheduler and retain the planned tasks.
    pub dry_run: bool,
    /// Collect the parsed recipe/version inventory from `bitbake --show-versions`.
    pub inventory: bool,
    /// Summarize resolved package, provider, runtime dependency, and image metadata.
    pub packages: bool,
}

impl SemanticAnalysisOptions {
    /// Enables every analysis supported by the semantic report.
    pub const fn full() -> Self {
        Self {
            dependency_graph: true,
            dry_run: true,
            inventory: true,
            packages: true,
        }
    }

    pub const fn requested(&self) -> bool {
        self.dependency_graph || self.dry_run || self.inventory || self.packages
    }
}

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
    /// Additional BitBake-backed analyses to include in the report.
    pub analysis: SemanticAnalysisOptions,
}

impl Default for SemanticOptions {
    fn default() -> Self {
        Self {
            bitbake: PathBuf::from("bitbake"),
            build_dir: PathBuf::new(),
            targets: Vec::new(),
            variables: Vec::new(),
            analysis: SemanticAnalysisOptions::default(),
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

    /// Enables the complete BitBake build analysis surface.
    pub fn full_analysis(mut self) -> Self {
        self.analysis = SemanticAnalysisOptions::full();
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
    DependencyGraph,
    DryRun,
    Inventory,
    PackageSummary,
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

/// One directed edge emitted by a BitBake dependency graph.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticGraphEdge {
    from: String,
    to: String,
}

impl SemanticGraphEdge {
    pub fn from(&self) -> &str {
        &self.from
    }

    pub fn to(&self) -> &str {
        &self.to
    }
}

/// A provider exposed by the parsed BitBake metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticProvider {
    name: String,
    recipe: String,
}

impl SemanticProvider {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn recipe(&self) -> &str {
        &self.recipe
    }
}

/// Dependency graph artifacts produced by `bitbake --graphviz` for one target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticDependencyGraph {
    target: String,
    succeeded: bool,
    diagnostics: Vec<SemanticDiagnostic>,
    task_edges: Vec<SemanticGraphEdge>,
    recipe_edges: Vec<SemanticGraphEdge>,
    package_edges: Vec<SemanticGraphEdge>,
    build_list: Vec<String>,
    providers: Vec<SemanticProvider>,
    artifacts: BTreeMap<String, String>,
}

impl SemanticDependencyGraph {
    pub fn target(&self) -> &str {
        &self.target
    }

    pub const fn succeeded(&self) -> bool {
        self.succeeded
    }

    pub fn diagnostics(&self) -> &[SemanticDiagnostic] {
        &self.diagnostics
    }

    pub fn task_edges(&self) -> &[SemanticGraphEdge] {
        &self.task_edges
    }

    pub fn recipe_edges(&self) -> &[SemanticGraphEdge] {
        &self.recipe_edges
    }

    pub fn package_edges(&self) -> &[SemanticGraphEdge] {
        &self.package_edges
    }

    pub fn build_list(&self) -> &[String] {
        &self.build_list
    }

    pub fn providers(&self) -> &[SemanticProvider] {
        &self.providers
    }

    pub fn artifacts(&self) -> &BTreeMap<String, String> {
        &self.artifacts
    }
}

/// The scheduler output of a BitBake dry run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticDryRun {
    targets: Vec<String>,
    succeeded: bool,
    diagnostics: Vec<SemanticDiagnostic>,
    tasks: Vec<String>,
    stdout: String,
    stderr: String,
}

impl SemanticDryRun {
    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    pub const fn succeeded(&self) -> bool {
        self.succeeded
    }

    pub fn diagnostics(&self) -> &[SemanticDiagnostic] {
        &self.diagnostics
    }

    pub fn tasks(&self) -> &[String] {
        &self.tasks
    }

    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

/// One recipe/version row from `bitbake --show-versions`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticRecipeVersion {
    recipe: String,
    version: String,
    revision: Option<String>,
}

impl SemanticRecipeVersion {
    pub fn recipe(&self) -> &str {
        &self.recipe
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }
}

/// Parsed recipe/provider inventory for the active BitBake context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticRecipeInventory {
    succeeded: bool,
    diagnostics: Vec<SemanticDiagnostic>,
    recipes: Vec<SemanticRecipeVersion>,
    providers: Vec<SemanticProvider>,
    raw: String,
}

impl SemanticRecipeInventory {
    pub const fn succeeded(&self) -> bool {
        self.succeeded
    }

    pub fn diagnostics(&self) -> &[SemanticDiagnostic] {
        &self.diagnostics
    }

    pub fn recipes(&self) -> &[SemanticRecipeVersion] {
        &self.recipes
    }

    pub fn providers(&self) -> &[SemanticProvider] {
        &self.providers
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }
}

/// Resolved package and image metadata for one target environment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticPackageSummary {
    target: String,
    succeeded: bool,
    diagnostics: Vec<SemanticDiagnostic>,
    packages: Vec<String>,
    provides: Vec<String>,
    build_dependencies: Vec<String>,
    image_install: Vec<String>,
    image_fstypes: Vec<String>,
    runtime_dependencies: BTreeMap<String, Vec<String>>,
    runtime_recommends: BTreeMap<String, Vec<String>>,
    runtime_provides: BTreeMap<String, Vec<String>>,
}

impl SemanticPackageSummary {
    pub fn target(&self) -> &str {
        &self.target
    }

    pub const fn succeeded(&self) -> bool {
        self.succeeded
    }

    pub fn diagnostics(&self) -> &[SemanticDiagnostic] {
        &self.diagnostics
    }

    pub fn packages(&self) -> &[String] {
        &self.packages
    }

    pub fn provides(&self) -> &[String] {
        &self.provides
    }

    pub fn build_dependencies(&self) -> &[String] {
        &self.build_dependencies
    }

    pub fn image_install(&self) -> &[String] {
        &self.image_install
    }

    pub fn image_fstypes(&self) -> &[String] {
        &self.image_fstypes
    }

    pub fn runtime_dependencies(&self) -> &BTreeMap<String, Vec<String>> {
        &self.runtime_dependencies
    }

    pub fn runtime_recommends(&self) -> &BTreeMap<String, Vec<String>> {
        &self.runtime_recommends
    }

    pub fn runtime_provides(&self) -> &BTreeMap<String, Vec<String>> {
        &self.runtime_provides
    }
}

/// All requested build-analysis sections in one semantic report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticBuildAnalysis {
    succeeded: bool,
    graphs: Vec<SemanticDependencyGraph>,
    dry_run: Option<SemanticDryRun>,
    inventory: Option<SemanticRecipeInventory>,
    packages: Vec<SemanticPackageSummary>,
}

impl SemanticBuildAnalysis {
    pub const fn succeeded(&self) -> bool {
        self.succeeded
    }

    pub fn graphs(&self) -> &[SemanticDependencyGraph] {
        &self.graphs
    }

    pub fn dry_run(&self) -> Option<&SemanticDryRun> {
        self.dry_run.as_ref()
    }

    pub fn inventory(&self) -> Option<&SemanticRecipeInventory> {
        self.inventory.as_ref()
    }

    pub fn packages(&self) -> &[SemanticPackageSummary] {
        &self.packages
    }
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
    build_analysis: Option<SemanticBuildAnalysis>,
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

    pub fn analysis_succeeded(&self) -> bool {
        self.parse_succeeded
            && self.target_queries_succeeded
            && match &self.build_analysis {
                Some(analysis) => analysis.succeeded,
                None => true,
            }
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

    pub fn build_analysis(&self) -> Option<&SemanticBuildAnalysis> {
        self.build_analysis.as_ref()
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
    AnalysisTargetsRequired { mode: String },
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
            Self::AnalysisTargetsRequired { mode } => {
                write!(
                    formatter,
                    "BitBake {mode} analysis requires at least one target"
                )
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
    if options.analysis.dependency_graph && options.targets.is_empty() {
        return Err(SemanticError::AnalysisTargetsRequired {
            mode: "dependency graph".to_owned(),
        });
    }
    if options.analysis.dry_run && options.targets.is_empty() {
        return Err(SemanticError::AnalysisTargetsRequired {
            mode: "dry-run".to_owned(),
        });
    }
    if options.analysis.packages && options.targets.is_empty() {
        return Err(SemanticError::AnalysisTargetsRequired {
            mode: "package".to_owned(),
        });
    }

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

    let build_analysis = if options.analysis.requested() {
        if parse_succeeded {
            let (analysis, analysis_diagnostics) =
                run_build_analysis(options, &target_results, &environments)?;
            diagnostics.extend(analysis_diagnostics);
            Some(analysis)
        } else {
            Some(SemanticBuildAnalysis {
                succeeded: false,
                graphs: Vec::new(),
                dry_run: None,
                inventory: None,
                packages: Vec::new(),
            })
        }
    } else {
        None
    };

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
        build_analysis,
    })
}

fn run_build_analysis(
    options: &SemanticOptions,
    target_results: &[SemanticTargetResult],
    _environments: &[SemanticEnvironment],
) -> Result<(SemanticBuildAnalysis, Vec<SemanticDiagnostic>), SemanticError> {
    let mut diagnostics = Vec::new();
    let mut succeeded = true;

    let graphs = if options.analysis.dependency_graph {
        let mut graphs = Vec::new();
        for target in &options.targets {
            let graph = run_dependency_graph(options, target)?;
            succeeded &= graph.succeeded;
            diagnostics.extend(graph.diagnostics.clone());
            graphs.push(graph);
        }
        graphs
    } else {
        Vec::new()
    };

    let dry_run = if options.analysis.dry_run {
        let dry_run = run_dry_run(options)?;
        succeeded &= dry_run.succeeded;
        diagnostics.extend(dry_run.diagnostics.clone());
        Some(dry_run)
    } else {
        None
    };

    let mut inventory = if options.analysis.inventory {
        let inventory = run_recipe_inventory(options)?;
        succeeded &= inventory.succeeded;
        diagnostics.extend(inventory.diagnostics.clone());
        Some(inventory)
    } else {
        None
    };

    let packages = if options.analysis.packages {
        let mut packages = Vec::new();
        for result in target_results {
            let summary = match result.environment() {
                Some(environment) => package_summary_from_environment(environment),
                None => failed_package_summary(result.target()),
            };
            succeeded &= summary.succeeded;
            diagnostics.extend(summary.diagnostics.clone());
            packages.push(summary);
        }
        packages
    } else {
        Vec::new()
    };

    if let Some(inventory) = inventory.as_mut() {
        for graph in &graphs {
            inventory.providers.extend(graph.providers.iter().cloned());
        }
        inventory.providers.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.recipe.cmp(&right.recipe))
        });
        inventory
            .providers
            .dedup_by(|left, right| left.name == right.name && left.recipe == right.recipe);
    }

    Ok((
        SemanticBuildAnalysis {
            succeeded,
            graphs,
            dry_run,
            inventory,
            packages,
        },
        diagnostics,
    ))
}

fn run_dependency_graph(
    options: &SemanticOptions,
    target: &str,
) -> Result<SemanticDependencyGraph, SemanticError> {
    let filenames = [
        "task-depends.dot",
        "pn-depends.dot",
        "package-depends.dot",
        "pn-buildlist",
        "pn-provides",
    ];
    let previous_artifacts = filenames
        .iter()
        .map(|filename| {
            let path = options.build_dir.join(filename);
            let contents = if path.is_file() {
                Some(fs::read_to_string(&path)?)
            } else {
                None
            };
            Ok::<_, SemanticError>((*filename, contents))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let arguments = ["--graphviz", target];
    let output = match run_bitbake(options, &arguments) {
        Ok(output) => output,
        Err(error) => {
            restore_graph_artifacts(&options.build_dir, &previous_artifacts)?;
            return Err(error);
        }
    };
    let mut diagnostics = parse_diagnostics(
        &output,
        SemanticDiagnosticPhase::DependencyGraph,
        Some(target),
    );
    let artifact_result = filenames
        .iter()
        .filter_map(|filename| {
            let path = options.build_dir.join(filename);
            if path.is_file() {
                Some(
                    fs::read_to_string(path)
                        .map(|contents| ((*filename).to_owned(), contents))
                        .map_err(SemanticError::Io),
                )
            } else {
                None
            }
        })
        .collect::<Result<BTreeMap<_, _>, _>>();
    restore_graph_artifacts(&options.build_dir, &previous_artifacts)?;
    let artifacts = artifact_result?;

    let mut succeeded = output.status.success();
    if !succeeded && diagnostics.is_empty() {
        diagnostics.push(analysis_diagnostic(
            SemanticDiagnosticPhase::DependencyGraph,
            Some(target),
            "BitBake dependency graph command failed",
        ));
    }
    if succeeded && artifacts.is_empty() {
        succeeded = false;
        diagnostics.push(analysis_diagnostic(
            SemanticDiagnosticPhase::DependencyGraph,
            Some(target),
            "BitBake did not emit dependency graph artifacts",
        ));
    }

    let task_edges = artifacts
        .get("task-depends.dot")
        .map(|value| parse_dot_edges(value))
        .unwrap_or_default();
    let recipe_edges = artifacts
        .get("pn-depends.dot")
        .map(|value| parse_dot_edges(value))
        .unwrap_or_default();
    let package_edges = artifacts
        .get("package-depends.dot")
        .map(|value| parse_dot_edges(value))
        .unwrap_or_default();
    let build_list = artifacts
        .get("pn-buildlist")
        .map(|value| parse_lines(value))
        .unwrap_or_default();
    let providers = artifacts
        .get("pn-provides")
        .map(|value| parse_providers(value))
        .unwrap_or_default();

    Ok(SemanticDependencyGraph {
        target: target.to_owned(),
        succeeded,
        diagnostics,
        task_edges,
        recipe_edges,
        package_edges,
        build_list,
        providers,
        artifacts,
    })
}

fn restore_graph_artifacts(
    build_dir: &Path,
    previous_artifacts: &BTreeMap<&str, Option<String>>,
) -> Result<(), SemanticError> {
    for (filename, previous) in previous_artifacts {
        let path = build_dir.join(filename);
        match previous {
            Some(contents) => fs::write(path, contents)?,
            None if path.is_file() => fs::remove_file(path)?,
            None => {}
        }
    }
    Ok(())
}

fn run_dry_run(options: &SemanticOptions) -> Result<SemanticDryRun, SemanticError> {
    let mut argument_storage = vec!["--dry-run".to_owned()];
    argument_storage.extend(options.targets.iter().cloned());
    let arguments = argument_storage
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let output = run_bitbake(options, &arguments)?;
    let diagnostics = parse_diagnostics(&output, SemanticDiagnosticPhase::DryRun, None);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let succeeded = output.status.success();
    let mut diagnostics = diagnostics;
    if !succeeded && diagnostics.is_empty() {
        diagnostics.push(analysis_diagnostic(
            SemanticDiagnosticPhase::DryRun,
            None,
            "BitBake dry-run command failed",
        ));
    }
    Ok(SemanticDryRun {
        targets: options.targets.clone(),
        succeeded,
        diagnostics,
        tasks: parse_dry_run_tasks(&stdout, &stderr),
        stdout,
        stderr,
    })
}

fn run_recipe_inventory(
    options: &SemanticOptions,
) -> Result<SemanticRecipeInventory, SemanticError> {
    let output = run_bitbake(options, &["--show-versions"])?;
    let diagnostics = parse_diagnostics(&output, SemanticDiagnosticPhase::Inventory, None);
    let raw = combined_output(&output);
    let succeeded = output.status.success();
    let mut diagnostics = diagnostics;
    if !succeeded && diagnostics.is_empty() {
        diagnostics.push(analysis_diagnostic(
            SemanticDiagnosticPhase::Inventory,
            None,
            "BitBake recipe inventory command failed",
        ));
    }
    Ok(SemanticRecipeInventory {
        succeeded,
        diagnostics,
        recipes: parse_recipe_versions(&raw),
        providers: Vec::new(),
        raw,
    })
}

fn package_summary_from_environment(environment: &SemanticEnvironment) -> SemanticPackageSummary {
    let values = parse_environment_values(&environment.raw);
    let packages = split_words(values.get("PACKAGES"));
    let provides = split_words(values.get("PROVIDES"));
    let build_dependencies = split_words(values.get("DEPENDS"));
    let image_install = split_words(values.get("IMAGE_INSTALL"));
    let image_fstypes = split_words(values.get("IMAGE_FSTYPES"));
    let mut runtime_dependencies = BTreeMap::new();
    let mut runtime_recommends = BTreeMap::new();
    let mut runtime_provides = BTreeMap::new();
    for (name, value) in &values {
        if let Some(package) = variable_package_suffix(name, "RDEPENDS") {
            runtime_dependencies.insert(package, split_words(Some(value)));
        } else if let Some(package) = variable_package_suffix(name, "RRECOMMENDS") {
            runtime_recommends.insert(package, split_words(Some(value)));
        } else if let Some(package) = variable_package_suffix(name, "RPROVIDES") {
            runtime_provides.insert(package, split_words(Some(value)));
        }
    }
    SemanticPackageSummary {
        target: environment.target.clone(),
        succeeded: true,
        diagnostics: Vec::new(),
        packages,
        provides,
        build_dependencies,
        image_install,
        image_fstypes,
        runtime_dependencies,
        runtime_recommends,
        runtime_provides,
    }
}

fn failed_package_summary(target: &str) -> SemanticPackageSummary {
    SemanticPackageSummary {
        target: target.to_owned(),
        succeeded: false,
        diagnostics: vec![analysis_diagnostic(
            SemanticDiagnosticPhase::PackageSummary,
            Some(target),
            "package metadata unavailable because the target environment query failed",
        )],
        packages: Vec::new(),
        provides: Vec::new(),
        build_dependencies: Vec::new(),
        image_install: Vec::new(),
        image_fstypes: Vec::new(),
        runtime_dependencies: BTreeMap::new(),
        runtime_recommends: BTreeMap::new(),
        runtime_provides: BTreeMap::new(),
    }
}

fn variable_package_suffix(name: &str, prefix: &str) -> Option<String> {
    let suffix = name
        .strip_prefix(&format!("{prefix}:"))
        .or_else(|| name.strip_prefix(&format!("{prefix}_")))?;
    if suffix.is_empty() {
        None
    } else {
        Some(
            suffix
                .trim_matches(|character| character == '{' || character == '}')
                .to_owned(),
        )
    }
}

fn split_words(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| value.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn parse_dot_edges(raw: &str) -> Vec<SemanticGraphEdge> {
    raw.lines()
        .filter_map(|line| {
            let (from, to) = line.split_once("->")?;
            let from = parse_dot_node(from)?;
            let to = parse_dot_node(to)?;
            Some(SemanticGraphEdge { from, to })
        })
        .collect()
}

fn parse_dot_node(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(stripped) = value.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].replace("\\\"", "\""))
    } else {
        value
            .split_whitespace()
            .next()
            .map(|value| value.trim_matches(';').to_owned())
    }
}

fn parse_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn parse_providers(raw: &str) -> Vec<SemanticProvider> {
    let mut providers = Vec::new();
    for line in parse_lines(raw) {
        let (recipe, names) = if let Some((recipe, names)) = line.split_once(':') {
            (recipe.trim().to_owned(), names.trim().to_owned())
        } else {
            let mut words = line.split_whitespace();
            let recipe = words.next().unwrap_or_default().to_owned();
            let names = words.collect::<Vec<_>>().join(" ");
            (recipe, names)
        };
        if recipe.is_empty() || names.is_empty() {
            continue;
        }
        for name in names.split_whitespace() {
            providers.push(SemanticProvider {
                name: name.to_owned(),
                recipe: recipe.clone(),
            });
        }
    }
    providers
}

fn parse_recipe_versions(raw: &str) -> Vec<SemanticRecipeVersion> {
    let mut recipes = Vec::new();
    for line in raw.lines().map(str::trim) {
        if line.is_empty()
            || line.starts_with("Recipe Name")
            || line.starts_with("NOTE:")
            || line.starts_with("WARNING:")
            || line.starts_with("DEBUG:")
            || line
                .chars()
                .all(|character| character == '=' || character.is_whitespace())
        {
            continue;
        }
        let fields = line.split(':').map(str::trim).collect::<Vec<_>>();
        let (recipe, version, revision) =
            if fields.len() >= 2 && !fields[0].contains(char::is_whitespace) {
                (fields[0], fields[1], fields.get(2).copied())
            } else {
                let mut fields = line.split_whitespace();
                let recipe = fields.next().unwrap_or_default();
                let version = fields.next().unwrap_or_default();
                (recipe, version, fields.next())
            };
        if !recipe.is_empty() && !version.is_empty() {
            recipes.push(SemanticRecipeVersion {
                recipe: recipe.to_owned(),
                version: version.to_owned(),
                revision: revision
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
            });
        }
    }
    recipes
}

fn parse_dry_run_tasks(stdout: &str, stderr: &str) -> Vec<String> {
    stdout
        .lines()
        .chain(stderr.lines())
        .filter_map(|line| {
            let marker = ["Running task", "Executing task"]
                .iter()
                .find(|marker| line.contains(**marker))?;
            if let Some(start) = line.find('(') {
                let end = line[start + 1..].find(')')? + start + 1;
                return Some(line[start + 1..end].to_owned());
            }
            Some(line[line.find(*marker)? + marker.len()..].trim().to_owned())
        })
        .collect()
}

fn analysis_diagnostic(
    phase: SemanticDiagnosticPhase,
    target: Option<&str>,
    message: &str,
) -> SemanticDiagnostic {
    SemanticDiagnostic {
        phase,
        target: target.map(str::to_owned),
        stream: SemanticDiagnosticStream::Stderr,
        severity: SemanticSeverity::Error,
        path: None,
        line: None,
        column: None,
        message: message.to_owned(),
        raw: message.to_owned(),
    }
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
    let all_values = parse_environment_values(raw);
    let variables = all_values
        .into_iter()
        .filter(|(name, _)| {
            requested_variables
                .iter()
                .any(|requested| requested == name)
        })
        .collect();
    SemanticEnvironment {
        target: target.to_owned(),
        variables,
        raw: raw.to_owned(),
    }
}

fn parse_environment_values(raw: &str) -> BTreeMap<String, String> {
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
        variables.insert(name.to_owned(), decode_environment_value(&encoded_value));
        index = next.max(index + 1);
    }
    variables
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

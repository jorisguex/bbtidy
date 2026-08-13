use bbtidy::{
    BitBakeCancellationToken, BitBakeExecutionLimits, BitBakeExecutionStats, BitBakeRunner,
    BuildContext, BuildContextDiscoveryOptions, Config, LintDiagnostic, LintFailurePolicy,
    LintProfile, LintSeverity, LintSuppressionSummary, SafetyOptions, SemanticAnalysisOptions,
    SemanticOptions, SemanticReport, SyntaxKind, Token, WorkspaceIndex,
    analyze_bitbake_with_limits, analyze_bitbake_with_runner, apply_lint_fixes,
    discover_build_context_with_options, format, format_with_options, get_line_col, lint_rules,
    lint_with_options, lint_with_workspace, load_config, parse, semantic_lint_diagnostics,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use logos::Logos;
use serde_json::{Value, json};
use similar::TextDiff;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};

const EXIT_DIFFERENCES: i32 = 1;
const EXIT_ERROR: i32 = 2;
const BITBAKE_EXTENSIONS: &[&str] = &["bb", "bbappend", "bbclass", "conf", "inc"];
static CLI_CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Format and inspect BitBake metadata",
    long_about = None
)]
struct Cli {
    /// Read configuration from an explicit TOML file.
    #[arg(long, global = true, conflicts_with = "no_config", value_name = "PATH")]
    config: Option<PathBuf>,

    /// Do not discover or load a project configuration file.
    #[arg(long, global = true, conflicts_with = "config")]
    no_config: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Format BitBake metadata
    Format(FormatArgs),

    /// Check BitBake metadata for lint findings
    Check(LintArgs),

    /// Print the lexer token stream
    Lex(InputArgs),

    /// Run authoritative semantic analysis through BitBake
    Semantic(SemanticArgs),

    /// Report CST coverage metrics for the compatibility harness
    #[command(hide = true)]
    SyntaxStats(SyntaxStatsArgs),
}

#[derive(Args)]
struct FormatArgs {
    /// Check whether files are formatted without writing them.
    #[arg(long, conflicts_with_all = ["write", "diff"])]
    check: bool,

    /// Rewrite files in place
    #[arg(long, conflicts_with = "diff")]
    write: bool,

    /// Print a unified diff instead of formatted source
    #[arg(long, conflicts_with = "write")]
    diff: bool,

    /// Override the configured maximum number of files for this invocation.
    #[arg(long, value_name = "N")]
    max_files: Option<usize>,

    /// Override the configured maximum total source size in bytes.
    #[arg(long, value_name = "BYTES")]
    max_bytes: Option<u64>,

    #[command(flatten)]
    inputs: InputArgs,
}

#[derive(Args)]
struct LintArgs {
    /// Select human-readable text, JSON, or SARIF diagnostics.
    #[arg(long, value_enum, default_value_t = LintOutput::Text, value_name = "FORMAT")]
    output: LintOutput,

    /// Select an evidence-driven lint profile.
    #[arg(long, value_enum, value_name = "PROFILE")]
    profile: Option<LintProfileArg>,

    /// Enable one or more rules in addition to the selected profile.
    #[arg(long = "enable", value_name = "RULE")]
    enable: Vec<String>,

    /// Disable one or more rules after profile and enable selection.
    #[arg(long = "disable", value_name = "RULE")]
    disable: Vec<String>,

    /// Set the minimum diagnostic severity that fails the command.
    #[arg(long, value_enum, value_name = "SEVERITY")]
    fail_on: Option<LintFailureArg>,

    /// Apply safe, machine-generated fixes and re-lint the resulting source.
    #[arg(long)]
    fix: bool,

    /// Include help and edit details in human-readable diagnostics.
    #[arg(long)]
    show_fixes: bool,

    /// Include findings matched by inline suppressions in machine output.
    #[arg(long)]
    show_suppressed: bool,

    /// Emit the versioned machine-readable output schema.
    #[arg(long, default_value_t = 1, value_name = "N")]
    output_version: u8,

    /// Write the current findings as an adoption baseline.
    #[arg(long, value_name = "PATH", conflicts_with = "refresh_baseline")]
    write_baseline: Option<PathBuf>,

    /// Compare findings with an adoption baseline.
    #[arg(long, value_name = "PATH")]
    baseline: Option<PathBuf>,

    /// Include findings already present in the baseline in the blocking set.
    #[arg(long)]
    show_existing: bool,

    /// Explicitly replace a baseline with the current findings.
    #[arg(long, value_name = "PATH")]
    refresh_baseline: Option<PathBuf>,

    /// Override the configured maximum number of files for this invocation.
    #[arg(long, value_name = "N")]
    max_files: Option<usize>,

    /// Override the configured maximum total source size in bytes.
    #[arg(long, value_name = "BYTES")]
    max_bytes: Option<u64>,

    /// Lint the complete BitBake workspace described by build/conf/bblayers.conf.
    #[arg(long, value_name = "BUILD_DIR")]
    workspace: Option<PathBuf>,

    #[command(flatten)]
    semantic: SemanticLintArgs,

    #[command(flatten)]
    inputs: InputArgs,
}

#[derive(Args)]
struct SemanticLintArgs {
    /// Run BitBake-backed semantic analysis in addition to static linting.
    #[arg(long)]
    semantic: bool,

    /// Existing BitBake build directory containing conf/local.conf and conf/bblayers.conf.
    #[arg(long, value_name = "PATH")]
    build_dir: Option<PathBuf>,

    /// Project directory from which to discover a build directory.
    #[arg(long, value_name = "PATH")]
    project_dir: Option<PathBuf>,

    /// BitBake executable for semantic analysis or --workspace; defaults to project configuration or PATH.
    #[arg(long, value_name = "PATH")]
    bitbake: Option<PathBuf>,

    /// Recipe or target to inspect. May be supplied more than once.
    #[arg(long = "target", value_name = "TARGET")]
    targets: Vec<String>,

    /// Fully expanded variable to include in the semantic report. May be supplied more than once.
    #[arg(long = "variable", value_name = "NAME")]
    variables: Vec<String>,

    /// Run every available BitBake build analysis section.
    #[arg(long)]
    full: bool,

    /// Collect BitBake task, recipe, and package dependency graphs.
    #[arg(long)]
    graph: bool,

    /// Run BitBake's scheduler in dry-run mode and report planned tasks.
    #[arg(long)]
    dry_run: bool,

    /// Include the parsed recipe/version inventory.
    #[arg(long)]
    inventory: bool,

    /// Include resolved package, provider, runtime dependency, and image metadata.
    #[arg(long)]
    packages: bool,

    #[command(flatten)]
    bitbake_limits: BitBakeLimitArgs,
}

#[derive(Args)]
struct SemanticArgs {
    /// Existing BitBake build directory containing conf/local.conf and conf/bblayers.conf.
    #[arg(long, value_name = "PATH")]
    build_dir: Option<PathBuf>,

    /// Project directory from which to discover a build directory.
    #[arg(long, value_name = "PATH")]
    project_dir: Option<PathBuf>,

    /// BitBake executable to invoke; defaults to project configuration or PATH.
    #[arg(long, value_name = "PATH")]
    bitbake: Option<PathBuf>,

    /// Recipe or target to inspect. May be supplied more than once.
    #[arg(long = "target", value_name = "TARGET")]
    targets: Vec<String>,

    /// Fully expanded variable to report for every selected target. May be supplied more than once.
    #[arg(long = "variable", value_name = "NAME")]
    variables: Vec<String>,

    /// Run every available BitBake build analysis section.
    #[arg(long)]
    full: bool,

    /// Collect BitBake task, recipe, and package dependency graphs.
    #[arg(long)]
    graph: bool,

    /// Run BitBake's scheduler in dry-run mode and report planned tasks.
    #[arg(long)]
    dry_run: bool,

    /// Include the parsed recipe/version inventory.
    #[arg(long)]
    inventory: bool,

    /// Include resolved package, provider, runtime dependency, and image metadata.
    #[arg(long)]
    packages: bool,

    /// Select human-readable text or JSON output.
    #[arg(long, value_enum, default_value_t = SemanticOutput::Text, value_name = "FORMAT")]
    output: SemanticOutput,

    #[command(flatten)]
    bitbake_limits: BitBakeLimitArgs,
}

#[derive(Args, Clone, Default)]
struct BitBakeLimitArgs {
    /// Maximum seconds for one BitBake command.
    #[arg(long = "bitbake-command-timeout-seconds", value_name = "SECONDS")]
    command_timeout_seconds: Option<u64>,

    /// Maximum seconds for all BitBake commands in this operation.
    #[arg(long = "bitbake-total-timeout-seconds", value_name = "SECONDS")]
    total_timeout_seconds: Option<u64>,

    /// Maximum captured stdout bytes per BitBake command.
    #[arg(long = "bitbake-max-stdout-bytes", value_name = "BYTES")]
    max_stdout_bytes: Option<u64>,

    /// Maximum captured stderr bytes per BitBake command.
    #[arg(long = "bitbake-max-stderr-bytes", value_name = "BYTES")]
    max_stderr_bytes: Option<u64>,

    /// Maximum BitBake process launches in this operation.
    #[arg(long = "bitbake-max-commands", value_name = "N")]
    max_commands: Option<usize>,

    /// Maximum recipe-specific environment queries in this operation.
    #[arg(long = "bitbake-max-recipe-queries", value_name = "N")]
    max_recipe_queries: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum SemanticOutput {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LintOutput {
    Text,
    Json,
    Sarif,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LintProfileArg {
    Essential,
    Recommended,
    Strict,
    All,
}

impl From<LintProfileArg> for LintProfile {
    fn from(value: LintProfileArg) -> Self {
        match value {
            LintProfileArg::Essential => Self::Essential,
            LintProfileArg::Recommended => Self::Recommended,
            LintProfileArg::Strict => Self::Strict,
            LintProfileArg::All => Self::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LintFailureArg {
    Info,
    Warning,
    Error,
    Never,
}

impl From<LintFailureArg> for LintFailurePolicy {
    fn from(value: LintFailureArg) -> Self {
        match value {
            LintFailureArg::Info => Self::Info,
            LintFailureArg::Warning => Self::Warning,
            LintFailureArg::Error => Self::Error,
            LintFailureArg::Never => Self::Never,
        }
    }
}

impl BitBakeLimitArgs {
    fn is_set(&self) -> bool {
        self.command_timeout_seconds.is_some()
            || self.total_timeout_seconds.is_some()
            || self.max_stdout_bytes.is_some()
            || self.max_stderr_bytes.is_some()
            || self.max_commands.is_some()
            || self.max_recipe_queries.is_some()
    }
}

#[derive(Args)]
struct InputArgs {
    /// Files or directories to process; use '-' to read standard input
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,
}

#[derive(Args)]
struct SyntaxStatsArgs {
    /// Include per-file unknown-node records and before/after formatting data.
    #[arg(long)]
    details: bool,

    #[command(flatten)]
    inputs: InputArgs,
}

#[derive(Debug)]
enum Input {
    Stdin,
    File(PathBuf),
}

struct FormattedInput {
    label: String,
    path: Option<PathBuf>,
    original: String,
    formatted: String,
}

fn main() {
    let cli = Cli::parse();
    let config = match load_config(cli.config.as_deref(), cli.no_config) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            process::exit(EXIT_ERROR);
        }
    };
    let exit_code = match cli.command {
        Command::Format(args) => run_format(args, &config),
        Command::Check(args) => run_lint(args, &config),
        Command::Lex(args) => run_lex(args, &config),
        Command::Semantic(args) => run_semantic(args, &config),
        Command::SyntaxStats(args) => run_syntax_stats(args, &config),
    };

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

fn run_semantic(args: SemanticArgs, config: &Config) -> i32 {
    install_cli_cancellation_handler();
    let start = match args.project_dir {
        Some(path) => path,
        None => match std::env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("error: could not determine project directory: {error}");
                return EXIT_ERROR;
            }
        },
    };
    let context_result = match args.build_dir {
        Some(path) => BuildContext::from_build_dir(path),
        None => {
            let mut discovery = BuildContextDiscoveryOptions::from_environment();
            discovery.configured_build_dir = config.semantic.build_dir.clone();
            discover_build_context_with_options(&start, &discovery)
        }
    };
    let context = match context_result {
        Ok(context) => context,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };

    let options = SemanticOptions {
        bitbake: args
            .bitbake
            .or_else(|| config.semantic.bitbake.clone())
            .unwrap_or_else(|| PathBuf::from("bitbake")),
        build_dir: context.build_dir().to_path_buf(),
        targets: args.targets,
        variables: args.variables,
        analysis: semantic_analysis_options(
            args.full,
            args.graph,
            args.dry_run,
            args.inventory,
            args.packages,
            &config.semantic.analysis,
        ),
    };
    let limits = effective_bitbake_limits(&config.bitbake, &args.bitbake_limits);
    let cancellation = BitBakeCancellationToken::with_external_flag(&CLI_CANCELLED);
    let report = match analyze_bitbake_with_limits(&options, limits, cancellation) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };

    if args.output == SemanticOutput::Json {
        let value = serde_json::json!({
            "version": 1,
            "bitbake": report.bitbake(),
            "bitbake_version": report.bitbake_version(),
            "project_dir": context.project_dir(),
            "build_dir": report.build_dir(),
            "build_context_source": context.source(),
            "requested_targets": report.requested_targets(),
            "requested_variables": report.requested_variables(),
            "parse_succeeded": report.parse_succeeded(),
            "target_queries_succeeded": report.target_queries_succeeded(),
            "analysis_succeeded": report.analysis_succeeded(),
            "diagnostics": report.diagnostics(),
            "environments": report.environments(),
            "target_results": report.target_results(),
            "build_analysis": report.build_analysis(),
            "execution": report.execution(),
        });
        match serde_json::to_writer_pretty(io::stdout().lock(), &value) {
            Ok(()) => println!(),
            Err(error) => {
                eprintln!("error: could not write semantic report: {error}");
                return EXIT_ERROR;
            }
        }
    } else {
        println!("BitBake: {}", report.bitbake_version());
        println!("Project directory: {}", context.project_dir().display());
        println!("Build directory: {}", report.build_dir().display());
        println!("Build context: {}", context.source());
        println!(
            "Parse: {}",
            if report.parse_succeeded() {
                "passed"
            } else {
                "failed"
            }
        );
        println!(
            "Target queries: {}",
            if report.target_queries_succeeded() {
                "passed"
            } else {
                "failed"
            }
        );
        if !report.requested_targets().is_empty() {
            println!(
                "Requested targets: {}",
                report.requested_targets().join(", ")
            );
        }
        if !report.requested_variables().is_empty() {
            println!(
                "Requested variables: {}",
                report.requested_variables().join(", ")
            );
        }
        for diagnostic in report.diagnostics() {
            let location = match (diagnostic.path(), diagnostic.line(), diagnostic.column()) {
                (Some(path), Some(line), Some(column)) => {
                    format!("{}:{line}:{column}", path.display())
                }
                (Some(path), Some(line), None) => format!("{}:{line}", path.display()),
                (Some(path), None, _) => path.display().to_string(),
                _ => String::new(),
            };
            if location.is_empty() {
                println!("{}: {}", diagnostic.severity(), diagnostic.message());
            } else {
                println!(
                    "{}: {}: {}",
                    location,
                    diagnostic.severity(),
                    diagnostic.message()
                );
            }
        }
        for result in report.target_results() {
            let status = if !result.queried() {
                "skipped"
            } else if result.succeeded() {
                "passed"
            } else {
                "failed"
            };
            println!("Target {}: {status}", result.target());
            if let Some(environment) = result.environment() {
                for (name, value) in environment.variables() {
                    println!("  {name}={value}");
                }
            }
        }
        if let Some(analysis) = report.build_analysis() {
            println!(
                "Build analysis: {}",
                if analysis.succeeded() {
                    "passed"
                } else {
                    "failed"
                }
            );
            for graph in analysis.graphs() {
                println!(
                    "Dependency graph {}: {} task edges, {} recipe edges, {} package edges",
                    graph.target(),
                    graph.task_edges().len(),
                    graph.recipe_edges().len(),
                    graph.package_edges().len()
                );
            }
            if let Some(dry_run) = analysis.dry_run() {
                println!("Dry-run: {} planned tasks", dry_run.tasks().len());
            }
            if let Some(inventory) = analysis.inventory() {
                println!("Recipe inventory: {} recipes", inventory.recipes().len());
            }
            for packages in analysis.packages() {
                println!(
                    "Packages {}: {} packages, {} build dependencies",
                    packages.target(),
                    packages.packages().len(),
                    packages.build_dependencies().len()
                );
            }
        }
        println!(
            "BitBake commands: {} ({} cache hits)",
            report.execution().total_commands,
            report.execution().cache_hits
        );
    }

    if report.analysis_succeeded() && !report.has_errors() {
        0
    } else {
        EXIT_DIFFERENCES
    }
}

fn analyze_semantic_lint(
    args: &SemanticLintArgs,
    config: &Config,
    runner: &mut BitBakeRunner,
) -> Result<SemanticLintAnalysis, String> {
    let start = match &args.project_dir {
        Some(path) => path.clone(),
        None => std::env::current_dir()
            .map_err(|error| format!("could not determine project directory: {error}"))?,
    };
    let context_result = match &args.build_dir {
        Some(path) => BuildContext::from_build_dir(path),
        None => {
            let mut discovery = BuildContextDiscoveryOptions::from_environment();
            discovery.configured_build_dir = config.semantic.build_dir.clone();
            discover_build_context_with_options(&start, &discovery)
        }
    };
    let context = context_result.map_err(|error| error.to_string())?;
    let mut variables = [
        "SUMMARY",
        "DESCRIPTION",
        "LICENSE",
        "LIC_FILES_CHKSUM",
        "SRCREV",
        "SRCPV",
        "SRC_URI",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    for variable in &args.variables {
        if !variables.iter().any(|known| known == variable) {
            variables.push(variable.clone());
        }
    }
    let options = SemanticOptions {
        bitbake: args
            .bitbake
            .clone()
            .or_else(|| config.semantic.bitbake.clone())
            .unwrap_or_else(|| PathBuf::from("bitbake")),
        build_dir: context.build_dir().to_path_buf(),
        targets: args.targets.clone(),
        variables,
        analysis: semantic_analysis_options(
            args.full,
            args.graph,
            args.dry_run,
            args.inventory,
            args.packages,
            &config.semantic.analysis,
        ),
    };
    let report =
        analyze_bitbake_with_runner(&options, runner).map_err(|error| error.to_string())?;
    Ok(SemanticLintAnalysis { context, report })
}

fn semantic_analysis_options(
    full: bool,
    graph: bool,
    dry_run: bool,
    inventory: bool,
    packages: bool,
    configured: &SemanticAnalysisOptions,
) -> SemanticAnalysisOptions {
    if full {
        SemanticAnalysisOptions::full()
    } else {
        SemanticAnalysisOptions {
            dependency_graph: configured.dependency_graph || graph,
            dry_run: configured.dry_run || dry_run,
            inventory: configured.inventory || inventory,
            packages: configured.packages || packages,
        }
    }
}

fn run_format(args: FormatArgs, config: &Config) -> i32 {
    let inputs = match resolve_inputs(&args.inputs.paths, config) {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };

    if !args.write && !args.diff && !args.check && inputs.len() != 1 {
        eprintln!(
            "error: formatting to standard output requires exactly one input; use --check, --diff, or --write for multiple inputs"
        );
        return EXIT_ERROR;
    }

    if args.write && inputs.iter().any(|input| matches!(input, Input::Stdin)) {
        eprintln!("error: --write cannot be used with standard input");
        return EXIT_ERROR;
    }

    let limits = SafetyOptions {
        max_files: args.max_files.unwrap_or(config.safety.max_files),
        max_bytes: args.max_bytes.unwrap_or(config.safety.max_bytes),
    };
    if let Err(error) = validate_input_limits(&inputs, limits) {
        eprintln!("error: {error}");
        return EXIT_ERROR;
    }

    let formatted_inputs = match format_inputs(&inputs, &config.format) {
        Ok(formatted_inputs) => formatted_inputs,
        Err(()) => return EXIT_ERROR,
    };

    if let Err(error) = validate_formatted_limits(&formatted_inputs, limits) {
        eprintln!("error: {error}");
        return EXIT_ERROR;
    }

    if args.write {
        write_inputs(&formatted_inputs)
    } else if args.diff {
        print_diffs(&formatted_inputs)
    } else if args.check {
        check_formatted_inputs(&formatted_inputs)
    } else {
        let mut stdout = io::stdout().lock();
        match stdout.write_all(formatted_inputs[0].formatted.as_bytes()) {
            Ok(()) => 0,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
            Err(error) => {
                eprintln!("error: could not write standard output: {error}");
                EXIT_ERROR
            }
        }
    }
}

fn check_formatted_inputs(formatted_inputs: &[FormattedInput]) -> i32 {
    let mut differences = false;
    for input in formatted_inputs {
        if input.original != input.formatted {
            println!("would reformat: {}", input.label);
            differences = true;
        }
    }

    if differences { EXIT_DIFFERENCES } else { 0 }
}

fn run_lint(args: LintArgs, config: &Config) -> i32 {
    install_cli_cancellation_handler();
    if !matches!(args.output_version, 1 | 2) {
        eprintln!(
            "error: unsupported lint output version {}; expected 1 or 2",
            args.output_version
        );
        return EXIT_ERROR;
    }
    let mut lint_options = config.lint.clone();
    if let Some(profile) = args.profile {
        lint_options.set_profile(profile.into());
    }
    let known_rules = lint_rules()
        .iter()
        .map(|rule| rule.id())
        .collect::<BTreeSet<_>>();
    for rule_id in args.enable.iter().chain(args.disable.iter()) {
        if !known_rules.contains(rule_id.as_str()) {
            eprintln!("error: unknown lint rule '{rule_id}'");
            return EXIT_ERROR;
        }
    }
    if let Some(rule_id) = args.enable.iter().find(|rule_id| {
        args.disable.iter().any(|disabled| disabled == *rule_id)
            || lint_options.is_explicitly_disabled(rule_id)
    }) {
        eprintln!("error: lint rule '{rule_id}' cannot be both enabled and disabled");
        return EXIT_ERROR;
    }
    if let Some(rule_id) = args
        .disable
        .iter()
        .find(|rule_id| lint_options.is_explicitly_enabled(rule_id))
    {
        eprintln!("error: lint rule '{rule_id}' cannot be both enabled and disabled");
        return EXIT_ERROR;
    }
    for rule_id in &args.enable {
        lint_options.enable_rule(rule_id.clone());
    }
    for rule_id in &args.disable {
        lint_options.disable_rule(rule_id.clone());
    }
    lint_options.set_show_suppressed(args.show_suppressed);
    if let Some(fail_on) = args.fail_on {
        lint_options.set_fail_on(fail_on.into());
    }
    if args.workspace.is_some() && !args.inputs.paths.is_empty() {
        eprintln!("error: --workspace cannot be combined with file or directory inputs");
        return EXIT_ERROR;
    }
    let limits = SafetyOptions {
        max_files: args.max_files.unwrap_or(config.safety.max_files),
        max_bytes: args.max_bytes.unwrap_or(config.safety.max_bytes),
    };
    let bitbake_limits = effective_bitbake_limits(&config.bitbake, &args.semantic.bitbake_limits);
    let uses_bitbake = args.workspace.is_some() || args.semantic.semantic;
    let mut bitbake_runner = if uses_bitbake {
        match BitBakeRunner::with_cancellation(
            bitbake_limits,
            BitBakeCancellationToken::with_external_flag(&CLI_CANCELLED),
        ) {
            Ok(runner) => Some(runner),
            Err(error) => {
                eprintln!("error: invalid BitBake execution limits: {error}");
                return EXIT_ERROR;
            }
        }
    } else {
        None
    };
    let (inputs, workspace) = if let Some(build_dir) = args.workspace.as_deref() {
        let bitbake = args
            .semantic
            .bitbake
            .clone()
            .or_else(|| config.semantic.bitbake.clone())
            .unwrap_or_else(|| PathBuf::from("bitbake"));
        let workspace = match WorkspaceIndex::from_bitbake_with_runner(
            build_dir,
            &bitbake,
            |path| !config.is_excluded(path),
            Some(limits),
            bitbake_runner.as_mut().expect("workspace runner"),
        ) {
            Ok(workspace) => workspace,
            Err(error) => {
                eprintln!(
                    "error: could not resolve build workspace through BitBake {}: {error}",
                    bitbake.display()
                );
                return EXIT_ERROR;
            }
        };
        let inputs = workspace
            .files()
            .map(|path| Input::File(path.to_path_buf()))
            .collect::<Vec<_>>();
        if inputs.is_empty() {
            eprintln!("error: no BitBake files found in the build workspace");
            return EXIT_ERROR;
        }
        (inputs, workspace)
    } else {
        let inputs = match resolve_inputs(&args.inputs.paths, config) {
            Ok(inputs) => inputs,
            Err(error) => {
                eprintln!("error: {error}");
                return EXIT_ERROR;
            }
        };
        let workspace_paths = inputs.iter().filter_map(|input| match input {
            Input::Stdin => None,
            Input::File(path) => Some(path),
        });
        let workspace = match WorkspaceIndex::from_paths(workspace_paths) {
            Ok(workspace) => workspace,
            Err(error) => {
                eprintln!("error: could not index workspace: {error}");
                return EXIT_ERROR;
            }
        };
        (inputs, workspace)
    };
    if !args.semantic.semantic
        && (args.semantic.build_dir.is_some()
            || args.semantic.project_dir.is_some()
            || !args.semantic.targets.is_empty()
            || !args.semantic.variables.is_empty()
            || args.semantic.full
            || args.semantic.graph
            || args.semantic.dry_run
            || args.semantic.inventory
            || args.semantic.packages
            || args.semantic.bitbake_limits.is_set()
            || (args.semantic.bitbake.is_some() && args.workspace.is_none()))
    {
        eprintln!(
            "error: --build-dir, --project-dir, --target, --variable, --full, --graph, --dry-run, --inventory, and --packages require --semantic; --bitbake requires --semantic unless used with --workspace"
        );
        return EXIT_ERROR;
    }
    if args.semantic.semantic && inputs.iter().any(|input| matches!(input, Input::Stdin)) {
        eprintln!("error: --semantic cannot be used with standard input");
        return EXIT_ERROR;
    }
    if args.fix && inputs.iter().any(|input| matches!(input, Input::Stdin)) {
        eprintln!("error: --fix cannot be used with standard input");
        return EXIT_ERROR;
    }
    if let Err(error) = validate_input_limits(&inputs, limits) {
        eprintln!("error: {error}");
        return EXIT_ERROR;
    }
    let semantic_report = if args.semantic.semantic {
        match analyze_semantic_lint(
            &args.semantic,
            config,
            bitbake_runner.as_mut().expect("semantic runner"),
        ) {
            Ok(report) => Some(report),
            Err(error) => {
                eprintln!("error: {error}");
                return EXIT_ERROR;
            }
        }
    } else {
        None
    };
    let semantic_findings = semantic_report
        .as_ref()
        .map(|analysis| semantic_lint_diagnostics(&analysis.report, &lint_options))
        .unwrap_or_default();
    let mut had_error = false;
    let machine_output = args.output != LintOutput::Text;
    let mut analyzed = Vec::with_capacity(inputs.len());

    for input in &inputs {
        let (label, text) = match read_input(input) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("error: {error}");
                had_error = true;
                continue;
            }
        };
        if matches!(input, Input::Stdin) && text.len() as u64 > limits.max_bytes {
            eprintln!(
                "error: safety limit exceeded: {} bytes read, maximum is {}",
                text.len(),
                limits.max_bytes
            );
            had_error = true;
            continue;
        }
        let diagnostics = match input {
            Input::Stdin => lint_with_options(&text, &lint_options),
            Input::File(path) => lint_with_workspace(&text, path, &workspace, &lint_options),
        };
        match diagnostics {
            Ok(diagnostics) => {
                let fixed = if args.fix {
                    match apply_lint_fixes(&text, &diagnostics) {
                        Ok(fixed) => fixed,
                        Err(error) => {
                            eprintln!("error: could not apply fixes to {label}: {error}");
                            had_error = true;
                            continue;
                        }
                    }
                } else {
                    text.clone()
                };
                let fixes_applied = if args.fix {
                    diagnostics
                        .iter()
                        .map(|diagnostic| diagnostic.fixes().len())
                        .sum()
                } else {
                    0
                };
                analyzed.push(AnalyzedLintInput {
                    label,
                    path: match input {
                        Input::Stdin => None,
                        Input::File(path) => Some(path.clone()),
                    },
                    original: text,
                    fixed,
                    diagnostics,
                    fixes_applied,
                });
            }
            Err(error) => {
                eprintln!("error: could not lint {label}: {error}");
                had_error = true;
            }
        }
    }

    if had_error {
        return EXIT_ERROR;
    }

    if args.fix {
        let formatted = analyzed
            .iter()
            .map(|input| FormattedInput {
                label: input.label.clone(),
                path: input.path.clone(),
                original: input.original.clone(),
                formatted: input.fixed.clone(),
            })
            .collect::<Vec<_>>();
        let changed = formatted
            .iter()
            .filter(|input| input.original != input.formatted)
            .collect::<Vec<_>>();
        if !changed.is_empty() {
            let pending = match stage_writes(&changed) {
                Ok(pending) => pending,
                Err(error) => {
                    eprintln!("error: could not prepare repository-wide lint fix: {error}");
                    return EXIT_ERROR;
                }
            };
            if let Err(error) = commit_writes(&pending) {
                eprintln!("error: could not commit repository-wide lint fix: {error}");
                return EXIT_ERROR;
            }
        }

        for input in &mut analyzed {
            if input.fixes_applied == 0 {
                continue;
            }
            let path = input
                .path
                .as_deref()
                .expect("lint fix mode rejects standard input");
            input.diagnostics =
                match lint_with_workspace(&input.fixed, path, &workspace, &lint_options) {
                    Ok(diagnostics) => diagnostics,
                    Err(error) => {
                        eprintln!("error: could not re-lint {}: {error}", input.label);
                        return EXIT_ERROR;
                    }
                };
        }
    }

    let mut collected = analyzed
        .iter()
        .flat_map(|input| {
            input
                .diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| ReportedDiagnostic {
                    label: input.label.clone(),
                    diagnostic,
                    baseline_status: BaselineFindingStatus::New,
                })
        })
        .collect::<Vec<_>>();
    collected.extend(
        semantic_findings
            .iter()
            .cloned()
            .map(|finding| ReportedDiagnostic {
                label: finding.label.clone(),
                diagnostic: finding.diagnostic.clone(),
                baseline_status: BaselineFindingStatus::New,
            }),
    );
    collected.sort_by(|left, right| {
        (
            left.label.as_str(),
            left.diagnostic.line(),
            left.diagnostic.column(),
            left.diagnostic.rule_id(),
        )
            .cmp(&(
                right.label.as_str(),
                right.diagnostic.line(),
                right.diagnostic.column(),
                right.diagnostic.rule_id(),
            ))
    });
    let applied_fixes = analyzed
        .iter()
        .filter(|input| input.fixes_applied > 0)
        .map(|input| AppliedFixSummary {
            path: input.label.clone(),
            count: input.fixes_applied,
        })
        .collect::<Vec<_>>();

    let sources = analyzed
        .iter()
        .map(|input| (input.label.clone(), input.fixed.clone()))
        .collect::<BTreeMap<_, _>>();
    let current_baseline_entries = baseline_entries(&collected, &sources);
    let suppression_summary =
        analyzed
            .iter()
            .fold(LintSuppressionSummary::default(), |mut total, input| {
                let current = bbtidy::lint_suppression_summary(&input.fixed);
                total.directives += current.directives;
                total.malformed += current.malformed;
                total.unknown_rules += current.unknown_rules;
                total
            });
    let configured_baseline = args
        .baseline
        .clone()
        .or_else(|| config.lint.baseline().map(Path::to_path_buf));
    if let Some(path) = args.write_baseline.as_deref() {
        if let Err(error) =
            write_baseline_file(path, lint_options.profile(), &current_baseline_entries)
        {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    }
    let comparison_path = args
        .refresh_baseline
        .clone()
        .or(configured_baseline.clone());
    let mut baseline_summary = None;
    if let Some(path) = comparison_path.as_deref() {
        let previous = if path.is_file() {
            match read_baseline_file(path, lint_options.profile()) {
                Ok(entries) => entries,
                Err(error) => {
                    eprintln!("error: {error}");
                    return EXIT_ERROR;
                }
            }
        } else if args.refresh_baseline.is_some() {
            Vec::new()
        } else {
            eprintln!("error: baseline {} does not exist", path.display());
            return EXIT_ERROR;
        };
        let previous_ids = previous
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<BTreeSet<_>>();
        let current_ids = current_baseline_entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<BTreeSet<_>>();
        for (entry, finding) in current_baseline_entries.iter().zip(collected.iter_mut()) {
            if previous_ids.contains(entry.id.as_str()) {
                finding.baseline_status = BaselineFindingStatus::Existing;
            }
        }
        let stale = previous_ids.difference(&current_ids).count();
        baseline_summary = Some(BaselineSummary {
            path: path.display().to_string(),
            existing: collected
                .iter()
                .filter(|entry| entry.baseline_status == BaselineFindingStatus::Existing)
                .count(),
            new: collected
                .iter()
                .filter(|entry| entry.baseline_status == BaselineFindingStatus::New)
                .count(),
            stale,
            profile: lint_options.profile().to_string(),
            refreshed: args.refresh_baseline.is_some(),
        });
        if args.refresh_baseline.is_some() {
            if let Err(error) =
                write_baseline_file(path, lint_options.profile(), &current_baseline_entries)
            {
                eprintln!("error: {error}");
                return EXIT_ERROR;
            }
        }
    }

    let mut stdout = io::stdout().lock();
    if machine_output {
        if let Err(error) = write_lint_report(
            args.output,
            args.output_version,
            lint_options.profile(),
            &collected,
            &applied_fixes,
            baseline_summary.as_ref(),
            suppression_summary,
            semantic_report.as_ref(),
            bitbake_runner.as_ref().map(BitBakeRunner::stats),
            &mut stdout,
        ) {
            if error.kind() == io::ErrorKind::BrokenPipe {
                return 0;
            }
            eprintln!("error: could not write standard output: {error}");
            return EXIT_ERROR;
        }
    }

    if !machine_output {
        for input in &analyzed {
            if input.fixes_applied > 0 {
                if let Err(error) = writeln!(
                    stdout,
                    "fixed: {} ({} edit{})",
                    input.label,
                    input.fixes_applied,
                    if input.fixes_applied == 1 { "" } else { "s" }
                ) {
                    if error.kind() == io::ErrorKind::BrokenPipe {
                        return 0;
                    }
                    eprintln!("error: could not write standard output: {error}");
                    return EXIT_ERROR;
                }
            }
        }
        for entry in &collected {
            if let Err(error) = write_text_diagnostic(
                &mut stdout,
                &entry.label,
                &entry.diagnostic,
                args.show_fixes,
            ) {
                if error.kind() == io::ErrorKind::BrokenPipe {
                    return 0;
                }
                eprintln!("error: could not write standard output: {error}");
                return EXIT_ERROR;
            }
        }
    }

    if !machine_output && let Some(runner) = bitbake_runner.as_ref() {
        let stats = runner.stats();
        eprintln!(
            "BitBake: {} commands, {} recipe queries, {} cache hits{}",
            stats.total_commands,
            stats.recipe_queries_completed,
            stats.cache_hits,
            stats
                .strategy
                .as_deref()
                .map(|strategy| format!(", strategy {strategy}"))
                .unwrap_or_default()
        );
    }

    if collected.iter().any(|entry| {
        (entry.diagnostic.rule_id() == "BBT038"
            || args.show_existing
            || entry.baseline_status == BaselineFindingStatus::New)
            && !entry.diagnostic.is_suppressed()
            && (entry.diagnostic.rule_id() == "BBT038"
                || lint_options
                    .fail_on()
                    .is_blocking(entry.diagnostic.severity()))
    }) {
        EXIT_DIFFERENCES
    } else {
        0
    }
}

struct AnalyzedLintInput {
    label: String,
    path: Option<PathBuf>,
    original: String,
    fixed: String,
    diagnostics: Vec<LintDiagnostic>,
    fixes_applied: usize,
}

struct SemanticLintAnalysis {
    context: BuildContext,
    report: SemanticReport,
}

struct ReportedDiagnostic {
    label: String,
    diagnostic: LintDiagnostic,
    baseline_status: BaselineFindingStatus,
}

struct AppliedFixSummary {
    path: String,
    count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BaselineFindingStatus {
    New,
    Existing,
}

#[derive(Clone, Debug)]
struct BaselineEntry {
    id: String,
    rule_id: String,
    path: String,
    shape: String,
    source_digest: String,
    occurrence: usize,
}

#[derive(Clone, Debug, Default)]
struct BaselineSummary {
    path: String,
    existing: usize,
    new: usize,
    stale: usize,
    profile: String,
    refreshed: bool,
}

const BASELINE_SCHEMA: u64 = 1;
const BASELINE_MAX_BYTES: u64 = 10 * 1024 * 1024;

fn rule_fingerprints() -> BTreeMap<String, String> {
    lint_rules()
        .iter()
        .map(|rule| {
            let profile_text = rule
                .profiles()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            (
                rule.id().to_owned(),
                stable_digest(&format!(
                    "{}\0{}\0{}\0{}\0{}",
                    rule.id(),
                    rule.name(),
                    rule.description(),
                    rule.severity(),
                    profile_text
                )),
            )
        })
        .collect()
}

fn stable_digest(value: &str) -> String {
    // Four independent FNV-1a lanes provide a deterministic, fixed-width
    // identity without making the lint binary depend on a crypto runtime.
    let mut lanes = [
        0xcbf29ce484222325u64,
        0x84222325cbf29ce4u64,
        0x9e3779b185ebca87u64,
        0xd6e8feb86659fd93u64,
    ];
    for byte in value.as_bytes() {
        for (index, lane) in lanes.iter_mut().enumerate() {
            *lane ^= u64::from(*byte).wrapping_add((index as u64) * 17);
            *lane = lane.wrapping_mul(0x100000001b3);
            *lane ^= *lane >> 29;
        }
    }
    format!(
        "{:016x}{:016x}{:016x}{:016x}",
        lanes[0], lanes[1], lanes[2], lanes[3]
    )
}

fn normalized_path_identity(label: &str) -> String {
    let path = Path::new(label);
    let path = if path.is_absolute() {
        std::env::current_dir()
            .ok()
            .and_then(|current| path.strip_prefix(current).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            std::path::Component::CurDir => None,
            _ => Some("<external>"),
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn diagnostic_shape(diagnostic: &LintDiagnostic) -> String {
    let mut shape = String::new();
    let mut previous_space = false;
    for character in diagnostic.message().chars() {
        if character.is_ascii_digit() {
            if !shape.ends_with('#') {
                shape.push('#');
            }
        } else if character.is_ascii_alphanumeric() || character == '_' {
            shape.push(character.to_ascii_lowercase());
            previous_space = false;
        } else if !previous_space && !shape.is_empty() {
            shape.push(' ');
            previous_space = true;
        }
    }
    format!("{}:{}", diagnostic.rule_id(), shape.trim())
}

fn source_anchor(source: Option<&str>, line: usize) -> String {
    source
        .and_then(|source| source.lines().nth(line.saturating_sub(1)))
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

fn baseline_entries(
    diagnostics: &[ReportedDiagnostic],
    sources: &BTreeMap<String, String>,
) -> Vec<BaselineEntry> {
    let mut occurrences = BTreeMap::<String, usize>::new();
    let mut entries = Vec::with_capacity(diagnostics.len());
    for entry in diagnostics {
        let path = normalized_path_identity(&entry.label);
        let shape = diagnostic_shape(&entry.diagnostic);
        let anchor = source_anchor(
            sources.get(&entry.label).map(String::as_str),
            entry.diagnostic.line(),
        );
        let source_digest = stable_digest(&anchor);
        let key = format!(
            "{}\0{}\0{}\0{}",
            entry.diagnostic.rule_id(),
            path,
            shape,
            source_digest
        );
        let occurrence = occurrences.entry(key.clone()).or_default();
        let current_occurrence = *occurrence;
        *occurrence += 1;
        let id = stable_digest(&format!(
            "bbtidy-finding-v1\0{}\0{}\0{}\0{}\0{}",
            entry.diagnostic.rule_id(),
            path,
            shape,
            source_digest,
            current_occurrence
        ));
        entries.push(BaselineEntry {
            id,
            rule_id: entry.diagnostic.rule_id().to_owned(),
            path,
            shape,
            source_digest,
            occurrence: current_occurrence,
        });
    }
    entries
}

fn baseline_entry_json(entry: &BaselineEntry) -> Value {
    json!({
        "id": entry.id,
        "rule_id": entry.rule_id,
        "path": entry.path,
        "shape": entry.shape,
        "source_digest": entry.source_digest,
        "occurrence": entry.occurrence,
    })
}

fn write_baseline_file(
    path: &Path,
    profile: LintProfile,
    entries: &[BaselineEntry],
) -> Result<(), String> {
    let value = json!({
        "schema": BASELINE_SCHEMA,
        "profile": profile.to_string(),
        "catalog_version": 1,
        "fingerprint_version": 1,
        "rule_fingerprints": rule_fingerprints(),
        "findings": entries.iter().map(baseline_entry_json).collect::<Vec<_>>(),
    });
    let serialized = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    if serialized.len() as u64 > BASELINE_MAX_BYTES {
        return Err("baseline exceeds the 10 MiB safety limit".to_owned());
    }
    fs::write(path, serialized)
        .map_err(|error| format!("could not write baseline {}: {error}", path.display()))
}

fn read_baseline_file(path: &Path, profile: LintProfile) -> Result<Vec<BaselineEntry>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("could not read baseline {}: {error}", path.display()))?;
    if metadata.len() > BASELINE_MAX_BYTES {
        return Err("baseline exceeds the 10 MiB safety limit".to_owned());
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("could not read baseline {}: {error}", path.display()))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid baseline JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "baseline must be a JSON object".to_owned())?;
    if object.get("schema").and_then(Value::as_u64) != Some(BASELINE_SCHEMA) {
        return Err("unsupported baseline schema".to_owned());
    }
    if object.get("profile").and_then(Value::as_str) != Some(&profile.to_string()) {
        return Err(
            "baseline profile differs; refresh it explicitly for the selected profile".to_owned(),
        );
    }
    let fingerprints = object
        .get("rule_fingerprints")
        .and_then(Value::as_object)
        .ok_or_else(|| "baseline is missing rule_fingerprints".to_owned())?;
    let current_fingerprints = rule_fingerprints();
    for (rule_id, expected) in &current_fingerprints {
        if fingerprints.get(rule_id).and_then(Value::as_str) != Some(expected.as_str()) {
            return Err(format!(
                "baseline rule fingerprint changed for {rule_id}; refresh it explicitly"
            ));
        }
    }
    if fingerprints.len() != current_fingerprints.len() {
        return Err("baseline rule catalog is incomplete; refresh it explicitly".to_owned());
    }
    let findings = object
        .get("findings")
        .and_then(Value::as_array)
        .ok_or_else(|| "baseline is missing findings".to_owned())?;
    let mut ids = BTreeSet::new();
    let mut entries = Vec::with_capacity(findings.len());
    for finding in findings {
        let item = finding
            .as_object()
            .ok_or_else(|| "baseline finding must be an object".to_owned())?;
        let string_field = |name: &str| {
            item.get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("baseline finding field '{name}' must be a string"))
        };
        let id = string_field("id")?.to_owned();
        if id.len() != 64
            || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !ids.insert(id.clone())
        {
            return Err(
                "baseline finding IDs must be unique 64-character hexadecimal strings".to_owned(),
            );
        }
        let path_value = string_field("path")?;
        let path = Path::new(path_value);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(
                "baseline paths must be repository-relative and cannot contain '..'".to_owned(),
            );
        }
        let source_digest = string_field("source_digest")?.to_owned();
        if source_digest.len() != 64 || !source_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(
                "baseline source digests must be 64-character hexadecimal strings".to_owned(),
            );
        }
        let occurrence = item
            .get("occurrence")
            .and_then(Value::as_u64)
            .ok_or_else(|| "baseline occurrence must be an integer".to_owned())?
            as usize;
        let rule_id = string_field("rule_id")?.to_owned();
        if !lint_rules().iter().any(|rule| rule.id() == rule_id) {
            return Err(format!("baseline finding contains unknown rule {rule_id}"));
        }
        entries.push(BaselineEntry {
            id,
            rule_id,
            path: path_value.to_owned(),
            shape: string_field("shape")?.to_owned(),
            source_digest,
            occurrence,
        });
    }
    Ok(entries)
}

fn write_text_diagnostic(
    stdout: &mut impl Write,
    label: &str,
    diagnostic: &LintDiagnostic,
    show_fixes: bool,
) -> io::Result<()> {
    writeln!(
        stdout,
        "{}:{}:{}: {}[{}]{}: {}",
        label,
        diagnostic.line(),
        diagnostic.column(),
        diagnostic.severity(),
        diagnostic.rule_id(),
        if diagnostic.is_suppressed() {
            " (suppressed)"
        } else {
            ""
        },
        diagnostic.message()
    )?;
    if show_fixes {
        if let Some(help) = diagnostic.help() {
            writeln!(stdout, "  help: {help}")?;
        }
        for fix in diagnostic.fixes() {
            writeln!(
                stdout,
                "  fix: {} (bytes {}..{})",
                fix.message(),
                fix.range().start(),
                fix.range().end()
            )?;
        }
    }
    Ok(())
}

fn write_lint_report(
    output: LintOutput,
    output_version: u8,
    profile: LintProfile,
    diagnostics: &[ReportedDiagnostic],
    applied_fixes: &[AppliedFixSummary],
    baseline: Option<&BaselineSummary>,
    suppression: LintSuppressionSummary,
    semantic_report: Option<&SemanticLintAnalysis>,
    execution: Option<&BitBakeExecutionStats>,
    stdout: &mut impl Write,
) -> io::Result<()> {
    let report = match output {
        LintOutput::Text => unreachable!("text reports are streamed directly"),
        LintOutput::Json => json_report(
            output_version,
            profile,
            diagnostics,
            applied_fixes,
            baseline,
            suppression,
            semantic_report,
            execution,
        ),
        LintOutput::Sarif => sarif_report(
            profile,
            diagnostics,
            applied_fixes,
            baseline,
            suppression,
            semantic_report,
            execution,
        ),
    };
    let serialized = serde_json::to_vec_pretty(&report)
        .map_err(|error| io::Error::other(format!("could not serialize diagnostics: {error}")))?;
    stdout.write_all(&serialized)?;
    stdout.write_all(b"\n")
}

fn json_report(
    output_version: u8,
    profile: LintProfile,
    diagnostics: &[ReportedDiagnostic],
    applied_fixes: &[AppliedFixSummary],
    baseline: Option<&BaselineSummary>,
    suppression: LintSuppressionSummary,
    semantic_report: Option<&SemanticLintAnalysis>,
    execution: Option<&BitBakeExecutionStats>,
) -> Value {
    let mut report = json!({
        "version": output_version,
        "profile": profile.to_string(),
        "catalog_version": 1,
        "rules": lint_rules().iter().map(json_rule).collect::<Vec<_>>(),
        "diagnostics": diagnostics.iter().map(json_diagnostic).collect::<Vec<_>>(),
        "fixes_applied": applied_fixes.iter().map(|fix| json!({
            "path": fix.path,
            "count": fix.count,
        })).collect::<Vec<_>>(),
    });
    if let Some(semantic_report) = semantic_report {
        report["semantic"] = semantic_summary(semantic_report);
    }
    if let Some(execution) = execution {
        report["execution"] = json!(execution);
    }
    if let Some(baseline) = baseline {
        report["baseline"] = json!({
            "path": baseline.path,
            "profile": baseline.profile,
            "existing": baseline.existing,
            "new": baseline.new,
            "stale": baseline.stale,
            "refreshed": baseline.refreshed,
        });
    }
    report["suppressions"] = json!({
        "directives": suppression.directives,
        "malformed": suppression.malformed,
        "unknown_rules": suppression.unknown_rules,
        "suppressed_findings": diagnostics.iter().filter(|entry| entry.diagnostic.is_suppressed()).count(),
    });
    report
}

fn json_rule(rule: &bbtidy::LintRule) -> Value {
    json!({
        "id": rule.id(),
        "name": rule.name(),
        "description": rule.description(),
        "severity": rule.severity().to_string(),
        "fixable": rule.fixable(),
        "profiles": rule.profiles().iter().map(ToString::to_string).collect::<Vec<_>>(),
    })
}

fn semantic_summary(analysis: &SemanticLintAnalysis) -> Value {
    let report = &analysis.report;
    json!({
        "bitbake": report.bitbake(),
        "bitbake_version": report.bitbake_version(),
        "project_dir": analysis.context.project_dir(),
        "build_dir": report.build_dir(),
        "build_context_source": analysis.context.source(),
        "requested_targets": report.requested_targets(),
        "requested_variables": report.requested_variables(),
        "targets": report.requested_targets(),
        "parse_succeeded": report.parse_succeeded(),
        "target_queries_succeeded": report.target_queries_succeeded(),
        "analysis_succeeded": report.analysis_succeeded(),
        "diagnostics": report.diagnostics(),
        "environments": report.environments(),
        "target_results": report.target_results(),
        "build_analysis": report.build_analysis(),
        "execution": report.execution(),
    })
}

fn json_diagnostic(entry: &ReportedDiagnostic) -> Value {
    let diagnostic = &entry.diagnostic;
    let rule = lint_rules()
        .iter()
        .find(|rule| rule.id() == diagnostic.rule_id());
    json!({
        "path": entry.label,
        "line": diagnostic.line(),
        "column": diagnostic.column(),
        "severity": diagnostic.severity().to_string(),
        "rule_id": diagnostic.rule_id(),
        "rule_description": rule.map(|rule| rule.description()),
        "origin": if diagnostic.rule_id() == "BBT019" { "authoritative" } else { "static" },
        "message": diagnostic.message(),
        "end_line": diagnostic.end_line(),
        "end_column": diagnostic.end_column(),
        "range": {
            "start_byte": diagnostic.range().start(),
            "end_byte": diagnostic.range().end(),
        },
        "help": diagnostic.help(),
        "fixable": diagnostic.is_fixable(),
        "suppressed": diagnostic.is_suppressed(),
        "suppression_reason": diagnostic.suppression_reason(),
        "baseline": match entry.baseline_status {
            BaselineFindingStatus::New => "new",
            BaselineFindingStatus::Existing => "existing",
        },
        "fixes": diagnostic.fixes().iter().map(|fix| json!({
            "start_byte": fix.range().start(),
            "end_byte": fix.range().end(),
            "replacement": fix.replacement(),
            "message": fix.message(),
        })).collect::<Vec<_>>(),
    })
}

fn sarif_report(
    profile: LintProfile,
    diagnostics: &[ReportedDiagnostic],
    applied_fixes: &[AppliedFixSummary],
    baseline: Option<&BaselineSummary>,
    suppression: LintSuppressionSummary,
    semantic_report: Option<&SemanticLintAnalysis>,
    execution: Option<&BitBakeExecutionStats>,
) -> Value {
    let rules = lint_rules()
        .iter()
        .map(|rule| {
            json!({
                "id": rule.id(),
                "name": rule.name(),
                "shortDescription": {"text": rule.description()},
                "defaultConfiguration": {"level": sarif_level(rule.severity())},
                "properties": {
                    "fixable": rule.fixable(),
                    "profiles": rule.profiles().iter().map(ToString::to_string).collect::<Vec<_>>(),
                },
                "helpUri": format!("https://github.com/jorisguex/bbtidy/blob/main/README.md#{}", rule.id().to_lowercase()),
            })
        })
        .collect::<Vec<_>>();
    let results = diagnostics
        .iter()
        .map(|entry| {
            let diagnostic = &entry.diagnostic;
            let mut properties = serde_json::Map::from_iter([
                ("startByte".to_owned(), json!(diagnostic.range().start())),
                ("endByte".to_owned(), json!(diagnostic.range().end())),
                ("fixable".to_owned(), json!(diagnostic.is_fixable())),
            ]);
            if let Some(help) = diagnostic.help() {
                properties.insert("help".to_owned(), json!(help));
            }
            properties.insert("suppressed".to_owned(), json!(diagnostic.is_suppressed()));
            properties.insert(
                "origin".to_owned(),
                json!(if diagnostic.rule_id() == "BBT019" {
                    "authoritative"
                } else {
                    "static"
                }),
            );
            properties.insert(
                "baselineStatus".to_owned(),
                json!(match entry.baseline_status {
                    BaselineFindingStatus::New => "new",
                    BaselineFindingStatus::Existing => "existing",
                }),
            );
            if let Some(reason) = diagnostic.suppression_reason() {
                properties.insert("suppressionReason".to_owned(), json!(reason));
            }
            json!({
                "ruleId": diagnostic.rule_id(),
                "level": sarif_level(diagnostic.severity()),
                "message": {"text": diagnostic.message()},
                "properties": properties,
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": {"uri": entry.label},
                        "region": {
                            "startLine": diagnostic.line(),
                            "startColumn": diagnostic.column(),
                            "endLine": diagnostic.end_line(),
                            "endColumn": diagnostic.end_column(),
                        },
                    },
                }],
                "fixes": diagnostic.fixes().iter().map(|fix| json!({
                    "description": {"text": fix.message()},
                    "artifactChanges": [{
                        "artifactLocation": {"uri": entry.label},
                        "replacements": [{
                            "deletedRegion": {
                                "startLine": diagnostic.line(),
                                "startColumn": diagnostic.column(),
                                "endLine": diagnostic.end_line(),
                                "endColumn": diagnostic.end_column(),
                            },
                            "insertedContent": {"text": fix.replacement()},
                        }],
                    }],
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    let mut report = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "bbtidy",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/jorisguex/bbtidy",
                    "properties": {"profile": profile.to_string(), "catalogVersion": 1},
                    "rules": rules,
                },
            },
            "results": results,
            "properties": {"fixesApplied": applied_fixes.iter().map(|fix| json!({
                "path": fix.path,
                "count": fix.count,
            })).collect::<Vec<_>>()},
        }],
    });
    if let Some(semantic_report) = semantic_report {
        report["runs"][0]["properties"]["semantic"] = semantic_summary(semantic_report);
    }
    if let Some(execution) = execution {
        report["runs"][0]["properties"]["execution"] = json!(execution);
    }
    if let Some(baseline) = baseline {
        report["runs"][0]["properties"]["baseline"] = json!({
            "path": baseline.path,
            "profile": baseline.profile,
            "existing": baseline.existing,
            "new": baseline.new,
            "stale": baseline.stale,
            "refreshed": baseline.refreshed,
        });
    }
    report["runs"][0]["properties"]["suppressions"] = json!({
        "directives": suppression.directives,
        "malformed": suppression.malformed,
        "unknownRules": suppression.unknown_rules,
        "suppressedFindings": diagnostics.iter().filter(|entry| entry.diagnostic.is_suppressed()).count(),
    });
    report
}

fn sarif_level(severity: LintSeverity) -> &'static str {
    match severity {
        LintSeverity::Info => "note",
        LintSeverity::Warning => "warning",
        LintSeverity::Error => "error",
    }
}

fn effective_bitbake_limits(
    configured: &BitBakeExecutionLimits,
    overrides: &BitBakeLimitArgs,
) -> BitBakeExecutionLimits {
    BitBakeExecutionLimits {
        command_timeout: std::time::Duration::from_secs(
            overrides
                .command_timeout_seconds
                .unwrap_or(configured.command_timeout.as_secs()),
        ),
        total_timeout: std::time::Duration::from_secs(
            overrides
                .total_timeout_seconds
                .unwrap_or(configured.total_timeout.as_secs()),
        ),
        max_stdout_bytes: overrides
            .max_stdout_bytes
            .unwrap_or(configured.max_stdout_bytes),
        max_stderr_bytes: overrides
            .max_stderr_bytes
            .unwrap_or(configured.max_stderr_bytes),
        max_commands: overrides.max_commands.unwrap_or(configured.max_commands),
        max_recipe_queries: overrides
            .max_recipe_queries
            .unwrap_or(configured.max_recipe_queries),
    }
}

fn install_cli_cancellation_handler() {
    #[cfg(unix)]
    {
        static INSTALLED: std::sync::Once = std::sync::Once::new();
        INSTALLED.call_once(|| unsafe {
            libc::signal(libc::SIGINT, cli_sigint_handler as libc::sighandler_t);
        });
    }
}

#[cfg(unix)]
extern "C" fn cli_sigint_handler(_signal: libc::c_int) {
    CLI_CANCELLED.store(true, Ordering::SeqCst);
}

fn run_lex(args: InputArgs, config: &Config) -> i32 {
    let inputs = match resolve_inputs(&args.paths, config) {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };
    let multiple_inputs = inputs.len() > 1;
    let mut had_error = false;

    for (index, input) in inputs.iter().enumerate() {
        let (label, text) = match read_input(input) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("error: {error}");
                had_error = true;
                continue;
            }
        };

        if multiple_inputs {
            if index > 0 {
                println!();
            }
            println!("==> {label} <==");
        }

        let mut lexer = Token::lexer(&text);
        while let Some(token) = lexer.next() {
            let span = lexer.span();
            let (line, column) = get_line_col(&text, span.start);
            match token {
                Ok(token) => println!(
                    "{:<20} {:?} {}:{} {:?}",
                    format!("{token:?}"),
                    span,
                    line,
                    column,
                    lexer.slice()
                ),
                Err(_) => {
                    println!(
                        "{:<20} {:?} {}:{} {:?}",
                        "Error",
                        span,
                        line,
                        column,
                        lexer.slice()
                    );
                    had_error = true;
                }
            }
        }
    }

    if had_error { EXIT_ERROR } else { 0 }
}

fn run_syntax_stats(args: SyntaxStatsArgs, config: &Config) -> i32 {
    let inputs = match resolve_inputs(&args.inputs.paths, config) {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };
    let mut total_nodes = 0_u64;
    let mut structured_nodes = 0_u64;
    let mut trivia_nodes = 0_u64;
    let mut unknown_nodes = 0_u64;
    let mut unknown_bytes = 0_u64;

    let mut details = Vec::new();
    for input in &inputs {
        let (label, text) = match read_input(input) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("error: {error}");
                return EXIT_ERROR;
            }
        };
        let tree = match parse(&text) {
            Ok(tree) => tree,
            Err(error) => {
                eprintln!("error: could not parse {label}: {error}");
                return EXIT_ERROR;
            }
        };
        let before_unknowns = if args.details {
            unknown_node_details(&text, tree.nodes())
        } else {
            Vec::new()
        };
        let after_unknowns = if args.details {
            match format(&text) {
                Ok(formatted) => match parse(&formatted) {
                    Ok(formatted_tree) => unknown_node_details(&formatted, formatted_tree.nodes()),
                    Err(_) => Vec::new(),
                },
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        for node in tree.nodes() {
            total_nodes += 1;
            match node.kind() {
                SyntaxKind::Blank | SyntaxKind::Comment => trivia_nodes += 1,
                SyntaxKind::Unknown => {
                    unknown_nodes += 1;
                    unknown_bytes += node.text().len() as u64;
                }
                SyntaxKind::Assignment(_)
                | SyntaxKind::Directive(_)
                | SyntaxKind::Function(_)
                | SyntaxKind::PythonDefinition(_) => structured_nodes += 1,
            }
        }
        if args.details {
            details.push(json!({
                "path": label,
                "before": before_unknowns,
                "after": after_unknowns,
            }));
        }
    }

    let mut report = json!({
        "version": 1,
        "files": inputs.len(),
        "total_nodes": total_nodes,
        "structured_nodes": structured_nodes,
        "trivia_nodes": trivia_nodes,
        "unknown_nodes": unknown_nodes,
        "unknown_bytes": unknown_bytes,
    });
    if args.details {
        report["details"] = Value::Array(details);
    }
    match serde_json::to_string_pretty(&report) {
        Ok(report) => {
            println!("{report}");
            0
        }
        Err(error) => {
            eprintln!("error: could not serialize syntax statistics: {error}");
            EXIT_ERROR
        }
    }
}

fn unknown_node_details(source: &str, nodes: &[bbtidy::SyntaxNode<'_>]) -> Vec<Value> {
    nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            if !matches!(node.kind(), SyntaxKind::Unknown) {
                return None;
            }
            let start = node.range().start();
            let end = node.range().end();
            let excerpt = source[start..end]
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(160)
                .collect::<String>();
            Some(json!({
                "start_byte": start,
                "end_byte": end,
                "length": node.range().len(),
                "excerpt": excerpt,
                "previous_kind": index.checked_sub(1).map(|i| syntax_kind_name(nodes[i].kind())),
                "next_kind": nodes.get(index + 1).map(|next| syntax_kind_name(next.kind())),
            }))
        })
        .collect()
}

fn syntax_kind_name(kind: &SyntaxKind<'_>) -> &'static str {
    match kind {
        SyntaxKind::Blank => "blank",
        SyntaxKind::Comment => "comment",
        SyntaxKind::Assignment(_) => "assignment",
        SyntaxKind::Directive(_) => "directive",
        SyntaxKind::Function(_) => "function",
        SyntaxKind::PythonDefinition(_) => "python_definition",
        SyntaxKind::Unknown => "unknown",
    }
}

fn validate_input_limits(inputs: &[Input], limits: SafetyOptions) -> Result<(), String> {
    if limits.max_files == 0 {
        return Err("safety limit max_files must be greater than zero".to_owned());
    }
    if limits.max_bytes == 0 {
        return Err("safety limit max_bytes must be greater than zero".to_owned());
    }
    if inputs.len() > limits.max_files {
        return Err(format!(
            "safety limit exceeded: {} files discovered, maximum is {}",
            inputs.len(),
            limits.max_files
        ));
    }

    let mut bytes = 0_u64;
    for input in inputs {
        let Input::File(path) = input else {
            continue;
        };
        let size = fs::symlink_metadata(path)
            .map_err(|error| format!("could not inspect {}: {error}", path.display()))?
            .len();
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| "safety limit exceeded: source size overflowed".to_owned())?;
    }
    if bytes > limits.max_bytes {
        return Err(format!(
            "safety limit exceeded: {} bytes discovered, maximum is {}",
            bytes, limits.max_bytes
        ));
    }
    Ok(())
}

fn validate_formatted_limits(
    inputs: &[FormattedInput],
    limits: SafetyOptions,
) -> Result<(), String> {
    if inputs.len() > limits.max_files {
        return Err(format!(
            "safety limit exceeded: {} files read, maximum is {}",
            inputs.len(),
            limits.max_files
        ));
    }
    let bytes = inputs.iter().try_fold(0_u64, |total, input| {
        total
            .checked_add(input.original.len() as u64)
            .ok_or_else(|| "safety limit exceeded: source size overflowed".to_owned())
    })?;
    if bytes > limits.max_bytes {
        return Err(format!(
            "safety limit exceeded: {} bytes read, maximum is {}",
            bytes, limits.max_bytes
        ));
    }
    Ok(())
}

fn resolve_inputs(paths: &[PathBuf], config: &Config) -> Result<Vec<Input>, String> {
    let stdin_count = paths.iter().filter(|path| path.as_os_str() == "-").count();
    if stdin_count > 1 {
        return Err("standard input ('-') may only be specified once".to_owned());
    }
    if stdin_count == 1 && paths.len() > 1 {
        return Err("standard input ('-') cannot be combined with other inputs".to_owned());
    }
    if stdin_count == 1 {
        return Ok(vec![Input::Stdin]);
    }

    let mut files = BTreeSet::new();
    for path in paths {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
        if config.is_excluded(path) {
            continue;
        }
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "{} is a symbolic link; refusing to process it",
                path.display()
            ));
        } else if metadata.is_dir() {
            collect_directory(path, path, config, &mut files)?;
        } else if metadata.is_file() {
            files.insert(path.clone());
        } else {
            return Err(format!(
                "{} is not a regular file or directory",
                path.display()
            ));
        }
    }

    if files.is_empty() {
        return Err("no BitBake files found in the supplied directories".to_owned());
    }

    Ok(files.into_iter().map(Input::File).collect())
}

fn collect_directory(
    directory: &Path,
    root: &Path,
    config: &Config,
    files: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("could not read directory {}: {error}", directory.display()))?;
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not read directory {}: {error}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if config.is_excluded(&path) {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_directory(&path, root, config, files)?;
        } else if file_type.is_file() && is_bitbake_file(&path, root) {
            files.insert(path);
        }
    }

    Ok(())
}

fn is_bitbake_file(path: &Path, root: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| BITBAKE_EXTENSIONS.contains(extension));
    let Some(extension) = extension else {
        return false;
    };

    let relative = path.strip_prefix(root).unwrap_or(path);
    if root.file_name().is_some_and(|name| name == "files")
        || relative
            .components()
            .any(|component| component.as_os_str() == "files")
    {
        return false;
    }

    extension != "conf"
        || path.ancestors().skip(1).any(|ancestor| {
            ancestor.file_name().is_some_and(|name| name == "conf")
                && ancestor.join("layer.conf").is_file()
        })
}

fn format_inputs(
    inputs: &[Input],
    options: &bbtidy::FormatOptions,
) -> Result<Vec<FormattedInput>, ()> {
    let mut formatted_inputs = Vec::with_capacity(inputs.len());
    let mut had_error = false;

    for input in inputs {
        let (label, original) = match read_input(input) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("error: {error}");
                had_error = true;
                continue;
            }
        };
        match format_with_options(&original, options) {
            Ok(formatted) => formatted_inputs.push(FormattedInput {
                label,
                path: match input {
                    Input::Stdin => None,
                    Input::File(path) => Some(path.clone()),
                },
                original,
                formatted,
            }),
            Err(error) => {
                eprintln!("error: could not format {label}: {error}");
                had_error = true;
            }
        }
    }

    if had_error {
        Err(())
    } else {
        Ok(formatted_inputs)
    }
}

fn read_input(input: &Input) -> Result<(String, String), String> {
    match input {
        Input::Stdin => {
            let mut text = String::new();
            io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| format!("could not read standard input: {error}"))?;
            Ok(("<stdin>".to_owned(), text))
        }
        Input::File(path) => fs::read_to_string(path)
            .map(|text| (path.display().to_string(), text))
            .map_err(|error| format!("could not read {}: {error}", path.display())),
    }
}

struct PendingWrite {
    path: PathBuf,
    original: String,
    temporary_path: PathBuf,
    backup_path: PathBuf,
}

fn write_inputs(inputs: &[FormattedInput]) -> i32 {
    let changed = inputs
        .iter()
        .filter(|input| input.original != input.formatted)
        .collect::<Vec<_>>();
    if changed.is_empty() {
        return 0;
    }

    let pending = match stage_writes(&changed) {
        Ok(pending) => pending,
        Err(error) => {
            eprintln!("error: could not prepare repository-wide write: {error}");
            return EXIT_ERROR;
        }
    };
    if let Err(error) = commit_writes(&pending) {
        eprintln!("error: could not commit repository-wide write: {error}");
        return EXIT_ERROR;
    }

    for input in changed {
        println!("formatted: {}", input.label);
    }
    0
}

fn stage_writes(inputs: &[&FormattedInput]) -> io::Result<Vec<PendingWrite>> {
    let mut pending = Vec::with_capacity(inputs.len());

    for input in inputs {
        let path = input
            .path
            .as_deref()
            .expect("write mode rejects standard input")
            .to_path_buf();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            cleanup_pending(&pending);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing to replace symbolic link {}", path.display()),
            ));
        }
        let current = fs::read_to_string(&path)?;
        if current != input.original {
            cleanup_pending(&pending);
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                format!("{} changed while it was being formatted", path.display()),
            ));
        }

        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
        let temporary_path = match create_temporary_file(
            parent,
            file_name,
            "output",
            input.formatted.as_bytes(),
            &metadata,
        ) {
            Ok(path) => path,
            Err(error) => {
                cleanup_pending(&pending);
                return Err(error);
            }
        };
        let backup_path = match create_temporary_file(
            parent,
            file_name,
            "backup",
            input.original.as_bytes(),
            &metadata,
        ) {
            Ok(path) => path,
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                cleanup_pending(&pending);
                return Err(error);
            }
        };
        pending.push(PendingWrite {
            path,
            original: input.original.clone(),
            temporary_path,
            backup_path,
        });
    }

    Ok(pending)
}

fn create_temporary_file(
    parent: &Path,
    file_name: &std::ffi::OsStr,
    purpose: &str,
    contents: &[u8],
    metadata: &fs::Metadata,
) -> io::Result<PathBuf> {
    for attempt in 0..100 {
        let temporary_name = format!(
            ".{}.bbtidy.{}.{}.{}.tmp",
            file_name.to_string_lossy(),
            process::id(),
            purpose,
            attempt
        );
        let temporary_path = parent.join(temporary_name);
        let mut temporary_file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };

        let result = (|| {
            temporary_file.write_all(contents)?;
            temporary_file.flush()?;
            temporary_file.sync_all()?;
            fs::set_permissions(&temporary_path, metadata.permissions())?;
            temporary_file.sync_all()
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        return Ok(temporary_path);
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a temporary output file",
    ))
}

fn commit_writes(pending: &[PendingWrite]) -> io::Result<()> {
    let mut committed = 0;
    for item in pending {
        let preflight = (|| {
            let metadata = fs::symlink_metadata(&item.path)?;
            if metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "{} became a symbolic link during formatting",
                        item.path.display()
                    ),
                ));
            }
            let current = fs::read_to_string(&item.path)?;
            if current != item.original {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    format!(
                        "{} changed while writes were being committed",
                        item.path.display()
                    ),
                ));
            }
            Ok(())
        })();
        if let Err(error) = preflight {
            let rollback = rollback_writes(pending, committed);
            return Err(combine_transaction_errors(error, rollback));
        }

        if let Err(error) = fs::rename(&item.temporary_path, &item.path) {
            let rollback = rollback_writes(pending, committed);
            return Err(combine_transaction_errors(error, rollback));
        }
        committed += 1;
        if let Err(error) = sync_parent(item.path.parent().unwrap_or_else(|| Path::new("."))) {
            let rollback = rollback_writes(pending, committed);
            return Err(combine_transaction_errors(error, rollback));
        }
    }

    for item in pending {
        fs::remove_file(&item.backup_path)?;
        sync_parent(item.path.parent().unwrap_or_else(|| Path::new(".")))?;
    }
    Ok(())
}

fn rollback_writes(pending: &[PendingWrite], committed: usize) -> io::Result<()> {
    let mut first_error = None;
    for item in pending[..committed].iter().rev() {
        let result = (|| {
            fs::remove_file(&item.path)?;
            fs::rename(&item.backup_path, &item.path)?;
            sync_parent(item.path.parent().unwrap_or_else(|| Path::new(".")))
        })();
        if let Err(error) = result {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    for item in &pending[committed..] {
        if let Err(error) = fs::remove_file(&item.temporary_path)
            && error.kind() != io::ErrorKind::NotFound
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Err(error) = fs::remove_file(&item.backup_path)
            && error.kind() != io::ErrorKind::NotFound
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn cleanup_pending(pending: &[PendingWrite]) {
    for item in pending {
        let _ = fs::remove_file(&item.temporary_path);
        let _ = fs::remove_file(&item.backup_path);
    }
}

fn combine_transaction_errors(error: io::Error, rollback: io::Result<()>) -> io::Error {
    match rollback {
        Ok(()) => error,
        Err(rollback_error) => {
            io::Error::other(format!("{error}; rollback also failed: {rollback_error}"))
        }
    }
}

fn sync_parent(parent: &Path) -> io::Result<()> {
    let directory = OpenOptions::new().read(true).open(parent)?;
    directory.sync_all()
}

fn print_diffs(inputs: &[FormattedInput]) -> i32 {
    let mut stdout = io::stdout().lock();

    for input in inputs {
        if input.original == input.formatted {
            continue;
        }

        let old_label = format!("a/{}", input.label);
        let new_label = format!("b/{}", input.label);
        let diff = TextDiff::from_lines(&input.original, &input.formatted)
            .unified_diff()
            .header(&old_label, &new_label)
            .to_string();
        if let Err(error) = stdout.write_all(diff.as_bytes()) {
            if error.kind() == io::ErrorKind::BrokenPipe {
                return 0;
            }
            eprintln!("error: could not write standard output: {error}");
            return EXIT_ERROR;
        }
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn transaction_rolls_back_when_a_later_file_changes() {
        let root = std::env::temp_dir().join(format!(
            "bbtidy-transaction-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let first_path = root.join("a.bb");
        let second_path = root.join("b.bb");
        fs::write(&first_path, "A=\"a\"\n").unwrap();
        fs::write(&second_path, "B=\"b\"\n").unwrap();
        let first = FormattedInput {
            label: first_path.display().to_string(),
            path: Some(first_path.clone()),
            original: "A=\"a\"\n".to_owned(),
            formatted: "A = \"a\"\n".to_owned(),
        };
        let second = FormattedInput {
            label: second_path.display().to_string(),
            path: Some(second_path.clone()),
            original: "B=\"b\"\n".to_owned(),
            formatted: "B = \"b\"\n".to_owned(),
        };

        let pending = stage_writes(&[&first, &second]).unwrap();
        fs::write(&second_path, "B=\"concurrent\"\n").unwrap();
        assert!(commit_writes(&pending).is_err());
        assert_eq!(fs::read_to_string(&first_path).unwrap(), "A=\"a\"\n");
        assert_eq!(
            fs::read_to_string(&second_path).unwrap(),
            "B=\"concurrent\"\n"
        );
        cleanup_pending(&pending);
        assert!(!fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".bbtidy.")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_bitbake_overrides_take_precedence_over_configured_limits() {
        let configured = BitBakeExecutionLimits::default();
        let overrides = BitBakeLimitArgs {
            command_timeout_seconds: Some(1),
            total_timeout_seconds: Some(2),
            max_stdout_bytes: Some(3),
            max_stderr_bytes: Some(4),
            max_commands: Some(5),
            max_recipe_queries: Some(6),
        };
        let effective = effective_bitbake_limits(&configured, &overrides);
        assert_eq!(effective.command_timeout.as_secs(), 1);
        assert_eq!(effective.total_timeout.as_secs(), 2);
        assert_eq!(effective.max_stdout_bytes, 3);
        assert_eq!(effective.max_stderr_bytes, 4);
        assert_eq!(effective.max_commands, 5);
        assert_eq!(effective.max_recipe_queries, 6);
    }
}

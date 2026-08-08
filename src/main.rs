use bbtidy::{
    BuildContext, BuildContextDiscoveryOptions, Config, LintDiagnostic, LintFailurePolicy,
    LintSeverity, SafetyOptions, SemanticOptions, SyntaxKind, Token, WorkspaceIndex,
    analyze_bitbake, apply_lint_fixes, discover_build_context_with_options, format_with_options,
    get_line_col, lint_rules, lint_with_options, lint_with_workspace, load_config, parse,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use logos::Logos;
use serde_json::{Value, json};
use similar::TextDiff;
use std::collections::BTreeSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;

const EXIT_DIFFERENCES: i32 = 1;
const EXIT_ERROR: i32 = 2;
const BITBAKE_EXTENSIONS: &[&str] = &["bb", "bbappend", "bbclass", "conf", "inc"];

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

    /// Check whether BitBake metadata is already formatted
    Check(InputArgs),

    /// Check BitBake metadata for lint findings
    Lint(LintArgs),

    /// Print the lexer token stream
    Lex(InputArgs),

    /// Run authoritative semantic analysis through BitBake
    Semantic(SemanticArgs),

    /// Report CST coverage metrics for the compatibility harness
    #[command(hide = true)]
    SyntaxStats(InputArgs),
}

#[derive(Args)]
struct FormatArgs {
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

    /// Set the minimum diagnostic severity that fails the command.
    #[arg(long, value_enum, value_name = "SEVERITY")]
    fail_on: Option<LintFailureArg>,

    /// Apply safe, machine-generated fixes and re-lint the resulting source.
    #[arg(long)]
    fix: bool,

    /// Include help and edit details in human-readable diagnostics.
    #[arg(long)]
    show_fixes: bool,

    #[command(flatten)]
    inputs: InputArgs,
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

    /// Select human-readable text or JSON output.
    #[arg(long, value_enum, default_value_t = SemanticOutput::Text, value_name = "FORMAT")]
    output: SemanticOutput,
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

#[derive(Args)]
struct InputArgs {
    /// Files or directories to process; use '-' to read standard input
    #[arg(required = true, value_name = "PATH")]
    paths: Vec<PathBuf>,
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
        Command::Check(args) => run_check(args, &config),
        Command::Lint(args) => run_lint(args, &config),
        Command::Lex(args) => run_lex(args, &config),
        Command::Semantic(args) => run_semantic(args, &config),
        Command::SyntaxStats(args) => run_syntax_stats(args, &config),
    };

    if exit_code != 0 {
        process::exit(exit_code);
    }
}

fn run_semantic(args: SemanticArgs, config: &Config) -> i32 {
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
    };
    let report = match analyze_bitbake(&options) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };

    if args.output == SemanticOutput::Json {
        let value = serde_json::json!({
            "version": 1,
            "bitbake_version": report.bitbake_version(),
            "project_dir": context.project_dir(),
            "build_dir": report.build_dir(),
            "build_context_source": context.source(),
            "parse_succeeded": report.parse_succeeded(),
            "target_queries_succeeded": report.target_queries_succeeded(),
            "analysis_succeeded": report.analysis_succeeded(),
            "diagnostics": report.diagnostics(),
            "environments": report.environments(),
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
        for environment in report.environments() {
            println!("Target {}:", environment.target());
            for (name, value) in environment.variables() {
                println!("  {name}={value}");
            }
        }
    }

    if report.analysis_succeeded() && !report.has_errors() {
        0
    } else {
        EXIT_DIFFERENCES
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

    if !args.write && !args.diff && inputs.len() != 1 {
        eprintln!(
            "error: formatting to standard output requires exactly one input; use --diff or --write for multiple inputs"
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

fn run_check(args: InputArgs, config: &Config) -> i32 {
    let inputs = match resolve_inputs(&args.paths, config) {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };
    let formatted_inputs = match format_inputs(&inputs, &config.format) {
        Ok(formatted_inputs) => formatted_inputs,
        Err(()) => return EXIT_ERROR,
    };

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
    let mut lint_options = config.lint.clone();
    if let Some(fail_on) = args.fail_on {
        lint_options.set_fail_on(fail_on.into());
    }
    let inputs = match resolve_inputs(&args.inputs.paths, config) {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };
    if args.fix && inputs.iter().any(|input| matches!(input, Input::Stdin)) {
        eprintln!("error: --fix cannot be used with standard input");
        return EXIT_ERROR;
    }
    if args.fix
        && let Err(error) = validate_input_limits(&inputs, config.safety)
    {
        eprintln!("error: {error}");
        return EXIT_ERROR;
    }
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

    let collected = analyzed
        .iter()
        .flat_map(|input| {
            input
                .diagnostics
                .iter()
                .cloned()
                .map(|diagnostic| ReportedDiagnostic {
                    label: input.label.clone(),
                    diagnostic,
                })
        })
        .collect::<Vec<_>>();
    let applied_fixes = analyzed
        .iter()
        .filter(|input| input.fixes_applied > 0)
        .map(|input| AppliedFixSummary {
            path: input.label.clone(),
            count: input.fixes_applied,
        })
        .collect::<Vec<_>>();

    let mut stdout = io::stdout().lock();
    if machine_output {
        if let Err(error) = write_lint_report(args.output, &collected, &applied_fixes, &mut stdout)
        {
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
            for diagnostic in &input.diagnostics {
                if let Err(error) =
                    write_text_diagnostic(&mut stdout, &input.label, diagnostic, args.show_fixes)
                {
                    if error.kind() == io::ErrorKind::BrokenPipe {
                        return 0;
                    }
                    eprintln!("error: could not write standard output: {error}");
                    return EXIT_ERROR;
                }
            }
        }
    }

    if lint_options.has_blocking_findings(
        &collected
            .iter()
            .map(|entry| entry.diagnostic.clone())
            .collect::<Vec<_>>(),
    ) {
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

struct ReportedDiagnostic {
    label: String,
    diagnostic: LintDiagnostic,
}

struct AppliedFixSummary {
    path: String,
    count: usize,
}

fn write_text_diagnostic(
    stdout: &mut impl Write,
    label: &str,
    diagnostic: &LintDiagnostic,
    show_fixes: bool,
) -> io::Result<()> {
    writeln!(
        stdout,
        "{}:{}:{}: {}[{}]: {}",
        label,
        diagnostic.line(),
        diagnostic.column(),
        diagnostic.severity(),
        diagnostic.rule_id(),
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
    diagnostics: &[ReportedDiagnostic],
    applied_fixes: &[AppliedFixSummary],
    stdout: &mut impl Write,
) -> io::Result<()> {
    let report = match output {
        LintOutput::Text => unreachable!("text reports are streamed directly"),
        LintOutput::Json => json_report(diagnostics, applied_fixes),
        LintOutput::Sarif => sarif_report(diagnostics, applied_fixes),
    };
    let serialized = serde_json::to_vec_pretty(&report)
        .map_err(|error| io::Error::other(format!("could not serialize diagnostics: {error}")))?;
    stdout.write_all(&serialized)?;
    stdout.write_all(b"\n")
}

fn json_report(diagnostics: &[ReportedDiagnostic], applied_fixes: &[AppliedFixSummary]) -> Value {
    json!({
        "version": 1,
        "diagnostics": diagnostics.iter().map(json_diagnostic).collect::<Vec<_>>(),
        "fixes_applied": applied_fixes.iter().map(|fix| json!({
            "path": fix.path,
            "count": fix.count,
        })).collect::<Vec<_>>(),
    })
}

fn json_diagnostic(entry: &ReportedDiagnostic) -> Value {
    let diagnostic = &entry.diagnostic;
    json!({
        "path": entry.label,
        "line": diagnostic.line(),
        "column": diagnostic.column(),
        "severity": diagnostic.severity().to_string(),
        "rule_id": diagnostic.rule_id(),
        "message": diagnostic.message(),
        "end_line": diagnostic.end_line(),
        "end_column": diagnostic.end_column(),
        "range": {
            "start_byte": diagnostic.range().start(),
            "end_byte": diagnostic.range().end(),
        },
        "help": diagnostic.help(),
        "fixable": diagnostic.is_fixable(),
        "fixes": diagnostic.fixes().iter().map(|fix| json!({
            "start_byte": fix.range().start(),
            "end_byte": fix.range().end(),
            "replacement": fix.replacement(),
            "message": fix.message(),
        })).collect::<Vec<_>>(),
    })
}

fn sarif_report(diagnostics: &[ReportedDiagnostic], applied_fixes: &[AppliedFixSummary]) -> Value {
    let rules = lint_rules()
        .iter()
        .map(|rule| {
            json!({
                "id": rule.id(),
                "name": rule.name(),
                "shortDescription": {"text": rule.description()},
                "defaultConfiguration": {"level": sarif_level(rule.severity())},
                "properties": {"fixable": rule.fixable()},
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

    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "bbtidy",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/jorisguex/bbtidy",
                    "rules": rules,
                },
            },
            "results": results,
            "properties": {"fixesApplied": applied_fixes.iter().map(|fix| json!({
                "path": fix.path,
                "count": fix.count,
            })).collect::<Vec<_>>()},
        }],
    })
}

fn sarif_level(severity: LintSeverity) -> &'static str {
    match severity {
        LintSeverity::Info => "note",
        LintSeverity::Warning => "warning",
        LintSeverity::Error => "error",
    }
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

fn run_syntax_stats(args: InputArgs, config: &Config) -> i32 {
    let inputs = match resolve_inputs(&args.paths, config) {
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
    }

    let report = json!({
        "version": 1,
        "files": inputs.len(),
        "total_nodes": total_nodes,
        "structured_nodes": structured_nodes,
        "trivia_nodes": trivia_nodes,
        "unknown_nodes": unknown_nodes,
        "unknown_bytes": unknown_bytes,
    });
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
        let size = fs::metadata(path)
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
        let metadata = fs::metadata(path)
            .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
        if metadata.is_dir() {
            collect_directory(path, path, config, &mut files)?;
        } else if metadata.is_file() && !config.is_excluded(path) {
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
}

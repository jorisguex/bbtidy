use bbtidy::{Config, Token, format_with_options, get_line_col, lint_with_options, load_config};
use clap::{Args, Parser, Subcommand};
use logos::Logos;
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
    Lint(InputArgs),

    /// Print the lexer token stream
    Lex(InputArgs),
}

#[derive(Args)]
struct FormatArgs {
    /// Rewrite files in place
    #[arg(long, conflicts_with = "diff")]
    write: bool,

    /// Print a unified diff instead of formatted source
    #[arg(long, conflicts_with = "write")]
    diff: bool,

    #[command(flatten)]
    inputs: InputArgs,
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
    };

    if exit_code != 0 {
        process::exit(exit_code);
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

    let formatted_inputs = match format_inputs(&inputs, &config.format) {
        Ok(formatted_inputs) => formatted_inputs,
        Err(()) => return EXIT_ERROR,
    };

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

fn run_lint(args: InputArgs, config: &Config) -> i32 {
    let inputs = match resolve_inputs(&args.paths, config) {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_ERROR;
        }
    };
    let mut had_findings = false;
    let mut had_error = false;
    let mut stdout = io::stdout().lock();

    for input in &inputs {
        let (label, text) = match read_input(input) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("error: {error}");
                had_error = true;
                continue;
            }
        };
        match lint_with_options(&text, &config.lint) {
            Ok(diagnostics) => {
                had_findings |= !diagnostics.is_empty();
                for diagnostic in diagnostics {
                    if let Err(error) = writeln!(
                        stdout,
                        "{}:{}:{}: {}[{}]: {}",
                        label,
                        diagnostic.line(),
                        diagnostic.column(),
                        diagnostic.severity(),
                        diagnostic.rule_id(),
                        diagnostic.message()
                    ) {
                        if error.kind() == io::ErrorKind::BrokenPipe {
                            return 0;
                        }
                        eprintln!("error: could not write standard output: {error}");
                        return EXIT_ERROR;
                    }
                }
            }
            Err(error) => {
                eprintln!("error: could not lint {label}: {error}");
                had_error = true;
            }
        }
    }

    if had_error {
        EXIT_ERROR
    } else if had_findings {
        EXIT_DIFFERENCES
    } else {
        0
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

fn write_inputs(inputs: &[FormattedInput]) -> i32 {
    for input in inputs {
        if input.original == input.formatted {
            continue;
        }
        let path = input
            .path
            .as_deref()
            .expect("write mode rejects standard input");
        if let Err(error) = write_atomically(path, input.formatted.as_bytes()) {
            eprintln!("error: could not write {}: {error}", input.label);
            return EXIT_ERROR;
        }
        println!("formatted: {}", input.label);
    }

    0
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

fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to replace a symbolic link",
        ));
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;

    for attempt in 0..100 {
        let temporary_name = format!(
            ".{}.bbtidy.{}.{}.tmp",
            file_name.to_string_lossy(),
            process::id(),
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
            fs::rename(&temporary_path, path)
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        return result;
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a temporary output file",
    ))
}

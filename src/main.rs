use bbtidy::{Token, format, get_line_col};
use clap::Parser;
use logos::Logos;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Files to process
    #[arg(required = true)]
    files: Vec<PathBuf>,

    /// Format the files
    #[arg(short, long)]
    format: bool,
}

fn main() {
    let cli = Cli::parse();
    let mut had_error = false;

    for file_path in cli.files {
        println!("Processing file: {:?}", file_path);
        match fs::read_to_string(&file_path) {
            Ok(text) => {
                if cli.format {
                    match format(&text) {
                        Ok(formatted) if formatted == text => {
                            println!("Already formatted {:?}", file_path);
                        }
                        Ok(formatted) => {
                            if let Err(error) = write_atomically(&file_path, formatted.as_bytes()) {
                                eprintln!("Error writing file {:?}: {}", file_path, error);
                                had_error = true;
                            } else {
                                println!("Formatted {:?}", file_path);
                            }
                        }
                        Err(error) => {
                            eprintln!("Error formatting {:?}: {}", file_path, error);
                            had_error = true;
                        }
                    }
                } else {
                    let mut lex = Token::lexer(&text);
                    while let Some(tok) = lex.next() {
                        let span = lex.span();
                        let (line, col) = get_line_col(&text, span.start);
                        match tok {
                            Ok(token) => println!(
                                "{:<20} {:?} {}:{} {:?}",
                                format!("{:?}", token),
                                span,
                                line,
                                col,
                                lex.slice()
                            ),
                            Err(_) => println!(
                                "{:<20} {:?} {}:{} {:?}",
                                "Error",
                                span,
                                line,
                                col,
                                lex.slice()
                            ),
                        }
                    }
                }
            }
            Err(error) => {
                eprintln!("Error reading file {:?}: {}", file_path, error);
                had_error = true;
            }
        }
        println!();
    }

    if had_error {
        process::exit(1);
    }
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

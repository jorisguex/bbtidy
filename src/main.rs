use logos::Logos;
use clap::Parser;
use std::fs;
use std::path::PathBuf;
use bbtidy::{Token, get_line_col, format};

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

    for file_path in cli.files {
        println!("Processing file: {:?}", file_path);
        match fs::read_to_string(&file_path) {
            Ok(text) => {
                if cli.format {
                    let formatted = format(&text);
                    if let Err(e) = fs::write(&file_path, formatted) {
                        eprintln!("Error writing file {:?}: {}", file_path, e);
                    } else {
                        println!("Formatted {:?}", file_path);
                    }
                } else {
                    let mut lex = Token::lexer(&text);
                    while let Some(tok) = lex.next() {
                        let span = lex.span();
                        let (line, col) = get_line_col(&text, span.start);
                        match tok {
                            Ok(token) => println!("{:<20} {:?} {}:{} {:?}", format!("{:?}", token), span, line, col, lex.slice()),
                            Err(_) => println!("{:<20} {:?} {}:{} {:?}", "Error", span, line, col, lex.slice()),
                        }
                    }
                }
            }
            Err(e) => eprintln!("Error reading file {:?}: {}", file_path, e),
        }
        println!(); // Separator between files
    }
}

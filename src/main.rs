use logos::Logos;
use clap::Parser;
use std::fs;
use std::path::PathBuf;
use bbtidy::{Token, get_line_col};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Files to process
    #[arg(required = true)]
    files: Vec<PathBuf>,
}

fn main() {
    let cli = Cli::parse();

    for file_path in cli.files {
        println!("Processing file: {:?}", file_path);
        match fs::read_to_string(&file_path) {
            Ok(text) => {
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
            Err(e) => eprintln!("Error reading file {:?}: {}", file_path, e),
        }
        println!(); // Separator between files
    }
}

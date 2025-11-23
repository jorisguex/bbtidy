use logos::Logos;

#[derive(Logos, Debug, PartialEq)]
pub enum Token {
    #[regex(r"[ \n\t\f]+")]
    Whitespace,

    // Comments start with # and go to end of line
    #[regex(r"#.*")]
    Comment,

    // Include or inherit directives
    #[token("include")]
    IncludeKw,
    #[token("require")]
    RequireKw,
    #[token("inherit")]
    InheritKw,

    // Assignment operators: =, :=, ?=, +=, .=
    #[token("=")]
    Assign,
    #[token(":=")]
    WeakAssign,
    #[token("?=")]
    ConditionalAssign,
    #[token("+=")]
    AppendAssign,
    #[token(".=")]
    PrependAssign,

    // Overrides separated by spaces or in variable names with ':'
    // Example: SRC_URI[append] or FILES_${PN} or VAR:append
    #[regex(r"[A-Za-z0-9_]+(:[A-Za-z0-9_]+)?", priority = 2)]
    Ident,

    // Variable references like ${VAR} or ${@python}
    #[regex(r"\$\{[^}]*\}")]
    VarRef,

    // Strings: double-quoted and single-quoted (keep the quotes in slice)
    #[regex(r#""([^"\\]|\\.)*""#)]
    DqString,
    #[regex(r"'([^'\\]|\\.)*'")]
    SqString,
}

use clap::Parser;
use std::fs;
use std::path::PathBuf;

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

fn get_line_col(text: &str, index: usize) -> (usize, usize) {
    let prefix = &text[..index];
    let line = prefix.chars().filter(|&c| c == '\n').count() + 1;
    let last_newline = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = index - last_newline + 1;
    (line, col)
}

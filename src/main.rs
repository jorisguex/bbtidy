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

fn main() {
    let text = r#"
# sample recipe
SUMMARY = "Example recipe"
LICENSE = "MIT"
SRC_URI = "file://hello.tar.gz"
inherit autotools
FILES_${PN} += "/usr/bin/*"
do_configure() {
    ./configure --prefix=/usr
}
"#;

    let mut lex = Token::lexer(text);
    while let Some(tok) = lex.next() {
        let span = lex.span();
        let (line, col) = get_line_col(text, span.start);
        match tok {
            Ok(token) => println!("{:<20} {:?} {}:{} {:?}", format!("{:?}", token), span, line, col, lex.slice()),
            Err(_) => println!("{:<20} {:?} {}:{} {:?}", "Error", span, line, col, lex.slice()),
        }
    }
}

fn get_line_col(text: &str, index: usize) -> (usize, usize) {
    let prefix = &text[..index];
    let line = prefix.chars().filter(|&c| c == '\n').count() + 1;
    let last_newline = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = index - last_newline + 1;
    (line, col)
}

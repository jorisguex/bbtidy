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
    #[token("export")]
    ExportKw,
    #[token("unset")]
    UnsetKw,
    #[token("addtask")]
    AddtaskKw,
    #[token("deltask")]
    DeltaskKw,
    #[token("python")]
    PythonKw,
    #[token("fakeroot")]
    FakerootKw,

    // Assignment operators: =, :=, ?=, ??=, +=, .=
    #[token("=")]
    Assign,
    #[token(":=")]
    WeakAssign,
    #[token("?=")]
    ConditionalAssign,
    #[token("??=")]
    LazyDefaultAssign,
    #[token("+=")]
    AppendAssign,
    #[token(".=")]
    PrependAssign,

    // Punctuation
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("\\")]
    LineContinuation,

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

pub fn get_line_col(text: &str, index: usize) -> (usize, usize) {
    let prefix = &text[..index];
    let line = prefix.chars().filter(|&c| c == '\n').count() + 1;
    let last_newline = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = index - last_newline + 1;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_line_col() {
        let text = "line1\nline2\nline3";
        assert_eq!(get_line_col(text, 0), (1, 1));
        assert_eq!(get_line_col(text, 5), (1, 6)); // newline char
        assert_eq!(get_line_col(text, 6), (2, 1));
        assert_eq!(get_line_col(text, 11), (2, 6)); // newline char
        assert_eq!(get_line_col(text, 12), (3, 1));
    }

    #[test]
    fn test_token_lexer() {
        let text = r#"
            include file
            VAR = "value"
            ${PN}
        "#;
        let mut lex = Token::lexer(text);
        
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::IncludeKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident))); // file
        assert_eq!(lex.slice(), "file");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident))); // VAR
        assert_eq!(lex.slice(), "VAR");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Assign)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DqString)));
        assert_eq!(lex.slice(), "\"value\"");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::VarRef)));
        assert_eq!(lex.slice(), "${PN}");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
    }

    #[test]
    fn test_simple_tokens() {
        let mut lex = Token::lexer("VAR = \"val\"");
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "VAR");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Assign)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DqString)));
        assert_eq!(lex.slice(), "\"val\"");
    }

    #[test]
    fn test_keywords() {
        let mut lex = Token::lexer("include require inherit export unset addtask deltask python fakeroot");
        assert_eq!(lex.next(), Some(Ok(Token::IncludeKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::RequireKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::InheritKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::ExportKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::UnsetKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::AddtaskKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DeltaskKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::PythonKw)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::FakerootKw)));
    }

    #[test]
    fn test_new_syntax() {
        let text = r#"
            VAR ??= "val"
            VAR[flag] = "val"
            do_foo() {
                return 0
            }
        "#;
        let mut lex = Token::lexer(text);

        // VAR ??= "val"
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "VAR");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::LazyDefaultAssign)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DqString)));
        
        // VAR[flag] = "val"
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "VAR");
        assert_eq!(lex.next(), Some(Ok(Token::LBracket)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "flag");
        assert_eq!(lex.next(), Some(Ok(Token::RBracket)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Assign)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::DqString)));

        // do_foo() {
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "do_foo");
        assert_eq!(lex.next(), Some(Ok(Token::LParen)));
        assert_eq!(lex.next(), Some(Ok(Token::RParen)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::LBrace)));
        
        // return 0
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident)));
        assert_eq!(lex.slice(), "return");
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::Ident))); // 0 is parsed as Ident for now
        assert_eq!(lex.slice(), "0");

        // }
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
        assert_eq!(lex.next(), Some(Ok(Token::RBrace)));
        assert_eq!(lex.next(), Some(Ok(Token::Whitespace)));
    }
}

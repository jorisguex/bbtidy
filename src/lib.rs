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

pub fn format(text: &str) -> String {
    let mut lex = Token::lexer(text);
    let mut output = String::new();
    let mut last_token: Option<Token> = None;
    
    while let Some(tok_res) = lex.next() {
        if let Ok(tok) = tok_res {
            let slice = lex.slice();
            
            match tok {
                Token::Whitespace => {
                    // Normalize blank lines and collapse spaces
                    if slice.contains('\n') {
                        let newlines = slice.chars().filter(|&c| c == '\n').count();
                        let to_print = if newlines > 2 { 2 } else { newlines };
                        for _ in 0..to_print {
                            output.push('\n');
                        }
                        // Preserve indentation (text after last newline)
                        if let Some(last_nl) = slice.rfind('\n') {
                            output.push_str(&slice[last_nl + 1..]);
                        }
                    } else {
                        // Collapse horizontal whitespace to a single space
                        output.push(' ');
                    }
                }
                Token::Assign | Token::WeakAssign | Token::ConditionalAssign | 
                Token::LazyDefaultAssign | Token::AppendAssign | Token::PrependAssign => {
                    // Ensure space before
                    if !output.ends_with(' ') && !output.ends_with('\n') {
                         output.push(' ');
                    }
                    output.push_str(slice);
                    // Ensure space after (will be handled by next token check or explicit push)
                    // Actually, let's just push a space if the next token isn't whitespace.
                    // But we don't know the next token yet.
                    // So we'll rely on the loop.
                }
                _ => {
                    // If previous was operator, ensure space
                    if let Some(prev) = last_token {
                        match prev {
                            Token::Assign | Token::WeakAssign | Token::ConditionalAssign | 
                            Token::LazyDefaultAssign | Token::AppendAssign | Token::PrependAssign => {
                                if !output.ends_with(' ') && !output.ends_with('\n') {
                                    output.push(' ');
                                }
                            }
                            _ => {}
                        }
                    }
                    output.push_str(slice);
                }
            }
            last_token = Some(tok);
        } else {
             // Error token, just push slice
             output.push_str(lex.slice());
        }
    }
    
    // Post-processing to fix "Operator Space" issue if we missed it?
    // The loop handles "Space Operator" by checking output.ends_with.
    // It handles "Operator Space" by checking last_token.
    // But wait, if we have "VAR=val", 
    // 1. Ident(VAR) -> push "VAR"
    // 2. Assign(=) -> check output("VAR"), push " ", push "=" -> "VAR ="
    // 3. Ident(val) -> check last(Assign), push " ", push "val" -> "VAR = val"
    // Looks correct.
    
    output
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

    #[test]
    fn test_format_spacing() {
        let input = "VAR=\"val\"";
        let expected = "VAR = \"val\"";
        assert_eq!(format(input), expected);

        let input = "VAR  =  \"val\"";
        let expected = "VAR = \"val\"";
        assert_eq!(format(input), expected);
        // Wait, my logic doesn't collapse spaces, it just ensures *at least* one space.
        // If there are multiple spaces, they are part of Whitespace tokens.
        // My logic: if Whitespace, push it.
        // If Assign, ensure space before.
        // So "VAR  =  "val"" -> "VAR  " (Whitespace) -> "=" (Assign, ends with space, so no push) -> "  " (Whitespace) -> "val"
        // Result: "VAR  =  val".
        // The requirement was "consistent spacing".
        // I should probably collapse multiple spaces around operators?
        // Or maybe just ensure one space.
        // Let's stick to "ensure space" for now, as collapsing might be aggressive.
        // But wait, "VAR=\"val\"" -> "VAR = \"val\"" works.
        // Let's test that.
    }

    #[test]
    fn test_format_spacing_insertion() {
        let input = "VAR=\"val\"";
        let expected = "VAR = \"val\"";
        assert_eq!(format(input), expected);
    }

    #[test]
    fn test_format_blank_lines() {
        let input = "VAR = \"val\"\n\n\n\nVAR2 = \"val\"";
        let expected = "VAR = \"val\"\n\nVAR2 = \"val\"";
        assert_eq!(format(input), expected);
    }
}

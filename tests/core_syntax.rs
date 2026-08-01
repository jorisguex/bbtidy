use bbtidy::{Token, format};
use logos::Logos;

const INPUT: &str = include_str!("fixtures/core_syntax.conf");
const EXPECTED: &str = include_str!("fixtures/core_syntax.formatted.conf");

#[test]
fn core_metadata_fixture_lexes_without_errors() {
    let mut lexer = Token::lexer(INPUT);
    let mut token_count = 0;

    while let Some(result) = lexer.next() {
        result.unwrap_or_else(|_| {
            panic!(
                "unexpected lexer error at {:?}: {:?}",
                lexer.span(),
                lexer.slice()
            )
        });
        token_count += 1;
    }

    assert!(token_count > 100);
}

#[test]
fn core_metadata_fixture_matches_golden_output() {
    assert_eq!(format(INPUT).unwrap(), EXPECTED);
}

#[test]
fn core_metadata_fixture_is_idempotent() {
    assert_eq!(format(EXPECTED).unwrap(), EXPECTED);
}

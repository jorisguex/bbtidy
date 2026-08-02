use bbtidy::{LintSeverity, lint, lint_rules};
use std::fs;
use std::path::{Path, PathBuf};

const CORPUS_FILES: [&str; 6] = [
    "conf/layer.conf",
    "classes/example.bbclass",
    "recipes-example/example/example.inc",
    "recipes-example/example/example_1.0.bb",
    "recipes-example/example/example_%.bbappend",
    "recipes-example/example/compatibility.bb",
];

#[test]
fn public_lint_api_exposes_rule_metadata_and_diagnostics() {
    let rules = lint_rules();
    assert_eq!(rules.len(), 5);
    assert_eq!(rules[0].id(), "BBT001");
    assert_eq!(rules[0].name(), "trailing-whitespace");
    assert_eq!(rules[0].severity(), LintSeverity::Warning);
    assert!(!rules[0].description().is_empty());

    let diagnostics = lint("SRCREV = \"${AUTOREV}\"\n").unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].rule_id(), "BBT004");
    assert_eq!(diagnostics[0].severity(), LintSeverity::Warning);
    assert_eq!(diagnostics[0].line(), 1);
    assert_eq!(diagnostics[0].column(), 11);
}

#[test]
fn formatted_fixture_layer_is_clean_under_default_rules() {
    for relative_path in CORPUS_FILES {
        let path = corpus_root().join(relative_path);
        let text = fs::read_to_string(&path).unwrap();
        let diagnostics = lint(&text).unwrap();
        assert!(
            diagnostics.is_empty(),
            "{} produced unexpected diagnostics: {diagnostics:?}",
            path.display()
        );
    }
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus/expected")
}

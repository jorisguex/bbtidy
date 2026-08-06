use bbtidy::{SyntaxKind, format, lint, parse};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn focused_syntax_corpus_matches_golden_output_and_invariants() {
    let mut inputs = fs::read_dir(input_root())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    inputs.sort();
    assert_eq!(inputs.len(), 5);
    let expected_diagnostics: Value = serde_json::from_str(
        &fs::read_to_string(expected_root().join("diagnostics.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(expected_diagnostics["schema"], 1);

    for input_path in inputs {
        let relative = input_path.file_name().unwrap();
        let file_name = relative.to_str().unwrap();
        let source = fs::read_to_string(&input_path).unwrap();
        let expected = fs::read_to_string(expected_root().join(relative)).unwrap();
        let tree = parse(&source)
            .unwrap_or_else(|error| panic!("{} did not parse: {error}", input_path.display()));

        let rebuilt = tree
            .nodes()
            .iter()
            .map(|node| node.text())
            .collect::<String>();
        assert_eq!(rebuilt, source, "{} was not lossless", input_path.display());
        if relative != "unsupported-top-level.conf" {
            assert!(
                tree.nodes()
                    .iter()
                    .any(|node| matches!(node.kind(), SyntaxKind::Assignment(_))),
                "{} did not exercise assignment syntax",
                input_path.display()
            );
        }

        let formatted = format(&source)
            .unwrap_or_else(|error| panic!("{} did not format: {error}", input_path.display()));
        assert_eq!(
            formatted,
            expected,
            "{} golden mismatch",
            input_path.display()
        );
        assert_eq!(format(&formatted).unwrap(), formatted);
        parse(&formatted).unwrap_or_else(|error| {
            panic!("formatted {} did not parse: {error}", input_path.display())
        });
        let diagnostics = lint(&source).unwrap();
        let expected_diagnostic_list = expected_diagnostics["diagnostics"][file_name]
            .as_array()
            .unwrap();
        assert_eq!(
            diagnostics.len(),
            expected_diagnostic_list.len(),
            "{} diagnostics changed",
            input_path.display()
        );
    }
}

fn input_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/syntax-boundaries/input")
}

fn expected_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/syntax-boundaries/expected")
}

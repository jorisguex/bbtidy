use bbtidy::{
    BodyDiagnostic, analyze_python_body, analyze_shell_body, format_syntax, lint_syntax, parse,
};
use proptest::prelude::*;

fn assert_body_diagnostics_are_safe(source: &str, diagnostics: &[BodyDiagnostic]) {
    let mut previous = (0, 0);
    for diagnostic in diagnostics {
        let range = diagnostic.range();
        assert!(range.start() <= range.end());
        assert!(range.end() <= source.len());
        assert!(source.is_char_boundary(range.start()));
        assert!(source.is_char_boundary(range.end()));
        assert!((range.start(), range.end()) >= previous);
        previous = (range.start(), range.end());
    }
}

proptest! {
    #[test]
    fn parser_is_lossless_and_formatting_is_idempotent(
        bytes in prop::collection::vec(any::<u8>(), 0..1024),
    ) {
        let source = String::from_utf8_lossy(&bytes).into_owned();
        let Ok(tree) = parse(&source) else {
            return Ok(());
        };

        let rebuilt: String = tree.nodes().iter().map(|node| node.text()).collect();
        prop_assert_eq!(&rebuilt, &source);

        let formatted = format_syntax(&tree);
        let formatted_tree = parse(&formatted)
            .expect("formatting a structurally complete tree must remain parseable");
        prop_assert_eq!(format_syntax(&formatted_tree), formatted);

        let _ = lint_syntax(&tree);
    }

    #[test]
    fn embedded_analyzers_are_deterministic_and_range_safe(
        bytes in prop::collection::vec(any::<u8>(), 0..2048),
    ) {
        let source = String::from_utf8_lossy(&bytes).into_owned();
        let shell = analyze_shell_body(&source);
        let python = analyze_python_body(&source);
        assert_body_diagnostics_are_safe(&source, &shell);
        assert_body_diagnostics_are_safe(&source, &python);
        prop_assert_eq!(&shell, &analyze_shell_body(&source));
        prop_assert_eq!(&python, &analyze_python_body(&source));
    }

    #[test]
    fn structured_embedded_examples_remain_lossless_and_idempotent(
        lines in prop::collection::vec(prop::sample::select(vec![
            "if value:",
            "    return {\"value\": value}",
            "for item in values:",
            "    echo \"$(printf '%s' \"$item\")\"",
            "case \"$value\" in",
            "    one) echo one ;;",
            "    *) echo other ;;",
            "esac",
            "    \"\"\"triple string\"\"\"",
            "def helper(value):",
            "    return value",
        ]), 1..32),
    ) {
        let source = format!("{}\n", lines.join("\n"));
        let tree = parse(&source).expect("generated structured example parses");
        let rebuilt: String = tree.nodes().iter().map(|node| node.text()).collect();
        prop_assert_eq!(&rebuilt, &source);
        let formatted = format_syntax(&tree);
        let reparsed = parse(&formatted).expect("formatted generated example parses");
        prop_assert_eq!(format_syntax(&reparsed), formatted);
        assert_body_diagnostics_are_safe(&source, &analyze_shell_body(&source));
        assert_body_diagnostics_are_safe(&source, &analyze_python_body(&source));
    }
}

#[test]
fn adversarial_embedded_inputs_scale_without_pathological_work() {
    for repetitions in [32, 64, 128, 256, 512] {
        let shell = format!(
            "do_install() {{\n{}\n}}\n",
            "    echo \"$(printf '%s' \"${D}/usr/bin\")\"".repeat(repetitions)
        );
        let python = format!(
            "def helper(\n    value: str = f\"{{value}}\",\n):\n    \"\"\"opaque {{value}}\n{}\n\"\"\"\n    return value\n",
            "    nested = {\"value\": value}".repeat(repetitions)
        );

        for source in [shell, python] {
            let tree = parse(&source).expect("adversarial structured input parses");
            let rebuilt: String = tree.nodes().iter().map(|node| node.text()).collect();
            assert_eq!(rebuilt, source);
            assert_body_diagnostics_are_safe(&source, &analyze_shell_body(&source));
            assert_body_diagnostics_are_safe(&source, &analyze_python_body(&source));
        }
    }
}

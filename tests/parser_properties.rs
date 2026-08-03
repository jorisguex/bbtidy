use bbtidy::{format_syntax, lint_syntax, parse};
use proptest::prelude::*;

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
}

#![no_main]

use bbtidy::{format_syntax, lint_syntax, parse};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let source = String::from_utf8_lossy(data);
    let Ok(tree) = parse(&source) else {
        return;
    };

    let rebuilt: String = tree.nodes().iter().map(|node| node.text()).collect();
    assert_eq!(rebuilt, source);

    let formatted = format_syntax(&tree);
    let formatted_tree = parse(&formatted)
        .expect("formatting a structurally complete fuzz input must remain parseable");
    assert_eq!(format_syntax(&formatted_tree), formatted);
    let _ = lint_syntax(&tree);
});

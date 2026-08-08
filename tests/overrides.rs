use bbtidy::{
    OverrideOperation, parse, parse_override_key, parse_override_key_with_overrides,
    resolve_overrides, resolve_overrides_with_active,
};

#[test]
fn public_override_parser_normalizes_modern_and_legacy_keys() {
    let modern = parse_override_key("FILES:${PN}:class-native:append").unwrap();
    assert_eq!(modern.base(), "FILES");
    assert_eq!(
        modern.overrides(),
        &["${PN}".to_owned(), "class-native".to_owned()]
    );
    assert_eq!(modern.operation(), OverrideOperation::Append);
    assert!(modern.is_dynamic());
    assert!(modern.operation_on_selected_value());

    let deferred = parse_override_key("FILES:append:class-native").unwrap();
    assert!(!deferred.operation_on_selected_value());

    let legacy =
        parse_override_key_with_overrides("RDEPENDS_${PN}_class-native_append", &["class-native"])
            .unwrap();
    assert_eq!(legacy.base(), "RDEPENDS_${PN}");
    assert_eq!(legacy.overrides(), &["class-native".to_owned()]);
    assert_eq!(legacy.operation(), OverrideOperation::Append);
    assert!(legacy.is_legacy());
}

#[test]
fn public_override_resolver_applies_precedence_and_operations() {
    let tree = parse(concat!(
        "OVERRIDES = \"machine:class-native\"\n",
        "PN = \"demo\"\n",
        "VALUE = \"base\"\n",
        "VALUE:machine = \"machine\"\n",
        "VALUE:class-native = \"native\"\n",
        "VALUE:prepend:class-native = \"prefix \"\n",
        "VALUE:append = \" suffix\"\n",
        "VALUE:remove = \"base\"\n",
        "RDEPENDS_${PN}_class-native = \"native-dependency\"\n",
    ))
    .unwrap();

    let resolved = resolve_overrides(&tree);
    assert_eq!(resolved.overrides(), &["machine", "class-native"]);
    assert_eq!(resolved.get("VALUE"), Some("prefix native suffix"));
    assert_eq!(resolved.get("RDEPENDS_demo"), Some("native-dependency"));
}

#[test]
fn public_override_resolver_accepts_external_context() {
    let tree = parse("VALUE = \"base\"\nVALUE:machine = \"machine\"\n").unwrap();
    let resolved = resolve_overrides_with_active(&tree, &["machine"]);
    assert_eq!(resolved.get("VALUE"), Some("machine"));
}

#[test]
fn public_override_resolver_expands_static_override_context() {
    let tree = parse(
        "MACHINE = \"machine\"\nOVERRIDES = \"${MACHINE}:class-native\"\nVALUE:machine = \"native\"\n",
    )
    .unwrap();
    let resolved = resolve_overrides(&tree);

    assert_eq!(resolved.overrides(), &["machine", "class-native"]);
    assert_eq!(resolved.get("VALUE"), Some("native"));
}

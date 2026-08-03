use bbtidy::{SyntaxKind, format_syntax, lint_syntax, parse};

#[test]
fn representative_bitbake_constructs_are_lossless_and_idempotent() {
    let cases = [
        (
            "modern metadata",
            concat!(
                "export RDEPENDS:${PN}:class-native[doc] = \" \\\n",
                "\tpython3 \\\n",
                "\t\"\n",
                "include_all conf/distro/include/maintainers.inc\n",
                "addfragments conf/fragments OE_FRAGMENTS OE_METADATA OE_BUILTIN\n",
                "inherit_defer ${@bb.utils.contains('DISTRO_FEATURES', 'x', 'class-x', '', d)}\n",
            ),
        ),
        (
            "opaque shell and python",
            concat!(
                "fakeroot do_install() {\n",
                "\tcat <<-EOF > ${D}/value\n",
                "\t}\n",
                "\tEOF\n",
                "}\n",
                "python () {\n",
                "    value = '}'; # body remains opaque\n",
                "}\n",
                "def helper(d):\n",
                "    return d.getVar('FOO')\n",
            ),
        ),
        (
            "legacy and modern operators",
            concat!(
                "A=\"a\"\n",
                "B  :=\"b\"\n",
                "C?=\"c\"\n",
                "D??=\"d\"\n",
                "E+=\"e\"\n",
                "F=+\"f\"\n",
                "G.=\"g\"\n",
                "H=.\"h\"\n",
            ),
        ),
    ];

    for (name, source) in cases {
        let tree = parse(source).unwrap_or_else(|error| panic!("{name} did not parse: {error}"));
        let rebuilt: String = tree.nodes().iter().map(|node| node.text()).collect();
        assert_eq!(rebuilt, source, "{name} was not lossless");

        let formatted = format_syntax(&tree);
        let reparsed = parse(&formatted)
            .unwrap_or_else(|error| panic!("formatted {name} did not parse: {error}"));
        assert_eq!(
            format_syntax(&reparsed),
            formatted,
            "{name} was not idempotent"
        );
        let _ = lint_syntax(&tree);
    }
}

#[test]
fn conformance_case_exposes_structured_nodes() {
    let tree = parse(
        "SUMMARY=\"demo\"\ninclude demo.inc\npython do_build() {\n}\ndef helper(d):\n    pass\n",
    )
    .unwrap();

    assert!(
        tree.nodes()
            .iter()
            .any(|node| matches!(node.kind(), SyntaxKind::Assignment(_)))
    );
    assert!(
        tree.nodes()
            .iter()
            .any(|node| matches!(node.kind(), SyntaxKind::Directive(_)))
    );
    assert!(
        tree.nodes()
            .iter()
            .any(|node| matches!(node.kind(), SyntaxKind::Function(_)))
    );
    assert!(
        tree.nodes()
            .iter()
            .any(|node| matches!(node.kind(), SyntaxKind::PythonDefinition(_)))
    );
}

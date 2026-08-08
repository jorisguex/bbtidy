use bbtidy::{
    AssignmentOperator, DirectiveKeyword, FunctionKind, SyntaxKind, format_syntax, lint_syntax,
    parse,
};

#[test]
fn public_tree_is_lossless_and_exposes_source_ranges() {
    let source = concat!(
        "# recipe metadata\n",
        "export SUMMARY = \"Example\"\n",
        "inherit base \\\n",
        "  features\n",
        "fakeroot python do_install() {\n",
        "    value = \"}\" # body stays opaque\n",
        "}\n",
    );

    let tree = parse(source).unwrap();
    let rebuilt: String = tree.nodes().iter().map(|node| node.text()).collect();
    assert_eq!(rebuilt, source);
    assert_eq!(format_syntax(&tree), bbtidy::format(source).unwrap());
    assert_eq!(lint_syntax(&tree), bbtidy::lint(source).unwrap());

    let SyntaxKind::Assignment(assignment) = tree.nodes()[1].kind() else {
        panic!("expected assignment");
    };
    assert_eq!(assignment.name(), "SUMMARY");
    assert_eq!(assignment.operator(), AssignmentOperator::Assign);
    assert!(assignment.is_exported());
    assert_eq!(
        &source[assignment.name_range().start()..assignment.name_range().end()],
        "SUMMARY"
    );

    let SyntaxKind::Directive(directive) = tree.nodes()[2].kind() else {
        panic!("expected directive");
    };
    assert_eq!(directive.keyword(), DirectiveKeyword::Inherit);
    assert!(directive.is_continued());

    let SyntaxKind::Function(function) = tree.nodes()[3].kind() else {
        panic!("expected function");
    };
    assert_eq!(function.function_kind(), FunctionKind::Python);
    assert_eq!(function.name(), Some("do_install"));
    assert!(function.is_fakeroot());
    assert_eq!(
        &source[function.body_range().start()..function.body_range().end()],
        "\n    value = \"}\" # body stays opaque\n"
    );

    let python_source = "def helper(d):\n    return d.getVar(\"FOO\")\nNEXT = \"value\"\n";
    let python_tree = parse(python_source).unwrap();
    let SyntaxKind::PythonDefinition(definition) = python_tree.nodes()[0].kind() else {
        panic!("expected Python definition");
    };
    assert_eq!(
        &python_source[definition.body_range().start()..definition.body_range().end()],
        "    return d.getVar(\"FOO\")\n"
    );
}

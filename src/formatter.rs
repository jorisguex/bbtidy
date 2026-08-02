use crate::{
    AssignmentSyntax, DirectiveSyntax, SyntaxKind, SyntaxNode, SyntaxTree, split_line_ending,
};

/// Formats a previously parsed syntax tree without reparsing its source.
///
/// Assignment operators and directive keywords are normalized at the
/// top-level boundary. Continuation tails, comments, unsupported syntax, and
/// embedded shell or Python bodies remain byte-for-byte unchanged.
pub fn format_syntax(tree: &SyntaxTree<'_>) -> String {
    let mut output = String::new();

    for node in tree.nodes() {
        match node.kind() {
            SyntaxKind::Blank => append_normalized_blank_line(&mut output, node.text()),
            SyntaxKind::Assignment(assignment) => format_assignment(&mut output, node, assignment),
            SyntaxKind::Directive(directive) => format_directive(&mut output, node, directive),
            _ => output.push_str(node.text()),
        }
    }

    output
}

fn format_assignment(
    output: &mut String,
    node: &SyntaxNode<'_>,
    assignment: &AssignmentSyntax<'_>,
) {
    let relative_operator = assignment.operator_range().start() - node.range().start();
    let left = node.text()[..relative_operator].trim_end();
    let value = assignment.value().trim_start_matches([' ', '\t']);
    let (_, line_ending) = split_line_ending(node.text());

    output.push_str(left);
    output.push(' ');
    output.push_str(assignment.operator().lexeme());
    if !value.is_empty() {
        output.push(' ');
        output.push_str(value);
    }
    output.push_str(line_ending);
}

fn format_directive(output: &mut String, node: &SyntaxNode<'_>, directive: &DirectiveSyntax<'_>) {
    let relative_keyword = directive.keyword_range().start() - node.range().start();
    let prefix = &node.text()[..relative_keyword];
    let arguments = directive.arguments();
    let (_, line_ending) = split_line_ending(node.text());

    output.push_str(prefix);
    output.push_str(directive.keyword().lexeme());
    if !arguments.is_empty() {
        output.push(' ');
        output.push_str(arguments);
    }
    output.push_str(line_ending);
}

fn append_normalized_blank_line(output: &mut String, line: &str) {
    let (_, line_ending) = split_line_ending(line);
    if line_ending.is_empty() {
        return;
    }

    if !output.ends_with("\n\n") && !output.ends_with("\r\n\r\n") {
        output.push_str(line_ending);
    }
}

#[cfg(test)]
mod tests {
    use crate::format;

    #[test]
    fn formats_continued_assignment_header_without_changing_its_tail() {
        let input = concat!(
            "SRC_URI  =   \" \\\n",
            "\tfile://one.patch \\\n",
            "      file://two.patch \\\n",
            "\" # keep this comment\n",
        );
        let expected = concat!(
            "SRC_URI = \" \\\n",
            "\tfile://one.patch \\\n",
            "      file://two.patch \\\n",
            "\" # keep this comment\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn formats_directive_separator_without_changing_arguments() {
        let input = concat!(
            "inherit\t  autotools  pkgconfig # keep  comment spacing\n",
            "addtask    package after  do_package before do_build\n",
            "inherit_defer   ${VARNAME} \\\n",
            "\tsecond\n",
            "export   CC\n",
        );
        let expected = concat!(
            "inherit autotools  pkgconfig # keep  comment spacing\n",
            "addtask package after  do_package before do_build\n",
            "inherit_defer ${VARNAME} \\\n",
            "\tsecond\n",
            "export CC\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn preserves_crlf_continuations_and_directive_comments() {
        let input =
            "SRC_URI=  \" \\\r\n\tfile://one.patch \\\r\n\"\r\nrequire   example.inc # note\r\n";
        let expected =
            "SRC_URI = \" \\\r\n\tfile://one.patch \\\r\n\"\r\nrequire example.inc # note\r\n";

        assert_eq!(format(input).unwrap(), expected);
    }

    #[test]
    fn top_level_blank_normalization_does_not_reach_function_bodies() {
        let input = concat!(
            "inherit   base\n",
            "\n",
            "\n",
            "do_example() {\n",
            "\n",
            "\n",
            "    echo unchanged\n",
            "}\n",
            "\n",
            "\n",
            "require   example.inc\n",
        );
        let expected = concat!(
            "inherit base\n",
            "\n",
            "do_example() {\n",
            "\n",
            "\n",
            "    echo unchanged\n",
            "}\n",
            "\n",
            "require example.inc\n",
        );

        assert_eq!(format(input).unwrap(), expected);
    }
}

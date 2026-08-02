use crate::{
    AssignmentSyntax, DirectiveSyntax, SyntaxKind, SyntaxNode, SyntaxTree, split_line_ending,
};

/// Controls formatting behavior that is safe to apply at the top-level
/// metadata boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FormatOptions {
    /// Maximum number of consecutive blank lines between top-level nodes.
    pub max_top_level_blank_lines: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            max_top_level_blank_lines: 1,
        }
    }
}

/// Formats a previously parsed syntax tree without reparsing its source.
///
/// Assignment operators and directive keywords are normalized at the
/// top-level boundary. Continuation tails, comments, unsupported syntax, and
/// embedded shell or Python bodies remain byte-for-byte unchanged.
pub fn format_syntax(tree: &SyntaxTree<'_>) -> String {
    format_syntax_with_options(tree, &FormatOptions::default())
}

/// Formats a previously parsed syntax tree with caller-provided options.
pub fn format_syntax_with_options(tree: &SyntaxTree<'_>, options: &FormatOptions) -> String {
    let mut output = String::new();

    for node in tree.nodes() {
        match node.kind() {
            SyntaxKind::Blank => append_normalized_blank_line(&mut output, node.text(), options),
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

fn append_normalized_blank_line(output: &mut String, line: &str, options: &FormatOptions) {
    let (_, line_ending) = split_line_ending(line);
    if line_ending.is_empty() || trailing_blank_lines(output) >= options.max_top_level_blank_lines {
        return;
    }

    output.push_str(line_ending);
}

fn trailing_blank_lines(output: &str) -> usize {
    output
        .split_inclusive('\n')
        .rev()
        .take_while(|line| {
            let (content, _) = split_line_ending(line);
            content.trim_matches([' ', '\t', '\r']).is_empty()
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::{FormatOptions, format_syntax_with_options};
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

    #[test]
    fn configurable_top_level_blank_line_limit_is_idempotent() {
        let source = "A=\"a\"\n\n\nB=\"b\"\n";
        let tree = crate::parse(source).unwrap();
        let options = FormatOptions {
            max_top_level_blank_lines: 2,
        };
        let formatted = format_syntax_with_options(&tree, &options);

        assert_eq!(formatted, "A = \"a\"\n\n\nB = \"b\"\n");
        assert_eq!(
            format_syntax_with_options(&crate::parse(&formatted).unwrap(), &options),
            formatted
        );
    }
}

use crate::{
    AssignmentSyntax, DirectiveSyntax, SyntaxKind, SyntaxNode, SyntaxTree, split_line_ending,
};
use serde::Deserialize;

/// Controls whether selected static metadata lists receive a structural layout.
///
/// The default preserves every continuation tail. `OnePerLine` is intentionally
/// limited to static, continued quoted values for known whitespace-separated
/// metadata variables.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataListLayout {
    /// Preserve continuation tails byte-for-byte.
    #[default]
    Preserve,
    /// Put each safely recognized metadata-list item on its own line.
    OnePerLine,
}

/// Controls formatting behavior that is safe to apply at the top-level
/// metadata boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FormatOptions {
    /// Maximum number of consecutive blank lines between top-level nodes.
    pub max_top_level_blank_lines: usize,
    /// Layout for safely recognized static metadata lists.
    pub metadata_list_layout: MetadataListLayout,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            max_top_level_blank_lines: 1,
            metadata_list_layout: MetadataListLayout::Preserve,
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
            SyntaxKind::Assignment(assignment) => {
                format_assignment(&mut output, node, assignment, options)
            }
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
    options: &FormatOptions,
) {
    if options.metadata_list_layout == MetadataListLayout::OnePerLine
        && let Some(list) = parse_static_metadata_list(node, assignment)
    {
        format_metadata_list(output, node, assignment, list);
        return;
    }

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

struct StaticMetadataList<'a> {
    quote: char,
    items: Vec<&'a str>,
    closing_tail: &'a str,
    line_ending: &'a str,
}

fn parse_static_metadata_list<'a>(
    node: &SyntaxNode<'a>,
    assignment: &AssignmentSyntax<'a>,
) -> Option<StaticMetadataList<'a>> {
    if !assignment.is_continued() || !is_known_metadata_list(assignment.name()) {
        return None;
    }

    let line_ending = consistent_line_ending(node.text())?;
    let value = assignment.value().trim_start_matches([' ', '\t']);
    let quote = value.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }

    let quoted_value = &value[quote.len_utf8()..];
    let (contents, closing_tail) = split_closing_quote(quoted_value, quote)?;
    if !is_comment_or_whitespace(closing_tail) {
        return None;
    }

    let mut items = Vec::new();
    let mut has_continuation = false;
    for (line_number, line) in contents.split_inclusive('\n').enumerate() {
        let (content, current_line_ending) = split_line_ending(line);
        if !current_line_ending.is_empty() && current_line_ending != line_ending {
            return None;
        }

        let content = content.trim_start_matches([' ', '\t']);
        if current_line_ending.is_empty() {
            if content.is_empty() {
                continue;
            }
            if content.trim_end_matches([' ', '\t']) != content {
                return None;
            }
            if !is_static_metadata_list_item(content, quote) {
                return None;
            }
            items.push(content);
            continue;
        }

        let before_continuation = content.strip_suffix('\\')?;
        has_continuation = true;
        if before_continuation.is_empty() {
            if line_number != 0 {
                return None;
            }
            continue;
        }
        if !before_continuation.ends_with([' ', '\t']) {
            return None;
        }
        let item = before_continuation.trim_end_matches([' ', '\t']);
        if !is_static_metadata_list_item(item, quote) {
            return None;
        }
        items.push(item);
    }

    if !has_continuation || items.len() < 2 {
        return None;
    }

    Some(StaticMetadataList {
        quote,
        items,
        closing_tail,
        line_ending,
    })
}

fn is_known_metadata_list(name: &str) -> bool {
    matches!(
        name.split(':').next(),
        Some("SRC_URI" | "DEPENDS" | "RDEPENDS")
    )
}

fn consistent_line_ending(text: &str) -> Option<&str> {
    let mut line_ending = None;
    for line in text.split_inclusive('\n') {
        let (_, current) = split_line_ending(line);
        if current.is_empty() {
            continue;
        }
        match line_ending {
            Some(existing) if existing != current => return None,
            Some(_) => {}
            None => line_ending = Some(current),
        }
    }
    line_ending
}

fn split_closing_quote(value: &str, quote: char) -> Option<(&str, &str)> {
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if character == '\\' {
            escaped = !escaped;
            continue;
        }
        if character == quote && !escaped {
            let closing_tail = &value[index + quote.len_utf8()..];
            if is_comment_or_whitespace(closing_tail) {
                return Some((&value[..index], closing_tail));
            }
        }
        escaped = false;
    }
    None
}

fn is_comment_or_whitespace(text: &str) -> bool {
    let trimmed = text.trim_start_matches([' ', '\t']);
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn is_static_metadata_list_item(item: &str, quote: char) -> bool {
    !item.is_empty()
        && !item.chars().any(|character| {
            character.is_whitespace()
                || matches!(character, '$' | '{' | '}' | '\\')
                || character == quote
        })
}

fn format_metadata_list(
    output: &mut String,
    node: &SyntaxNode<'_>,
    assignment: &AssignmentSyntax<'_>,
    list: StaticMetadataList<'_>,
) {
    let relative_operator = assignment.operator_range().start() - node.range().start();
    let left = node.text()[..relative_operator].trim_end();
    let (_, final_line_ending) = split_line_ending(node.text());

    output.push_str(left);
    output.push(' ');
    output.push_str(assignment.operator().lexeme());
    output.push(' ');
    output.push(list.quote);
    output.push(' ');
    output.push('\\');
    output.push_str(list.line_ending);
    for item in list.items {
        output.push_str("    ");
        output.push_str(item);
        output.push(' ');
        output.push('\\');
        output.push_str(list.line_ending);
    }
    output.push_str("    ");
    output.push(list.quote);
    output.push_str(list.closing_tail);
    output.push_str(final_line_ending);
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
    use super::{FormatOptions, MetadataListLayout, format_syntax_with_options};
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
    fn lays_out_static_known_metadata_lists_when_enabled() {
        let input = concat!(
            "SRC_URI  =  \" \\\n",
            "\tfile://one.patch \\\n",
            "      file://two.patch \\\n",
            "      \" # keep this comment\n",
            "DEPENDS= \"build-a \\\n",
            "  build-b \\\n",
            " build-c\"\n",
            "RDEPENDS:${PN} = 'runtime-a \\\n",
            "      runtime-b \\\n",
            "    '\n",
            "OTHER = \"unformatted \\\n",
            "        generic \\\n",
            "\"\n",
        );
        let expected = concat!(
            "SRC_URI = \" \\\n",
            "    file://one.patch \\\n",
            "    file://two.patch \\\n",
            "    \" # keep this comment\n",
            "DEPENDS = \" \\\n",
            "    build-a \\\n",
            "    build-b \\\n",
            "    build-c \\\n",
            "    \"\n",
            "RDEPENDS:${PN} = ' \\\n",
            "    runtime-a \\\n",
            "    runtime-b \\\n",
            "    '\n",
            "OTHER = \"unformatted \\\n",
            "        generic \\\n",
            "\"\n",
        );
        let options = FormatOptions {
            metadata_list_layout: MetadataListLayout::OnePerLine,
            ..FormatOptions::default()
        };
        let formatted = format_syntax_with_options(&crate::parse(input).unwrap(), &options);

        assert_eq!(formatted, expected);
        assert_eq!(
            format_syntax_with_options(&crate::parse(&formatted).unwrap(), &options),
            formatted
        );
    }

    #[test]
    fn metadata_list_layout_skips_dynamic_and_mixed_line_ending_values() {
        let input = concat!(
            "SRC_URI= \" \\\n",
            "    file://${PN}.patch \\\n",
            "    file://safe.patch \\\n",
            "\"\n",
            "DEPENDS= \" \\\r\n",
            "    build-a \\\n",
            "    build-b \\\n",
            "\"\n",
        );
        let expected =
            input
                .replacen("SRC_URI=", "SRC_URI =", 1)
                .replacen("DEPENDS=", "DEPENDS =", 1);
        let options = FormatOptions {
            metadata_list_layout: MetadataListLayout::OnePerLine,
            ..FormatOptions::default()
        };

        assert_eq!(
            format_syntax_with_options(&crate::parse(input).unwrap(), &options),
            expected
        );
    }

    #[test]
    fn metadata_list_layout_preserves_crlf() {
        let input = concat!(
            "SRC_URI= \" \\\r\n",
            "\tfile://one.patch \\\r\n",
            " file://two.patch \\\r\n",
            "\"\r\n",
        );
        let expected = concat!(
            "SRC_URI = \" \\\r\n",
            "    file://one.patch \\\r\n",
            "    file://two.patch \\\r\n",
            "    \"\r\n",
        );
        let options = FormatOptions {
            metadata_list_layout: MetadataListLayout::OnePerLine,
            ..FormatOptions::default()
        };

        assert_eq!(
            format_syntax_with_options(&crate::parse(input).unwrap(), &options),
            expected
        );
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
            ..FormatOptions::default()
        };
        let formatted = format_syntax_with_options(&tree, &options);

        assert_eq!(formatted, "A = \"a\"\n\n\nB = \"b\"\n");
        assert_eq!(
            format_syntax_with_options(&crate::parse(&formatted).unwrap(), &options),
            formatted
        );
    }
}

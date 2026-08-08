use crate::{AssignmentOperator, SyntaxKind, SyntaxTree};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// An operation encoded in a BitBake override key.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum OverrideOperation {
    /// A normal assignment key without an override operation.
    None,
    /// Append text after the selected value.
    Append,
    /// Prepend text before the selected value.
    Prepend,
    /// Remove whitespace-delimited tokens from the selected value.
    Remove,
}

impl OverrideOperation {
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Append => Some("append"),
            Self::Prepend => Some("prepend"),
            Self::Remove => Some("remove"),
        }
    }
}

/// An error returned when a variable key cannot be interpreted as a BitBake
/// override key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OverrideKeyError {
    message: String,
}

impl OverrideKeyError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the explanation of the invalid key.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for OverrideKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OverrideKeyError {}

/// A parsed BitBake variable key with modern or legacy override notation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OverrideKey {
    raw: String,
    base: String,
    overrides: Vec<String>,
    operation: OverrideOperation,
    flag: Option<String>,
    legacy: bool,
    dynamic: bool,
    operation_on_selected_value: bool,
    key_expanded: bool,
}

impl OverrideKey {
    /// Returns the original variable key.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the normalized base variable name.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Returns normalized literal override components in key order.
    pub fn overrides(&self) -> &[String] {
        &self.overrides
    }

    /// Returns the operation encoded in the key.
    pub const fn operation(&self) -> OverrideOperation {
        self.operation
    }

    /// Returns whether the operation modifies the override-specific variable
    /// before that variable is selected. For example, `A:machine:append`
    /// targets the `A:machine` value, while `A:append:machine` appends to the
    /// selected `A` value.
    pub const fn operation_on_selected_value(&self) -> bool {
        self.operation_on_selected_value
    }

    /// Returns the variable flag, when the key contains one.
    pub fn flag(&self) -> Option<&str> {
        self.flag.as_deref()
    }

    /// Returns whether the key used legacy underscore notation.
    pub const fn is_legacy(&self) -> bool {
        self.legacy
    }

    /// Returns whether a variable or override component could not be expanded
    /// statically.
    pub const fn is_dynamic(&self) -> bool {
        self.dynamic
    }

    /// Returns whether the key contained a variable reference that was
    /// expanded during normalization.
    pub const fn key_expanded(&self) -> bool {
        self.key_expanded
    }
}

/// Parses modern override notation and any unambiguous legacy operation
/// suffixes without an active override list.
pub fn parse_override_key(name: &str) -> Result<OverrideKey, OverrideKeyError> {
    parse_override_key_with_overrides(name, &[])
}

/// Parses a variable key using the supplied active override names to interpret
/// legacy underscore suffixes such as `RDEPENDS_${PN}_class-native`.
pub fn parse_override_key_with_overrides(
    name: &str,
    active_overrides: &[&str],
) -> Result<OverrideKey, OverrideKeyError> {
    let active = active_overrides
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<BTreeSet<_>>();
    parse_key(name, &active, &BTreeMap::new())
}

/// The result of resolving statically known assignments under BitBake override
/// semantics.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OverrideResolution {
    overrides: Vec<String>,
    values: BTreeMap<String, String>,
}

impl OverrideResolution {
    /// Returns the active override list in BitBake order.
    pub fn overrides(&self) -> &[String] {
        &self.overrides
    }

    /// Returns the resolved value for a normalized base variable name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Returns all statically resolved base variables.
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }
}

/// Resolves statically known assignments using the file's `OVERRIDES` value.
///
/// Dynamic Python expansion, anonymous functions, and values that depend on
/// unavailable metadata are skipped rather than guessed. This gives callers a
/// deterministic static model while BitBake remains authoritative for fully
/// expanded build environments.
pub fn resolve_overrides(tree: &SyntaxTree<'_>) -> OverrideResolution {
    let raw = collect_assignments(tree);
    let override_value = resolve_override_list(&raw);
    let active = override_tokens(&override_value);
    resolve_with_active_overrides(&raw, active)
}

/// Resolves statically known assignments using a caller-supplied active
/// override list. This is useful for tools that obtain `OVERRIDES` from a
/// machine, distro, or target context outside the source file.
pub fn resolve_overrides_with_active(
    tree: &SyntaxTree<'_>,
    active_overrides: &[&str],
) -> OverrideResolution {
    let raw = collect_assignments(tree);
    let mut seen = BTreeSet::new();
    let active = active_overrides
        .iter()
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(*value))
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    resolve_with_active_overrides(&raw, active)
}

#[derive(Clone, Debug)]
struct RawAssignment {
    name: String,
    operator: AssignmentOperator,
    value: String,
    sequence: usize,
}

#[derive(Clone, Debug)]
struct ResolvedAssignment {
    key: OverrideKey,
    operator: AssignmentOperator,
    value: String,
    sequence: usize,
}

fn collect_assignments(tree: &SyntaxTree<'_>) -> Vec<RawAssignment> {
    tree.nodes()
        .iter()
        .enumerate()
        .filter_map(|(sequence, node)| match node.kind() {
            SyntaxKind::Assignment(assignment) => Some(RawAssignment {
                name: assignment.name().to_owned(),
                operator: assignment.operator(),
                value: assignment.value().to_owned(),
                sequence,
            }),
            _ => None,
        })
        .collect()
}

fn resolve_override_list(assignments: &[RawAssignment]) -> String {
    let mut value = String::new();
    let known_values = resolve_static_environment(assignments);
    let mut operations = BTreeMap::<OverrideOperation, Vec<(String, usize)>>::new();
    for assignment in assignments {
        let Ok(key) = parse_key(&assignment.name, &BTreeSet::new(), &BTreeMap::new()) else {
            continue;
        };
        if key.base != "OVERRIDES" || !key.overrides.is_empty() || key.flag.is_some() {
            continue;
        }
        let Some(assignment_value) = static_value(&assignment.value, &known_values) else {
            continue;
        };
        if key.operation != OverrideOperation::None {
            operations
                .entry(key.operation)
                .or_default()
                .push((assignment_value, assignment.sequence));
        } else {
            apply_assignment_operator(&mut value, assignment.operator, &assignment_value);
        }
    }
    for operation in [
        OverrideOperation::Append,
        OverrideOperation::Prepend,
        OverrideOperation::Remove,
    ] {
        if let Some(entries) = operations.get_mut(&operation) {
            entries.sort_by_key(|(_, sequence)| *sequence);
            for (entry, _) in entries {
                match operation {
                    OverrideOperation::Append => value.push_str(entry),
                    OverrideOperation::Prepend => value.insert_str(0, entry),
                    OverrideOperation::Remove => remove_override_tokens(&mut value, entry),
                    OverrideOperation::None => unreachable!(),
                }
            }
        }
    }
    value
}

fn override_tokens(value: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    value
        .split(|character: char| character == ':' || character.is_ascii_whitespace())
        .filter(|token| !token.is_empty())
        .filter_map(|token| {
            if seen.insert(token.to_owned()) {
                Some(token.to_owned())
            } else {
                None
            }
        })
        .collect()
}

fn resolve_with_active_overrides(
    assignments: &[RawAssignment],
    active_overrides: Vec<String>,
) -> OverrideResolution {
    let active = active_overrides.iter().cloned().collect::<BTreeSet<_>>();
    let known_values = resolve_static_environment(assignments);

    let mut grouped = BTreeMap::<String, Vec<ResolvedAssignment>>::new();
    for assignment in assignments {
        let Ok(key) = parse_key(&assignment.name, &active, &known_values) else {
            continue;
        };
        if key.flag.is_some()
            || key.dynamic
            || !key.overrides.iter().all(|item| active.contains(item))
        {
            continue;
        }
        let Some(value) = static_value(&assignment.value, &known_values) else {
            continue;
        };
        grouped
            .entry(key.base.clone())
            .or_default()
            .push(ResolvedAssignment {
                key,
                operator: assignment.operator,
                value,
                sequence: assignment.sequence,
            });
    }

    let mut values = BTreeMap::new();
    for (base, mut entries) in grouped {
        entries.sort_by_key(|entry| entry.sequence);
        let mut value = resolve_direct_entries(&entries, &active_overrides);
        for operation in [
            OverrideOperation::Append,
            OverrideOperation::Prepend,
            OverrideOperation::Remove,
        ] {
            for entry in entries.iter().filter(|entry| {
                entry.key.operation == operation && !entry.key.operation_on_selected_value
            }) {
                match operation {
                    OverrideOperation::Append => value.push_str(&entry.value),
                    OverrideOperation::Prepend => value.insert_str(0, &entry.value),
                    OverrideOperation::Remove => remove_override_tokens(&mut value, &entry.value),
                    OverrideOperation::None => unreachable!(),
                }
            }
        }
        values.insert(base, value);
    }
    values.insert("OVERRIDES".to_owned(), active_overrides.join(":"));
    OverrideResolution {
        overrides: active_overrides,
        values,
    }
}

fn resolve_direct_entries(entries: &[ResolvedAssignment], active: &[String]) -> String {
    let mut base_entries = entries
        .iter()
        .filter(|entry| {
            entry.key.operation == OverrideOperation::None && entry.key.overrides.is_empty()
        })
        .collect::<Vec<_>>();
    base_entries.sort_by_key(|entry| (entry.key.key_expanded, entry.sequence));
    let mut value = String::new();
    for entry in base_entries {
        apply_assignment_operator(&mut value, entry.operator, &entry.value);
    }

    let mut variants = BTreeMap::<Vec<String>, (String, usize, Vec<&ResolvedAssignment>)>::new();
    let mut variant_entries = entries
        .iter()
        .filter(|entry| {
            !entry.key.overrides.is_empty()
                && (entry.key.operation == OverrideOperation::None
                    || entry.key.operation_on_selected_value)
        })
        .collect::<Vec<_>>();
    variant_entries.sort_by_key(|entry| (entry.key.key_expanded, entry.sequence));
    for entry in variant_entries {
        let variant = variants
            .entry(entry.key.overrides.clone())
            .or_insert_with(|| (String::new(), 0, Vec::new()));
        variant.1 = variant.1.max(entry.sequence);
        if entry.key.operation == OverrideOperation::None {
            apply_assignment_operator(&mut variant.0, entry.operator, &entry.value);
        } else if entry.key.operation_on_selected_value {
            variant.2.push(entry);
        }
    }

    let mut candidates = Vec::new();
    for (overrides, (mut variant, sequence, operations)) in variants {
        for operation in [
            OverrideOperation::Append,
            OverrideOperation::Prepend,
            OverrideOperation::Remove,
        ] {
            for entry in operations
                .iter()
                .filter(|entry| entry.key.operation == operation)
            {
                match operation {
                    OverrideOperation::Append => variant.push_str(&entry.value),
                    OverrideOperation::Prepend => variant.insert_str(0, &entry.value),
                    OverrideOperation::Remove => remove_override_tokens(&mut variant, &entry.value),
                    OverrideOperation::None => unreachable!(),
                }
            }
        }
        candidates.push((overrides, variant, sequence));
    }

    if let Some((_, selected, _)) = candidates.into_iter().max_by(|left, right| {
        override_rank(&left.0, active)
            .cmp(&override_rank(&right.0, active))
            .then_with(|| left.2.cmp(&right.2))
    }) {
        value = selected;
    }
    value
}

fn override_rank(overrides: &[String], active: &[String]) -> (usize, Vec<usize>) {
    let mut positions = overrides
        .iter()
        .filter_map(|item| active.iter().position(|active_item| active_item == item))
        .collect::<Vec<_>>();
    positions.sort_unstable();
    (overrides.len(), positions)
}

fn apply_assignment_operator(value: &mut String, operator: AssignmentOperator, addition: &str) {
    match operator {
        AssignmentOperator::Assign | AssignmentOperator::Immediate => *value = addition.to_owned(),
        AssignmentOperator::Default | AssignmentOperator::WeakDefault => {
            if value.is_empty() {
                *value = addition.to_owned();
            }
        }
        AssignmentOperator::AppendWithSpace => {
            if !value.is_empty() && !addition.is_empty() {
                value.push(' ');
            }
            value.push_str(addition);
        }
        AssignmentOperator::PrependWithSpace => {
            if !value.is_empty() && !addition.is_empty() {
                value.insert(0, ' ');
            }
            value.insert_str(0, addition);
        }
        AssignmentOperator::AppendWithoutSpace => value.push_str(addition),
        AssignmentOperator::PrependWithoutSpace => value.insert_str(0, addition),
    }
}

fn remove_override_tokens(value: &mut String, removal: &str) {
    let removals = removal.split_ascii_whitespace().collect::<BTreeSet<_>>();
    let original = value.as_str();
    let mut output = String::with_capacity(original.len());
    let mut token_start = None;
    for (index, character) in original.char_indices() {
        if character.is_whitespace() {
            if let Some(start) = token_start.take() {
                let token = &original[start..index];
                if !removals.contains(token) {
                    output.push_str(token);
                }
            }
            output.push(character);
        } else if token_start.is_none() {
            token_start = Some(index);
        }
    }
    if let Some(start) = token_start {
        let token = &original[start..];
        if !removals.contains(token) {
            output.push_str(token);
        }
    }
    *value = output;
}

fn resolve_static_environment(assignments: &[RawAssignment]) -> BTreeMap<String, String> {
    let mut known_values = BTreeMap::new();
    for _ in 0..=assignments.len() {
        let mut next_values = BTreeMap::new();
        let mut ordered = assignments.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|assignment| (assignment.name.contains("${"), assignment.sequence));
        for assignment in ordered {
            let Ok(key) = parse_key(&assignment.name, &BTreeSet::new(), &known_values) else {
                continue;
            };
            if key.flag.is_some()
                || !key.overrides.is_empty()
                || key.operation != OverrideOperation::None
            {
                continue;
            }
            let Some(value) = static_value(&assignment.value, &known_values) else {
                continue;
            };
            apply_assignment_operator(
                next_values.entry(key.base).or_default(),
                assignment.operator,
                &value,
            );
        }
        if next_values == known_values {
            break;
        }
        known_values = next_values;
    }
    known_values
}

fn static_value(raw: &str, values: &BTreeMap<String, String>) -> Option<String> {
    let trimmed = raw.trim();
    let value = if let Some(quote) = trimmed
        .chars()
        .next()
        .filter(|quote| *quote == '\'' || *quote == '"')
    {
        if !trimmed.ends_with(quote) || trimmed.len() < 2 {
            return None;
        }
        &trimmed[quote.len_utf8()..trimmed.len() - quote.len_utf8()]
    } else {
        trimmed
    };
    let (expanded, dynamic) = expand_references(value, values);
    if dynamic {
        None
    } else {
        Some(expanded.replace("\\\n", " "))
    }
}

fn expand_references(value: &str, values: &BTreeMap<String, String>) -> (String, bool) {
    let mut output = String::new();
    let mut dynamic = false;
    let mut index = 0;
    while index < value.len() {
        let remainder = &value[index..];
        let Some(relative) = remainder.find("${") else {
            output.push_str(remainder);
            break;
        };
        output.push_str(&remainder[..relative]);
        let start = index + relative;
        let Some(end_relative) = value[start + 2..].find('}') else {
            dynamic = true;
            output.push_str(&value[start..]);
            break;
        };
        let end = start + 2 + end_relative;
        let name = &value[start + 2..end];
        if name.starts_with('@') {
            dynamic = true;
            output.push_str(&value[start..=end]);
        } else if let Some(resolved) = values.get(name) {
            output.push_str(resolved);
        } else {
            dynamic = true;
            output.push_str(&value[start..=end]);
        }
        index = end + 1;
    }
    (output, dynamic)
}

fn parse_key(
    name: &str,
    active: &BTreeSet<String>,
    values: &BTreeMap<String, String>,
) -> Result<OverrideKey, OverrideKeyError> {
    let (body, flag) = split_flag(name)?;
    let modern = body.contains(':');
    if modern {
        let parts = split_components(body)?;
        let base = parts.first().cloned().unwrap_or_default();
        if base.is_empty() {
            return Err(OverrideKeyError::new(
                "override key has an empty base variable",
            ));
        }
        let mut operation = OverrideOperation::None;
        let mut operation_position = None;
        let mut components = Vec::new();
        for (position, component) in parts.iter().skip(1).enumerate() {
            if let Some(candidate) = operation_for(component) {
                if operation != OverrideOperation::None {
                    return Err(OverrideKeyError::new(
                        "override key contains more than one override operation",
                    ));
                }
                operation = candidate;
                operation_position = Some(position);
            } else {
                components.push(component.to_string());
            }
        }
        let key_expanded =
            base.contains("${") || components.iter().any(|component| component.contains("${"));
        let (base, base_dynamic) = expand_references(base, values);
        let (components, dynamic_components) = expand_components(&components, values);
        return Ok(OverrideKey {
            raw: name.to_owned(),
            base,
            overrides: components,
            operation,
            flag,
            legacy: false,
            dynamic: base_dynamic || dynamic_components,
            operation_on_selected_value: operation_position.is_some_and(|position| position > 0),
            key_expanded,
        });
    }

    let (without_operation, operation, had_operation, operation_after_suffix) =
        remove_legacy_operation(body)?;
    let mut base = without_operation.to_owned();
    let mut components = Vec::new();
    loop {
        let Some((prefix, component)) = longest_legacy_suffix(&base, active) else {
            break;
        };
        base = prefix.to_owned();
        components.push(component.to_owned());
    }
    components.reverse();
    let has_legacy_components = !components.is_empty();
    let operation_on_selected_value =
        had_operation && operation_after_suffix && has_legacy_components;
    let key_expanded =
        base.contains("${") || components.iter().any(|component| component.contains("${"));
    let (base, dynamic) = expand_references(&base, values);
    Ok(OverrideKey {
        raw: name.to_owned(),
        base,
        overrides: components,
        operation,
        flag,
        legacy: had_operation || has_legacy_components || body.contains('_'),
        dynamic,
        operation_on_selected_value,
        key_expanded,
    })
}

fn split_flag(name: &str) -> Result<(&str, Option<String>), OverrideKeyError> {
    let Some(open) = name.find('[') else {
        return Ok((name, None));
    };
    if !name.ends_with(']') || name[open + 1..name.len() - 1].contains('[') {
        return Err(OverrideKeyError::new(
            "override key has malformed variable flag",
        ));
    }
    let flag = &name[open + 1..name.len() - 1];
    if flag.is_empty() {
        return Err(OverrideKeyError::new(
            "override key has an empty variable flag",
        ));
    }
    Ok((&name[..open], Some(flag.to_owned())))
}

fn split_components(name: &str) -> Result<Vec<&str>, OverrideKeyError> {
    let mut components = Vec::new();
    let mut start = 0;
    let mut braces = 0usize;
    let bytes = name.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"${") {
            braces += 1;
            index += 2;
            continue;
        }
        match bytes[index] {
            b'}' if braces > 0 => braces -= 1,
            b':' if braces == 0 => {
                components.push(&name[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if braces != 0 {
        return Err(OverrideKeyError::new(
            "override key contains an unclosed variable expansion",
        ));
    }
    components.push(&name[start..]);
    if components.iter().any(|component| component.is_empty()) {
        return Err(OverrideKeyError::new(
            "override key contains an empty component",
        ));
    }
    Ok(components)
}

fn expand_components(
    components: &[String],
    values: &BTreeMap<String, String>,
) -> (Vec<String>, bool) {
    let mut dynamic = false;
    let components = components
        .iter()
        .map(|component| {
            let (expanded, unresolved) = expand_references(component, values);
            dynamic |= unresolved;
            expanded
        })
        .collect();
    (components, dynamic)
}

fn operation_for(component: &str) -> Option<OverrideOperation> {
    match component {
        "append" => Some(OverrideOperation::Append),
        "prepend" => Some(OverrideOperation::Prepend),
        "remove" => Some(OverrideOperation::Remove),
        _ => None,
    }
}

fn remove_legacy_operation(
    name: &str,
) -> Result<(String, OverrideOperation, bool, bool), OverrideKeyError> {
    let mut found = None;
    for operation in ["append", "prepend", "remove"] {
        let marker = format!("_{operation}");
        let mut search = 0;
        while let Some(relative) = name[search..].find(&marker) {
            let start = search + relative;
            let end = start + marker.len();
            let boundary = end == name.len() || name.as_bytes().get(end) == Some(&b'_');
            if boundary && !inside_reference(name, start) {
                if found.is_some() {
                    return Err(OverrideKeyError::new(
                        "legacy override key contains more than one operation",
                    ));
                }
                found = Some((start, end, operation));
            }
            search = end;
        }
    }
    let Some((start, end, operation)) = found else {
        return Ok((name.to_owned(), OverrideOperation::None, false, false));
    };
    let without = &name[..start];
    let suffix = &name[end..];
    let mut compact = String::with_capacity(without.len() + suffix.len());
    compact.push_str(without);
    compact.push_str(suffix);
    Ok((
        compact,
        operation_for(operation).unwrap(),
        true,
        suffix.is_empty() && !without.is_empty(),
    ))
}

fn inside_reference(name: &str, index: usize) -> bool {
    let prefix = &name[..index];
    prefix
        .rfind("${")
        .is_some_and(|start| prefix[start + 2..].find('}').is_none())
}

fn longest_legacy_suffix<'a>(
    name: &'a str,
    active: &BTreeSet<String>,
) -> Option<(&'a str, String)> {
    active
        .iter()
        .filter_map(|component| {
            let marker = format!("_{component}");
            let prefix = name.strip_suffix(&marker)?;
            (!prefix.is_empty()).then_some((prefix, component.clone()))
        })
        .max_by_key(|(_, component)| component.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn parses_modern_and_legacy_override_keys() {
        let modern = parse_override_key("RDEPENDS:${PN}:class-native").unwrap();
        assert_eq!(modern.base(), "RDEPENDS");
        assert_eq!(
            modern.overrides(),
            &["${PN}".to_owned(), "class-native".to_owned()]
        );
        assert!(modern.is_dynamic());
        assert!(!modern.is_legacy());

        let legacy = parse_override_key_with_overrides(
            "RDEPENDS_${PN}_class-native_append",
            &["class-native"],
        )
        .unwrap();
        assert_eq!(legacy.base(), "RDEPENDS_${PN}");
        assert_eq!(legacy.overrides(), &["class-native".to_owned()]);
        assert_eq!(legacy.operation(), OverrideOperation::Append);
        assert!(legacy.is_legacy());

        let selected =
            parse_override_key_with_overrides("A:class-native:append", &["class-native"]).unwrap();
        assert!(selected.operation_on_selected_value());

        let deferred =
            parse_override_key_with_overrides("A:append:class-native", &["class-native"]).unwrap();
        assert!(!deferred.operation_on_selected_value());

        assert!(parse_override_key("A::append").is_err());
        assert!(parse_override_key("A:append:remove").is_err());
    }

    #[test]
    fn resolves_override_precedence_operations_key_expansion_and_legacy_names() {
        let tree = parse(concat!(
            "OVERRIDES = \"machine:class-native\"\n",
            "PN = \"demo\"\n",
            "A = \"base\"\n",
            "A:machine = \"machine\"\n",
            "A:class-native = \"native\"\n",
            "A:prepend:class-native = \"prefix \"\n",
            "A:append = \" suffix\"\n",
            "A:remove = \"base\"\n",
            "RDEPENDS_${PN}_class-native = \"native-dependency\"\n",
            "B = \"one\"\n",
            "B_append_class-native = \" two\"\n",
        ))
        .unwrap();
        let resolved = resolve_overrides(&tree);

        assert_eq!(resolved.overrides(), &["machine", "class-native"]);
        assert_eq!(resolved.get("A"), Some("prefix native suffix"));
        assert_eq!(resolved.get("RDEPENDS_demo"), Some("native-dependency"));
        assert_eq!(resolved.get("B"), Some("one two"));
    }

    #[test]
    fn applies_override_operations_in_bitbake_order() {
        let tree = parse(concat!(
            "OVERRIDES = \"machine\"\n",
            "A = \"one\"\n",
            "A:remove = \"two\"\n",
            "A:append = \" two\"\n",
            "A:prepend = \"prefix \"\n",
            "B = \"base\"\n",
            "B:machine:append = \"-machine\"\n",
            "B:append:machine = \"-deferred\"\n",
        ))
        .unwrap();
        let resolved = resolve_overrides(&tree);

        assert_eq!(resolved.get("A"), Some("prefix one "));
        assert_eq!(resolved.get("B"), Some("-machine-deferred"));
    }

    #[test]
    fn expands_static_keys_after_all_assignments_are_known() {
        let tree = parse(concat!(
            "A${B} = \"expanded-key\"\n",
            "B = \"2\"\n",
            "A2 = \"literal-key\"\n",
        ))
        .unwrap();
        let resolved = resolve_overrides_with_active(&tree, &[]);

        assert_eq!(resolved.get("A2"), Some("expanded-key"));
    }

    #[test]
    fn caller_can_supply_override_context_when_source_is_dynamic() {
        let tree =
            parse("A = \"base\"\nA:machine = \"machine\"\nA:append:machine = \"!\"\n").unwrap();
        let resolved = resolve_overrides_with_active(&tree, &["machine"]);
        assert_eq!(resolved.get("A"), Some("machine!"));
    }
}

use super::*;

pub(super) fn validate_reachable_fragment_graph<'ast>(
    fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
    document: &ClientDocument,
    selection_set: &'ast Positioned<SelectionSet>,
) -> Result<(), ClientCompileError> {
    validate_fragment_selection_set(
        fragments,
        document,
        &mut FragmentGraphState::default(),
        selection_set,
        1,
    )
}

pub(super) fn validate_fragment_selection_set<'ast>(
    fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
    document: &ClientDocument,
    state: &mut FragmentGraphState,
    selection_set: &'ast Positioned<SelectionSet>,
    depth: usize,
) -> Result<(), ClientCompileError> {
    check_expansion_depth(depth, document, selection_set.pos)?;
    for selection in &selection_set.node.items {
        match &selection.node {
            Selection::Field(field) => {
                if !field.node.selection_set.node.items.is_empty() {
                    validate_fragment_selection_set(
                        fragments,
                        document,
                        state,
                        &field.node.selection_set,
                        depth + 1,
                    )?;
                }
            }
            Selection::InlineFragment(fragment) => {
                validate_fragment_selection_set(
                    fragments,
                    document,
                    state,
                    &fragment.node.selection_set,
                    depth + 1,
                )?;
            }
            Selection::FragmentSpread(spread) => {
                let name = spread.node.fragment_name.node.as_str();
                let Some(definition) = fragments.get(name) else {
                    return Err(source_error(
                        "client.graphql.fragment_undefined",
                        format!("fragment spread `{name}` has no definition in this document"),
                        document,
                        spread.node.fragment_name.pos,
                    ));
                };
                if let Some(cycle_start) = state
                    .active_fragments
                    .iter()
                    .position(|active| active == name)
                {
                    let mut cycle = state.active_fragments[cycle_start..].to_vec();
                    cycle.push(name.to_string());
                    return Err(source_error(
                        "client.graphql.fragment_cycle",
                        format!("fragment expansion cycle: {}", cycle.join(" -> ")),
                        document,
                        spread.node.fragment_name.pos,
                    ));
                }
                if state.completed_fragments.contains(name) {
                    continue;
                }
                check_expansion_depth(depth + 1, document, spread.pos)?;
                state.active_fragments.push(name.to_string());
                let result = validate_fragment_selection_set(
                    fragments,
                    document,
                    state,
                    &definition.node.selection_set,
                    depth + 1,
                );
                state.active_fragments.pop();
                result?;
                state.completed_fragments.insert(name.to_string());
            }
        }
    }
    Ok(())
}

impl<'ast, 'source> FragmentExpander<'ast, 'source> {
    pub(super) fn new(
        fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
        document: &'source ClientDocument,
    ) -> Self {
        Self {
            fragments,
            document,
            state: ExpansionState::default(),
        }
    }

    pub(super) fn merge_object(
        &mut self,
        selection_sets: &[&'ast Positioned<SelectionSet>],
        typename: &str,
        depth: usize,
        field_owner: &str,
    ) -> Result<Vec<MergedField<'ast>>, ClientCompileError> {
        let position = selection_sets
            .first()
            .map_or_else(Pos::default, |selection_set| selection_set.pos);
        check_expansion_depth(depth, self.document, position)?;
        count_expansion_unit(&mut self.state, self.document, position)?;

        let mut fields = Vec::new();
        let mut response_keys = BTreeMap::new();
        for selection_set in selection_sets {
            expand_selection_set(
                self.fragments,
                self.document,
                &mut self.state,
                selection_set,
                typename,
                depth,
                field_owner,
                &mut fields,
                &mut response_keys,
            )?;
        }
        Ok(fields)
    }

    pub(super) fn reject_unused_fragments(&self) -> Result<(), ClientCompileError> {
        let unused = self
            .fragments
            .iter()
            .filter(|(name, _)| !self.state.used_fragments.contains(name.as_str()))
            .min_by(|left, right| {
                (left.1.pos, left.0.as_str()).cmp(&(right.1.pos, right.0.as_str()))
            });
        let Some((name, definition)) = unused else {
            return Ok(());
        };
        Err(source_error(
            "client.graphql.fragment_unused",
            format!("fragment `{name}` is not reachable from the document operation"),
            self.document,
            definition.pos,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn expand_selection_set<'ast>(
    fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
    document: &ClientDocument,
    state: &mut ExpansionState,
    selection_set: &'ast Positioned<SelectionSet>,
    typename: &str,
    depth: usize,
    field_owner: &str,
    fields: &mut Vec<MergedField<'ast>>,
    response_keys: &mut BTreeMap<String, usize>,
) -> Result<(), ClientCompileError> {
    check_expansion_depth(depth, document, selection_set.pos)?;
    for selection in &selection_set.node.items {
        count_expansion_unit(state, document, selection.pos)?;
        match &selection.node {
            Selection::Field(field) => {
                merge_field(document, field, field_owner, fields, response_keys)?
            }
            Selection::FragmentSpread(spread) => {
                reject_directives(
                    &spread.node.directives,
                    &format!(
                        "fragment spread `{}`",
                        spread.node.fragment_name.node.as_str()
                    ),
                    document,
                )?;
                let name = spread.node.fragment_name.node.as_str();
                let Some(definition) = fragments.get(name) else {
                    return Err(source_error(
                        "client.graphql.fragment_undefined",
                        format!("fragment spread `{name}` has no definition in this document"),
                        document,
                        spread.node.fragment_name.pos,
                    ));
                };
                state.used_fragments.insert(name.to_string());
                reject_directives(
                    &definition.node.directives,
                    &format!("fragment definition `{name}`"),
                    document,
                )?;
                require_fragment_type(
                    definition.node.type_condition.node.on.node.as_str(),
                    typename,
                    name,
                    document,
                    definition.node.type_condition.pos,
                )?;
                if let Some(cycle_start) = state
                    .active_fragments
                    .iter()
                    .position(|active| active == name)
                {
                    let mut cycle = state.active_fragments[cycle_start..].to_vec();
                    cycle.push(name.to_string());
                    return Err(source_error(
                        "client.graphql.fragment_cycle",
                        format!("fragment expansion cycle: {}", cycle.join(" -> ")),
                        document,
                        spread.node.fragment_name.pos,
                    ));
                }
                check_expansion_depth(depth + 1, document, spread.pos)?;
                state.active_fragments.push(name.to_string());
                let result = expand_selection_set(
                    fragments,
                    document,
                    state,
                    &definition.node.selection_set,
                    typename,
                    depth + 1,
                    field_owner,
                    fields,
                    response_keys,
                );
                state.active_fragments.pop();
                result?;
            }
            Selection::InlineFragment(fragment) => {
                reject_directives(&fragment.node.directives, "inline fragment", document)?;
                if let Some(condition) = &fragment.node.type_condition {
                    require_fragment_type(
                        condition.node.on.node.as_str(),
                        typename,
                        "inline fragment",
                        document,
                        condition.pos,
                    )?;
                }
                check_expansion_depth(depth + 1, document, fragment.pos)?;
                expand_selection_set(
                    fragments,
                    document,
                    state,
                    &fragment.node.selection_set,
                    typename,
                    depth + 1,
                    field_owner,
                    fields,
                    response_keys,
                )?;
            }
        }
    }
    Ok(())
}

pub(super) fn merge_field<'ast>(
    document: &ClientDocument,
    field: &'ast Positioned<Field>,
    field_owner: &str,
    fields: &mut Vec<MergedField<'ast>>,
    response_keys: &mut BTreeMap<String, usize>,
) -> Result<(), ClientCompileError> {
    reject_directives(&field.node.directives, field_owner, document)?;
    let response_key = field.node.response_key().node.as_str();
    let canonical_arguments = canonical_field_arguments(&field.node, document)?;
    let is_object = !field.node.selection_set.node.items.is_empty();

    if let Some(index) = response_keys.get(response_key).copied() {
        let first = &mut fields[index];
        let first_is_object = !first.selection_sets.is_empty();
        if first.first.node.name.node != field.node.name.node
            || first.canonical_arguments != canonical_arguments
            || first_is_object != is_object
        {
            let first_position = first.first.node.response_key().pos;
            return Err(source_error(
                "client.selection.conflict",
                format!(
                    "response key `{response_key}` conflicts with its first selection at {}:{}",
                    first_position.line.max(1),
                    first_position.column.max(1)
                ),
                document,
                field.node.response_key().pos,
            ));
        }
        if is_object {
            first.selection_sets.push(&field.node.selection_set);
        }
        return Ok(());
    }

    response_keys.insert(response_key.to_string(), fields.len());
    fields.push(MergedField {
        first: field,
        selection_sets: is_object
            .then_some(&field.node.selection_set)
            .into_iter()
            .collect(),
        canonical_arguments,
    });
    Ok(())
}

pub(super) fn canonical_field_arguments(
    field: &Field,
    document: &ClientDocument,
) -> Result<Vec<(String, String)>, ClientCompileError> {
    let mut arguments = field
        .arguments
        .iter()
        .map(|(name, value)| {
            Ok((
                name.node.to_string(),
                render_value(&value.node, document, value.pos)?,
            ))
        })
        .collect::<Result<Vec<_>, ClientCompileError>>()?;
    arguments.sort();
    Ok(arguments)
}

pub(super) fn require_fragment_type(
    actual: &str,
    expected: &str,
    fragment: &str,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if actual == expected {
        return Ok(());
    }
    Err(source_error(
        "client.graphql.fragment_type",
        format!(
            "{fragment} has type condition `{actual}` but the current concrete type is `{expected}`"
        ),
        document,
        position,
    ))
}

pub(super) fn check_expansion_depth(
    depth: usize,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if depth <= MAX_OBJECT_DEPTH {
        return Ok(());
    }
    Err(source_error(
        "client.selection.depth",
        format!("selection expansion exceeds the supported {MAX_OBJECT_DEPTH}-level depth"),
        document,
        position,
    ))
}

pub(super) fn count_expansion_unit(
    state: &mut ExpansionState,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if state.expanded_units >= MAX_EXPANDED_SELECTIONS {
        return Err(source_error(
            "client.selection.expansion_bound",
            format!(
                "expanded selection exceeds the supported {MAX_EXPANDED_SELECTIONS}-unit bound"
            ),
            document,
            position,
        ));
    }
    state.expanded_units += 1;
    Ok(())
}

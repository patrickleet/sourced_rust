use std::collections::{BTreeMap, BTreeSet};

use super::{
    ProjectionDeltaError, ProjectionDeltaMutation, ProjectionDeltaOperation,
    ProjectionDeltaRecovery,
};

pub(crate) fn canonicalize_operations(
    operations: Vec<ProjectionDeltaOperation>,
) -> Result<Vec<ProjectionDeltaOperation>, ProjectionDeltaError> {
    let mut by_scope = BTreeMap::new();
    for operation in operations {
        let scope = operation.canonical_scope();
        if let Some(existing) = by_scope.get_mut(&scope) {
            merge_operation(existing, operation)?;
        } else {
            by_scope.insert(scope, operation);
        }
    }
    let mut operations = by_scope.into_values().collect::<Vec<_>>();
    operations.sort_by_key(ProjectionDeltaOperation::canonical_order);
    Ok(operations)
}

pub(crate) fn canonicalize_recoveries(
    recoveries: Vec<ProjectionDeltaRecovery>,
) -> Vec<ProjectionDeltaRecovery> {
    let mut by_target = BTreeMap::new();
    for recovery in recoveries {
        by_target
            .entry(recovery.target.clone())
            .and_modify(|existing: &mut ProjectionDeltaRecovery| {
                existing.occurrence_ordinal =
                    existing.occurrence_ordinal.max(recovery.occurrence_ordinal);
                merge_refs(
                    &mut existing.projection_refs,
                    recovery.projection_refs.clone(),
                );
            })
            .or_insert(recovery);
    }
    let mut recoveries = by_target.into_values().collect::<Vec<_>>();
    recoveries.sort_by_key(ProjectionDeltaRecovery::canonical_order);
    recoveries
}

fn merge_operation(
    existing: &mut ProjectionDeltaOperation,
    incoming: ProjectionDeltaOperation,
) -> Result<(), ProjectionDeltaError> {
    existing.occurrence_ordinal = existing.occurrence_ordinal.max(incoming.occurrence_ordinal);
    merge_refs(&mut existing.projection_refs, incoming.projection_refs);
    match (&mut existing.mutation, incoming.mutation) {
        (
            ProjectionDeltaMutation::Patch {
                set,
                unset,
                if_present,
                ..
            },
            ProjectionDeltaMutation::Patch {
                set: incoming_set,
                unset: incoming_unset,
                if_present: incoming_present,
                ..
            },
        ) => {
            *if_present &= incoming_present;
            let mut values = set
                .drain(..)
                .map(|field| (field.field.clone(), Some(field)))
                .collect::<BTreeMap<_, _>>();
            for field in unset.drain(..) {
                values.insert(field, None);
            }
            for field in incoming_set {
                values.insert(field.field.clone(), Some(field));
            }
            for field in incoming_unset {
                values.insert(field, None);
            }
            *set = values.values().filter_map(Clone::clone).collect();
            *unset = values
                .into_iter()
                .filter_map(|(field, value)| value.is_none().then_some(field))
                .collect();
            Ok(())
        }
        (
            ProjectionDeltaMutation::Upsert {
                fields, replace, ..
            },
            ProjectionDeltaMutation::Patch {
                set,
                unset,
                if_present: true,
                ..
            },
        ) => {
            let mut values = fields
                .drain(..)
                .map(|field| (field.field.clone(), field))
                .collect::<BTreeMap<_, _>>();
            for field in set {
                values.insert(field.field.clone(), field);
            }
            for field in unset {
                values.remove(&field);
            }
            *fields = values.into_values().collect();
            replace.sort();
            replace.dedup();
            Ok(())
        }
        (slot, incoming) if *slot == incoming => Ok(()),
        _ => Err(ProjectionDeltaError::InvalidOperation(
            "same scope resolves to incompatible final mutations",
        )),
    }
}

fn merge_refs(existing: &mut Vec<u32>, incoming: Vec<u32>) {
    let mut refs = existing.iter().copied().collect::<BTreeSet<_>>();
    refs.extend(incoming);
    *existing = refs.into_iter().collect();
}

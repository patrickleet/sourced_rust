use std::collections::HashMap;

use super::{ProjectionModelOwnership, ProjectionProtocolError};

pub(crate) fn validate_ownership_batch(
    ownership: &[ProjectionModelOwnership],
) -> Result<(), ProjectionProtocolError> {
    let mut models = HashMap::new();
    let mut tables = HashMap::new();
    for declaration in ownership {
        if let Some(previous) =
            models.insert(declaration.model.as_str(), declaration.table.as_str())
        {
            if previous != declaration.table {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection model `{}` declares both table `{previous}` and `{}`",
                    declaration.model, declaration.table
                )));
            }
        }
        if let Some(previous) =
            tables.insert(declaration.table.as_str(), declaration.model.as_str())
        {
            if previous != declaration.model {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection table `{}` is declared by both model `{previous}` and `{}`",
                    declaration.table, declaration.model
                )));
            }
        }
    }
    Ok(())
}

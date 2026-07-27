use super::{ProjectionChangeKind, ProjectionObservationKind};

impl ProjectionChangeKind {
    pub(crate) fn from_storage_str(value: &str) -> Option<Self> {
        match value {
            "checkpoint" => Some(Self::Checkpoint),
            "record_upsert" => Some(Self::RecordUpsert),
            "record_delete" => Some(Self::RecordDelete),
            "record_recreate" => Some(Self::RecordRecreate),
            "observation" => Some(Self::Observation),
            "failure" => Some(Self::Failure),
            _ => None,
        }
    }
}

impl ProjectionObservationKind {
    pub(crate) fn from_storage_str(value: &str) -> Option<Self> {
        match value {
            "record" => Some(Self::Record),
            "dependency" => Some(Self::Dependency),
            _ => None,
        }
    }
}
